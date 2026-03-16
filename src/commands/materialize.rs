//! Materialize a RepoRecord onto disk.
//!
//! Projects the config-first registry state (LocalForge) into per-repo
//! `.hyperforge/config.toml` and reconciles git remotes to match.

use std::path::Path;

use crate::config::{HyperforgeConfig, OrgConfig};
use crate::git::{build_remote_url, Git};
use crate::types::RepoRecord;

/// Options controlling which parts of materialization to perform.
pub struct MaterializeOpts {
    /// Write `.hyperforge/config.toml` (default true)
    pub config: bool,
    /// Reconcile git remotes (default true)
    pub remotes: bool,
    /// Install pre-push hook (default false)
    pub hooks: bool,
    /// Configure SSH wrapper (default false)
    pub ssh_wrapper: bool,
    /// Report without writing (default false)
    pub dry_run: bool,
}

impl Default for MaterializeOpts {
    fn default() -> Self {
        Self {
            config: true,
            remotes: true,
            hooks: false,
            ssh_wrapper: false,
            dry_run: false,
        }
    }
}

/// Report of what materialization changed (or would change for dry-run).
pub struct MaterializeReport {
    /// Whether `.hyperforge/config.toml` was written
    pub config_written: bool,
    /// Remote names that were added
    pub remotes_added: Vec<String>,
    /// Remote names whose URL was updated
    pub remotes_updated: Vec<String>,
    /// Whether hooks were installed
    pub hooks_installed: bool,
    /// Whether SSH wrapper was configured
    pub ssh_configured: bool,
    /// Warnings emitted during materialization
    pub warnings: Vec<String>,
}

/// Project a `RepoRecord` onto disk at the given path.
///
/// Writes the per-repo config, reconciles git remotes, and optionally installs
/// hooks.  When `opts.dry_run` is true the report is populated but nothing is
/// written to disk.
pub fn materialize(
    org: &str,
    record: &RepoRecord,
    repo_path: &Path,
    opts: MaterializeOpts,
) -> Result<MaterializeReport, String> {
    let mut report = MaterializeReport {
        config_written: false,
        remotes_added: Vec::new(),
        remotes_updated: Vec::new(),
        hooks_installed: false,
        ssh_configured: false,
        warnings: Vec::new(),
    };

    // ── Build config from record (needed for both writing and remote naming) ──

    let dir_name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let repo_name = if record.name != dir_name {
        Some(record.name.clone())
    } else {
        None
    };

    let default_branch = if record.default_branch == "main" {
        None
    } else {
        Some(record.default_branch.clone())
    };

    let config = HyperforgeConfig {
        repo_name,
        org: Some(org.to_string()),
        forges: record.forges.clone(),
        visibility: record.visibility.clone(),
        description: record.description.clone(),
        ssh: record.ssh.clone(),
        forge_config: record.forge_config.clone(),
        default_branch,
        ci: record.ci.clone(),
        large_file_threshold_kb: None,
    };

    // ── Step 1: config ──────────────────────────────────────────────────

    if opts.config {
        if !opts.dry_run {
            config
                .save(repo_path)
                .map_err(|e| format!("failed to write config: {}", e))?;
        }
        report.config_written = true;
    }

    // ── Step 2: remotes ─────────────────────────────────────────────────

    if opts.remotes && repo_path.join(".git").exists() {
        // Compute desired remotes from record.forges
        let mut desired: Vec<(String, String)> = Vec::new(); // (remote_name, url)

        for forge_str in record.forges.iter() {
            // Determine the org for this forge: check forge_config override, fall back to param
            let org_for_forge = record
                .forge_config
                .get(forge_str)
                .and_then(|fc| fc.org.as_deref())
                .unwrap_or(org);

            let url = build_remote_url(forge_str, org_for_forge, &record.name);

            // Use config.remote_for_forge to respect forge_config.remote overrides
            let remote_name = config.remote_for_forge(forge_str);

            desired.push((remote_name, url));
        }

        // Get current remotes
        let current_remotes = Git::list_remotes(repo_path)
            .map_err(|e| format!("failed to list remotes: {}", e))?;

        for (remote_name, desired_url) in &desired {
            if let Some(existing) = current_remotes.iter().find(|r| r.name == *remote_name) {
                // Remote exists — check if URL matches (compare against fetch_url)
                if existing.fetch_url != *desired_url {
                    if !opts.dry_run {
                        Git::set_remote_url(repo_path, remote_name, desired_url)
                            .map_err(|e| format!("failed to set remote url: {}", e))?;
                    }
                    report.remotes_updated.push(remote_name.clone());
                }
                // else: URL matches, nothing to do
            } else {
                // Remote does not exist — add it
                if !opts.dry_run {
                    Git::add_remote(repo_path, remote_name, desired_url)
                        .map_err(|e| format!("failed to add remote: {}", e))?;
                }
                report.remotes_added.push(remote_name.clone());
            }
        }
    }

    // ── Step 3: hooks ───────────────────────────────────────────────────

    if opts.hooks {
        let installed = crate::commands::hooks::install_pre_push_hook(repo_path, opts.dry_run)
            .map_err(|e| format!("failed to install pre-push hook: {}", e))?;
        report.hooks_installed = installed;
    }

    // ── Step 4: SSH wrapper ─────────────────────────────────────────────

    if opts.ssh_wrapper {
        // Resolve SSH key: per-repo first, then org-level defaults
        let ssh_key = record.ssh.iter().next().map(|(_f, k)| k.clone()).or_else(|| {
            let config_dir = dirs::home_dir()
                .unwrap_or_default()
                .join(".config")
                .join("hyperforge");
            let org_config = OrgConfig::load(&config_dir, org);
            // Pick the first org-level key that matches one of our forges
            record.forges.iter()
                .find_map(|f| org_config.ssh_key_for_forge(f).map(|k| k.to_string()))
        });

        match ssh_key {
            Some(key_path) => {
                if !opts.dry_run {
                    Git::configure_ssh(repo_path, &key_path)
                        .map_err(|e| format!("failed to configure SSH wrapper: {}", e))?;
                }
                report.ssh_configured = true;
            }
            None => {
                report.warnings.push(
                    "No SSH keys configured (per-repo or org-level). \
                     Pushes will use your default SSH agent keys. \
                     Set org defaults with: synapse lforge hyperforge config set_ssh_key"
                        .to_string(),
                );
            }
        }
    }

    Ok(report)
}
