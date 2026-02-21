//! WorkspaceHub - Multi-repo orchestration
//!
//! Supports two modes:
//! - Path-based: `--path ~/dev/org` discovers repos from filesystem
//! - Org-based (legacy): `--org acme --forge github` uses registry directly

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;

use crate::adapters::{CodebergAdapter, ForgePort, ForgeSyncState, GitHubAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::commands::init::{init, InitOptions};
use crate::commands::push::{push, PushOptions};
use crate::commands::workspace::{discover_workspace, repo_from_config};
use crate::config::HyperforgeConfig;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;
use crate::services::SyncOp;
use crate::types::{Forge, OwnerType, Visibility};
use std::collections::HashSet;

/// Sub-hub for multi-repo workspace orchestration
#[derive(Clone)]
pub struct WorkspaceHub {
    pub(crate) state: HyperforgeState,
}

impl WorkspaceHub {
    pub fn new(state: HyperforgeState) -> Self {
        Self { state }
    }
}

/// Create a forge adapter from a forge name string
pub(crate) fn make_adapter(forge: &str, org: &str, owner_type: Option<OwnerType>) -> Result<Arc<dyn ForgePort>, String> {
    let auth = YamlAuthProvider::new().map_err(|e| format!("Failed to create auth provider: {}", e))?;
    let auth = Arc::new(auth);
    let target_forge = match forge.to_lowercase().as_str() {
        "github" => Forge::GitHub,
        "codeberg" => Forge::Codeberg,
        "gitlab" => Forge::GitLab,
        _ => return Err(format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge)),
    };
    let adapter: Arc<dyn ForgePort> = match target_forge {
        Forge::GitHub => {
            let a = GitHubAdapter::new(auth, org).map_err(|e| format!("Failed to create GitHub adapter: {}", e))?;
            Arc::new(match owner_type { Some(ot) => a.with_owner_type(ot), None => a })
        }
        Forge::Codeberg => {
            let a = CodebergAdapter::new(auth, org).map_err(|e| format!("Failed to create Codeberg adapter: {}", e))?;
            Arc::new(match owner_type { Some(ot) => a.with_owner_type(ot), None => a })
        }
        Forge::GitLab => {
            let a = GitLabAdapter::new(auth, org).map_err(|e| format!("Failed to create GitLab adapter: {}", e))?;
            Arc::new(match owner_type { Some(ot) => a.with_owner_type(ot), None => a })
        }
    };
    Ok(adapter)
}

/// Simple glob matching for repo name filtering
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if pattern.contains('*') {
        // Simple prefix*suffix matching
        let parts: Vec<&str> = pattern.splitn(2, '*').collect();
        if parts.len() == 2 {
            return name.starts_with(parts[0]) && name.ends_with(parts[1]);
        }
    }
    name == pattern
}

#[plexus_macros::hub_methods(
    namespace = "workspace",
    description = "Multi-repo workspace orchestration",
    crate_path = "plexus_core"
)]
impl WorkspaceHub {
    /// Discover repos in a workspace directory
    #[plexus_macros::hub_method(
        description = "Scan a workspace directory and report discovered repos, orgs, and forges",
        params(
            path = "Path to workspace directory"
        )
    )]
    pub async fn discover(
        &self,
        path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);
            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Scanning workspace: {}", ctx.root.display()),
            };

            // Report each discovered repo
            for repo in &ctx.repos {
                let org = repo.org().unwrap_or("(none)");
                let forges = repo.forges().join(", ");
                let git_status = if repo.is_git_repo { "git" } else { "no-git" };
                let bs_label = format!("{}", repo.build_system);

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {} [{}] org={} forges=[{}] build=[{}]",
                        repo.dir_name, git_status, org, forges, bs_label
                    ),
                };
            }

            // Report unconfigured repos
            for path in &ctx.unconfigured_repos {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                yield HyperforgeEvent::Info {
                    message: format!("  {} [git, no hyperforge config]", name),
                };
            }

            // Report orgs, forges, and build systems
            yield HyperforgeEvent::Info {
                message: format!("Orgs: {}", ctx.orgs.join(", ")),
            };
            yield HyperforgeEvent::Info {
                message: format!("Forges: {}", ctx.forges.join(", ")),
            };
            let bs_list: Vec<String> = ctx.build_systems().iter().map(|bs| format!("{}", bs)).collect();
            if !bs_list.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("Build systems: {}", bs_list.join(", ")),
                };
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: None,
                push_failed: None,
                validation_passed: None,
            };
        }
    }

    /// Initialize unconfigured repos in a workspace
    #[plexus_macros::hub_method(
        description = "Initialize hyperforge config for unconfigured repos in a workspace directory. Discovers repos, infers org/forges from existing configs, and creates .hyperforge/config.toml for each unconfigured repo.",
        params(
            path = "Path to workspace directory",
            org = "Organization name (inferred from workspace if only one org exists)",
            forges = "Forges to configure (inferred from existing configs if not specified)",
            dry_run = "Preview without writing configs (optional, default: false)",
            force = "Re-init repos that already have config (optional, default: false)",
            no_hooks = "Skip installing pre-push hook (optional, default: false)",
            no_ssh_wrapper = "Skip configuring SSH wrapper (optional, default: false)"
        )
    )]
    pub async fn init(
        &self,
        path: String,
        org: Option<String>,
        forges: Option<Vec<String>>,
        dry_run: Option<bool>,
        force: Option<bool>,
        no_hooks: Option<bool>,
        no_ssh_wrapper: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);
        let is_force = force.unwrap_or(false);
        let is_no_hooks = no_hooks.unwrap_or(false);
        let is_no_ssh_wrapper = no_ssh_wrapper.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            // ── Phase 1: Discover ──
            yield HyperforgeEvent::Info {
                message: format!("{}Discovering workspace...", dry_prefix),
            };

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "  Found {} configured, {} unconfigured, {} non-git dirs. Orgs: [{}], Forges: [{}]",
                    ctx.repos.len(),
                    ctx.unconfigured_repos.len(),
                    ctx.skipped_dirs.len(),
                    ctx.orgs.join(", "),
                    ctx.forges.join(", "),
                ),
            };

            if !ctx.skipped_dirs.is_empty() {
                for dir in &ctx.skipped_dirs {
                    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    yield HyperforgeEvent::Info {
                        message: format!("  {} [no git — needs git init]", name),
                    };
                }
            }

            // ── Infer org and forges ──
            let inferred_org: Option<String> = org.clone().or_else(|| {
                if ctx.orgs.len() == 1 {
                    Some(ctx.orgs[0].clone())
                } else {
                    None
                }
            });

            let inferred_forges: Vec<String> = forges.clone().unwrap_or_else(|| ctx.forges.clone());

            // Determine targets: unconfigured repos, plus configured repos if --force
            let mut targets: Vec<PathBuf> = ctx.unconfigured_repos.clone();
            if is_force {
                for repo in &ctx.repos {
                    targets.push(repo.path.clone());
                }
            }

            if targets.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No repos to initialize.".to_string(),
                };
                yield HyperforgeEvent::WorkspaceSummary {
                    total_repos: ctx.repos.len(),
                    configured_repos: ctx.repos.len(),
                    unconfigured_repos: 0,
                    clean_repos: None,
                    dirty_repos: None,
                    wrong_branch_repos: None,
                    push_success: None,
                    push_failed: None,
                    validation_passed: None,
                };
                return;
            }

            if inferred_org.is_none() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init repos: multiple orgs found and --org not specified.".to_string(),
                };
                return;
            }
            if inferred_forges.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init repos: no forges found and --forges not specified.".to_string(),
                };
                return;
            }

            // ── Phase 2: Init ──
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Initializing {} repos (org={}, forges=[{}])...",
                    dry_prefix,
                    targets.len(),
                    inferred_org.as_deref().unwrap_or("?"),
                    inferred_forges.join(", "),
                ),
            };

            let mut inits_performed = 0usize;
            let mut init_failed = 0usize;

            for repo_path in &targets {
                let dir_name = repo_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?");

                let mut opts = InitOptions::new(inferred_forges.clone());
                if let Some(ref o) = inferred_org {
                    opts = opts.with_org(o.as_str());
                }
                if is_dry_run {
                    opts = opts.dry_run();
                }
                if is_force {
                    opts = opts.force();
                }
                if is_no_hooks {
                    opts = opts.no_hooks();
                }
                if is_no_ssh_wrapper {
                    opts = opts.no_ssh_wrapper();
                }

                match init(repo_path, opts) {
                    Ok(_report) => {
                        inits_performed += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("  {}Initialized {}", dry_prefix, dir_name),
                        };
                    }
                    Err(e) => {
                        init_failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to init {}: {}", dir_name, e),
                        };
                    }
                }
            }

            // ── Phase 3: Re-discover ──
            let ctx = if inits_performed > 0 && !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("Re-discovering after {} inits...", inits_performed),
                };
                match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Re-discovery failed: {}", e),
                        };
                        return;
                    }
                }
            } else {
                ctx
            };

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Init complete: {} initialized, {} failed",
                    dry_prefix, inits_performed, init_failed,
                ),
            };

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: None,
                push_failed: None,
                validation_passed: None,
            };
        }
    }

    /// Check all repos are on expected branch and clean
    #[plexus_macros::hub_method(
        description = "Verify all workspace repos are on the expected branch and have a clean working tree",
        params(
            path = "Path to workspace directory",
            branch = "Expected branch name (optional, default: main)"
        )
    )]
    pub async fn check(
        &self,
        path: String,
        branch: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);
            let expected_branch = branch.unwrap_or_else(|| "main".to_string());

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "Checking {} repos (expected branch: {})",
                    ctx.repos.len(),
                    expected_branch
                ),
            };

            let mut clean_count = 0usize;
            let mut dirty_count = 0usize;
            let mut wrong_branch_count = 0usize;

            // Parallel check: spawn_blocking per repo for git ops
            {
                use tokio::task::JoinSet;
                let concurrency = 8usize;
                let git_repos: Vec<_> = ctx.repos.iter().filter(|r| r.is_git_repo).cloned().collect();

                for chunk in git_repos.chunks(concurrency) {
                    let mut join_set = JoinSet::new();

                    for repo in chunk {
                        let dir_name = repo.dir_name.clone();
                        let path = repo.path.clone();
                        let exp_branch = expected_branch.clone();

                        join_set.spawn(tokio::task::spawn_blocking(move || {
                            let current_branch = Git::current_branch(&path)
                                .map_err(|e| format!("{}: failed to get branch: {}", dir_name, e));
                            let status = Git::repo_status(&path)
                                .map_err(|e| format!("{}: failed to get status: {}", dir_name, e));
                            let ssh_cmd = Git::config_get(&path, "core.sshCommand").ok().flatten();
                            let hf_org = Git::config_get(&path, "hyperforge.org").ok().flatten();
                            (dir_name, path, exp_branch, current_branch, status, ssh_cmd, hf_org)
                        }));
                    }

                    while let Some(result) = join_set.join_next().await {
                        let inner = match result {
                            Ok(inner) => inner,
                            Err(e) => {
                                yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                                continue;
                            }
                        };
                        let (dir_name, path, exp_branch, current_branch, status, ssh_cmd, hf_org) = match inner {
                            Ok(v) => v,
                            Err(e) => {
                                yield HyperforgeEvent::Error { message: format!("Spawn error: {}", e) };
                                continue;
                            }
                        };

                        let current_branch = match current_branch {
                            Ok(b) => b,
                            Err(e) => { yield HyperforgeEvent::Error { message: e }; continue; }
                        };
                        let status = match status {
                            Ok(s) => s,
                            Err(e) => { yield HyperforgeEvent::Error { message: e }; continue; }
                        };

                        let is_clean = !status.has_changes && !status.has_staged && !status.has_untracked;
                        let on_correct_branch = current_branch == exp_branch;

                        if is_clean { clean_count += 1; } else { dirty_count += 1; }
                        if !on_correct_branch { wrong_branch_count += 1; }

                        yield HyperforgeEvent::RepoCheck {
                            repo_name: dir_name.clone(),
                            path: path.display().to_string(),
                            branch: current_branch,
                            expected_branch: exp_branch,
                            is_clean,
                            on_correct_branch,
                        };

                        if ssh_cmd.as_deref() == Some("hyperforge-ssh") && hf_org.is_none() {
                            yield HyperforgeEvent::Error {
                                message: format!("{}: SSH misconfigured — hyperforge-ssh set but hyperforge.org missing", dir_name),
                            };
                        }
                    }
                }
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: Some(clean_count),
                dirty_repos: Some(dirty_count),
                wrong_branch_repos: Some(wrong_branch_count),
                push_success: None,
                push_failed: None,
                validation_passed: None,
            };
        }
    }

    /// Push all repos to their configured forges
    #[plexus_macros::hub_method(
        description = "Push all workspace repos to their configured forges",
        params(
            path = "Path to workspace directory",
            branch = "Branch to push (optional, uses current branch if not specified)",
            dry_run = "Preview pushes without executing (optional, default: false)",
            set_upstream = "Set upstream tracking (optional, default: false)",
            validate = "Run containerized validation before pushing (optional, default: false)"
        )
    )]
    pub async fn push_all(
        &self,
        path: String,
        branch: Option<String>,
        dry_run: Option<bool>,
        set_upstream: Option<bool>,
        validate: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let _ = branch; // push() uses current branch; param is informational
        stream! {
            let workspace_path = PathBuf::from(&path);
            let is_dry_run = dry_run.unwrap_or(false);
            let is_set_upstream = set_upstream.unwrap_or(false);
            let is_validate = validate.unwrap_or(false);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Validation gate (if --validate)
            if is_validate {
                yield HyperforgeEvent::Info {
                    message: "Validation: Running containerized build check...".to_string(),
                };

                let mut dep_nodes = Vec::new();
                let mut dep_all = Vec::new();
                for (idx, repo) in ctx.repos.iter().enumerate() {
                    let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                    dep_nodes.push(crate::build_system::dep_graph::DepNode {
                        name,
                        version: repo.package_version.clone(),
                        build_system: format!("{}", repo.build_system),
                        path: repo.dir_name.clone(),
                    });
                    if !repo.dependencies.is_empty() {
                        dep_all.push((idx, repo.dependencies.clone()));
                    }
                }
                let graph = crate::build_system::dep_graph::DepGraph::build(dep_nodes, &dep_all);
                let plan = crate::build_system::validate::build_validation_plan(&graph, &[], false);
                match plan {
                    Ok(p) => {
                        let results = crate::build_system::validate::execute_validation(&p, &ctx.root, is_dry_run);
                        let summary = crate::build_system::validate::summarize_results(&results);
                        yield HyperforgeEvent::ValidateSummary {
                            total: summary.total,
                            passed: summary.passed,
                            failed: summary.failed,
                            skipped: summary.skipped,
                            duration_ms: summary.duration_ms,
                        };
                        if summary.failed > 0 {
                            yield HyperforgeEvent::Error {
                                message: format!(
                                    "Validation failed ({}/{} steps failed) — aborting push.",
                                    summary.failed, summary.total
                                ),
                            };
                            return;
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Validation plan failed: {} — aborting push.", e),
                        };
                        return;
                    }
                }
            }

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("[DRY RUN] Pushing {} repos...", ctx.repos.len()),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Pushing {} repos...", ctx.repos.len()),
                };
            }

            let mut success_count = 0usize;
            let mut failed_count = 0usize;

            // Parallel push: spawn_blocking per repo
            {
                use tokio::task::JoinSet;
                let concurrency = 8usize;
                let git_repos: Vec<_> = ctx.repos.iter().filter(|r| r.is_git_repo).cloned().collect();
                let non_git: Vec<_> = ctx.repos.iter().filter(|r| !r.is_git_repo).collect();

                for repo in &non_git {
                    yield HyperforgeEvent::Info {
                        message: format!("  Skipping {} (not a git repo)", repo.dir_name),
                    };
                }

                for chunk in git_repos.chunks(concurrency) {
                    let mut join_set = JoinSet::new();

                    for repo in chunk {
                        let dir_name = repo.dir_name.clone();
                        let path = repo.path.clone();
                        let mut options = PushOptions::new();
                        if is_dry_run { options = options.dry_run(); }
                        if is_set_upstream { options = options.set_upstream(); }

                        join_set.spawn(tokio::task::spawn_blocking(move || {
                            let result = push(&path, options);
                            (dir_name, path, result)
                        }));
                    }

                    while let Some(result) = join_set.join_next().await {
                        let inner = match result {
                            Ok(inner) => inner,
                            Err(e) => {
                                yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                                failed_count += 1;
                                continue;
                            }
                        };
                        let (dir_name, path, push_result) = match inner {
                            Ok(v) => v,
                            Err(e) => {
                                yield HyperforgeEvent::Error { message: format!("Spawn error: {}", e) };
                                failed_count += 1;
                                continue;
                            }
                        };

                        match push_result {
                            Ok(report) => {
                                for r in &report.results {
                                    yield HyperforgeEvent::RepoPush {
                                        repo_name: dir_name.clone(),
                                        path: path.display().to_string(),
                                        forge: r.forge.clone(),
                                        success: r.success,
                                        error: r.error.clone(),
                                    };
                                }
                                if report.all_success { success_count += 1; } else { failed_count += 1; }
                            }
                            Err(e) => {
                                yield HyperforgeEvent::RepoPush {
                                    repo_name: dir_name.clone(),
                                    path: path.display().to_string(),
                                    forge: "all".to_string(),
                                    success: false,
                                    error: Some(e.to_string()),
                                };
                                failed_count += 1;
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: Some(success_count),
                push_failed: Some(failed_count),
                validation_passed: None,
            };
        }
    }

    /// Compute sync diff between local and a remote forge
    #[plexus_macros::hub_method(
        description = "Compute diff between local configuration and a remote forge. Use --path to discover from disk, or --org and --forge for direct registry access.",
        params(
            path = "Path to workspace directory (discovers orgs/forges from disk)",
            org = "Organization name (required if --path not provided)",
            forge = "Target forge: github, codeberg, or gitlab (required if --path not provided)"
        )
    )]
    pub async fn diff(
        &self,
        path: Option<String>,
        org: Option<String>,
        forge: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();

        stream! {
            // Resolve org/forge pairs to diff
            let pairs: Vec<(String, String)> = if let Some(ref workspace_path) = path {
                let workspace_path = PathBuf::from(workspace_path);
                let ctx = match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Discovery failed: {}", e),
                        };
                        return;
                    }
                };

                let all_pairs = ctx.org_forge_pairs();

                // Filter by explicit org/forge if provided
                match (&org, &forge) {
                    (Some(o), Some(f)) => all_pairs.into_iter().filter(|(ao, af)| ao == o && af == f).collect(),
                    (Some(o), None) => all_pairs.into_iter().filter(|(ao, _)| ao == o).collect(),
                    _ => all_pairs,
                }
            } else if let (Some(o), Some(f)) = (&org, &forge) {
                vec![(o.clone(), f.clone())]
            } else {
                yield HyperforgeEvent::Error {
                    message: "Must provide --path or both --org and --forge".to_string(),
                };
                return;
            };

            if pairs.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No org/forge pairs found to diff.".to_string(),
                };
                return;
            }

            // Parallel diff: spawn per org/forge pair
            {
                use tokio::task::JoinSet;
                let mut join_set = JoinSet::new();

                for (org_name, forge_name) in &pairs {
                    let state = state.clone();
                    let sync_service = sync_service.clone();
                    let org_name = org_name.clone();
                    let forge_name = forge_name.clone();

                    join_set.spawn(async move {
                        let local = state.get_local_forge(&org_name).await;
                        let ot = local.owner_type();
                        let adapter = match make_adapter(&forge_name, &org_name, ot) {
                            Ok(a) => a,
                            Err(e) => return (org_name, forge_name, Err(e)),
                        };
                        let result = sync_service.diff(local, adapter, &org_name).await
                            .map_err(|e| format!("Diff failed for {}/{}: {}", org_name, forge_name, e));
                        (org_name, forge_name, result)
                    });
                }

                while let Some(result) = join_set.join_next().await {
                    let (org_name, forge_name, diff_result) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                            continue;
                        }
                    };

                    yield HyperforgeEvent::Info {
                        message: format!("Computing diff for {}/{}", org_name, forge_name),
                    };

                    match diff_result {
                        Ok(diff) => {
                            yield HyperforgeEvent::SyncSummary {
                                forge: forge_name.clone(),
                                total: diff.ops.len(),
                                to_create: diff.to_create().len(),
                                to_update: diff.to_update().len(),
                                to_delete: diff.to_delete().len(),
                                in_sync: diff.in_sync().len(),
                            };

                            for op in diff.ops {
                                yield HyperforgeEvent::SyncOp {
                                    repo_name: op.repo.name.clone(),
                                    operation: format!("{:?}", op.op).to_lowercase(),
                                    forge: forge_name.clone(),
                                };
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                        }
                    }
                }
            }
        }
    }

    /// Full safe sync pipeline: discover → init → register → import → diff → apply (no deletes) → push
    #[plexus_macros::hub_method(
        description = "Sync workspace to remote forges. With --reflect, retire remote-only repos. With --purge, delete previously staged repos.",
        params(
            path = "Path to workspace directory (required)",
            org = "Organization name (inferred from workspace if only one org exists)",
            forges = "Forges for unconfigured repos (inferred from existing configs if not specified)",
            dry_run = "Preview all phases without making changes (optional, default: false)",
            no_push = "Skip the git push phase (optional, default: false)",
            no_init = "Skip initializing unconfigured repos (optional, default: false)",
            validate = "Run containerized validation before pushing (optional, default: false)",
            reflect = "Enable reflect mode: retire remote-only repos (optional, default: false)",
            purge = "Delete repos previously staged for deletion. Implies --reflect (optional, default: false)"
        )
    )]
    pub async fn sync(
        &self,
        path: String,
        org: Option<String>,
        forges: Option<Vec<String>>,
        dry_run: Option<bool>,
        no_push: Option<bool>,
        no_init: Option<bool>,
        validate: Option<bool>,
        reflect: Option<bool>,
        purge: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);
        let is_no_push = no_push.unwrap_or(false);
        let is_no_init = no_init.unwrap_or(false);
        let is_validate = validate.unwrap_or(false);
        let is_purge = purge.unwrap_or(false);
        let is_reflect = reflect.unwrap_or(false) || is_purge;

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            // ── Phase 1: Discover ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 1/8: Discovering workspace...", dry_prefix),
            };

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "  Found {} configured, {} unconfigured, {} non-git dirs. Orgs: [{}], Forges: [{}]",
                    ctx.repos.len(),
                    ctx.unconfigured_repos.len(),
                    ctx.skipped_dirs.len(),
                    ctx.orgs.join(", "),
                    ctx.forges.join(", "),
                ),
            };

            // Report non-git directories (no .git, no .hyperforge)
            if !ctx.skipped_dirs.is_empty() {
                for dir in &ctx.skipped_dirs {
                    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    yield HyperforgeEvent::Info {
                        message: format!("  {} [no git — needs git init]", name),
                    };
                }
            }

            // ── Infer org and forges for unconfigured repos ──
            let inferred_org: Option<String> = org.clone().or_else(|| {
                if ctx.orgs.len() == 1 {
                    Some(ctx.orgs[0].clone())
                } else {
                    None
                }
            });

            let inferred_forges: Vec<String> = forges.clone().unwrap_or_else(|| ctx.forges.clone());

            let has_unconfigured = !ctx.unconfigured_repos.is_empty();

            if has_unconfigured && inferred_org.is_none() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init unconfigured repos: multiple orgs found and --org not specified. Skipping init phase.".to_string(),
                };
            }
            if has_unconfigured && inferred_forges.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init unconfigured repos: no forges found and --forges not specified. Skipping init phase.".to_string(),
                };
            }

            // ── Phase 2: Init unconfigured repos ──
            let mut inits_performed = 0usize;

            if !is_no_init && has_unconfigured && inferred_org.is_some() && !inferred_forges.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "{}Phase 2/8: Initializing {} unconfigured repos (org={}, forges=[{}])...",
                        dry_prefix,
                        ctx.unconfigured_repos.len(),
                        inferred_org.as_deref().unwrap_or("?"),
                        inferred_forges.join(", "),
                    ),
                };

                for repo_path in &ctx.unconfigured_repos {
                    let dir_name = repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?");

                    let mut opts = InitOptions::new(inferred_forges.clone());
                    if let Some(ref o) = inferred_org {
                        opts = opts.with_org(o.as_str());
                    }
                    if is_dry_run {
                        opts = opts.dry_run();
                    }

                    match init(repo_path, opts) {
                        Ok(_report) => {
                            inits_performed += 1;
                            yield HyperforgeEvent::Info {
                                message: format!("  {}Initialized {}", dry_prefix, dir_name),
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to init {}: {}", dir_name, e),
                            };
                        }
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 2/8: Init skipped.", dry_prefix),
                };
            }

            // ── Phase 3: Re-discover ──
            let ctx = if inits_performed > 0 && !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 3/8: Re-discovering after {} inits...", dry_prefix, inits_performed),
                };
                match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Re-discovery failed: {}", e),
                        };
                        return;
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 3/8: Re-discover skipped.", dry_prefix),
                };
                ctx
            };

            // ── Phase 4: Register configured repos in LocalForge ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 4/8: Registering {} configured repos in LocalForge...", dry_prefix, ctx.repos.len()),
            };

            let mut registered = 0usize;
            let mut already_registered = 0usize;
            let mut unstaged = 0usize;

            for discovered in &ctx.repos {
                let repo = match repo_from_config(discovered) {
                    Some(r) => r,
                    None => continue,
                };
                let repo_org = match discovered.org() {
                    Some(o) => o.to_string(),
                    None => continue,
                };

                let local = state.get_local_forge(&repo_org).await;

                // Check if already registered
                match local.repo_exists(&repo_org, &repo.name).await {
                    Ok(true) => {
                        // In reflect mode: unstage repos found locally
                        if is_reflect {
                            match local.get_repo(&repo_org, &repo.name).await {
                                Ok(existing) if existing.staged_for_deletion => {
                                    let mut updated = existing.clone();
                                    updated.staged_for_deletion = false;
                                    if let Err(e) = local.update_repo(&repo_org, &updated).await {
                                        yield HyperforgeEvent::Error {
                                            message: format!("  Failed to unstage {}: {}", repo.name, e),
                                        };
                                    } else {
                                        unstaged += 1;
                                        yield HyperforgeEvent::Info {
                                            message: format!("  {}Unstaged {} (found locally)", dry_prefix, repo.name),
                                        };
                                    }
                                }
                                _ => {}
                            }
                        }
                        already_registered += 1;
                        // Mark as managed even if already registered
                        if let Ok(mut record) = local.get_record(&repo.name) {
                            if !record.managed {
                                record.managed = true;
                                let _ = local.update_record(&record);
                            }
                        }
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to check {}: {}", repo.name, e),
                        };
                        continue;
                    }
                }

                // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
                if let Err(e) = local.create_repo(&repo_org, &repo).await {
                    yield HyperforgeEvent::Error {
                        message: format!("  Failed to register {}: {}", repo.name, e),
                    };
                    continue;
                }

                // Mark newly registered repos as managed
                if let Ok(mut record) = local.get_record(&repo.name) {
                    record.managed = true;
                    let _ = local.update_record(&record);
                }

                registered += 1;
                yield HyperforgeEvent::Info {
                    message: format!("  {}Registered {}", dry_prefix, repo.name),
                };
            }

            // Persist to disk only on real runs
            if !is_dry_run && (registered > 0 || unstaged > 0) {
                // Save each org's LocalForge
                for org_name in &ctx.orgs {
                    let local = state.get_local_forge(org_name).await;
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {} newly registered, {} already in LocalForge{}",
                    registered, already_registered,
                    if unstaged > 0 { format!(", {} unstaged", unstaged) } else { String::new() },
                ),
            };

            // ── Phase 5: Import remote-only repos into LocalForge (ETag-based) ──
            let pairs = ctx.org_forge_pairs();

            if !is_reflect {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 5/8: Importing remote-only repos for {} org/forge pairs...", dry_prefix, pairs.len()),
                };

                let mut imported = 0usize;

                for (org_name, forge_name) in &pairs {
                    let local = state.get_local_forge(org_name).await;
                    let ot = local.owner_type();

                    let adapter = match make_adapter(forge_name, org_name, ot) {
                        Ok(a) => a,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                            continue;
                        }
                    };

                    // Get stored ETag for this forge
                    let forge_enum = HyperforgeConfig::parse_forge(forge_name);
                    let stored_etag = if let Some(ref fe) = forge_enum {
                        local.forge_states().ok()
                            .and_then(|states| states.get(fe).map(|s| s.etag.clone()))
                            .flatten()
                    } else {
                        None
                    };

                    // Use incremental list with ETag
                    let list_result = match adapter.list_repos_incremental(org_name, stored_etag).await {
                        Ok(lr) => lr,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to list remote repos for {}/{}: {}", org_name, forge_name, e),
                            };
                            continue;
                        }
                    };

                    if !list_result.modified {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}/{}: not modified (ETag match)", org_name, forge_name),
                        };
                        // Still update last_synced timestamp
                        if let Some(ref fe) = forge_enum {
                            let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                                last_synced: Utc::now(),
                                etag: list_result.etag.clone(),
                            });
                        }
                        continue;
                    }

                    let remote_repos = list_result.repos.as_deref().unwrap_or_default();

                    for remote_repo in remote_repos {
                        match local.repo_exists(org_name, &remote_repo.name).await {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  Failed to check {}: {}", remote_repo.name, e),
                                };
                                continue;
                            }
                        }

                        // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
                        if let Err(e) = local.create_repo(org_name, remote_repo).await {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to import {}: {}", remote_repo.name, e),
                            };
                            continue;
                        }

                        imported += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("  {}Imported {} from {}", dry_prefix, remote_repo.name, forge_name),
                        };
                    }

                    // Update forge sync state with new ETag
                    if let Some(ref fe) = forge_enum {
                        if !is_dry_run {
                            let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                                last_synced: Utc::now(),
                                etag: list_result.etag.clone(),
                            });
                        }
                    }

                    // Persist to disk only on real runs
                    if !is_dry_run && imported > 0 {
                        if let Err(e) = local.save_to_yaml().await {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                            };
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!("  {} repos imported from remotes", imported),
                };

                // ── Phase 5.5: Report unmanaged repos ──
                {
                    for org_name in &ctx.orgs {
                        let local = state.get_local_forge(org_name).await;
                        if let Ok(records) = local.all_records() {
                            let unmanaged: Vec<_> = records.iter()
                                .filter(|r| !r.managed && !r.dismissed)
                                .collect();
                            if !unmanaged.is_empty() {
                                yield HyperforgeEvent::Info {
                                    message: format!("  {} unmanaged repos in LocalForge for org '{}' (not on disk):",
                                        unmanaged.len(), org_name),
                                };
                                for r in &unmanaged {
                                    let forges: Vec<String> = r.present_on.iter()
                                        .map(|f| format!("{:?}", f).to_lowercase())
                                        .collect();
                                    yield HyperforgeEvent::Info {
                                        message: format!("    {} [{}]", r.name, forges.join(", ")),
                                    };
                                }
                            }
                        }
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 5/8: Import skipped (reflect mode).", dry_prefix),
                };
            }

            // ── Phase 6: Diff ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 6/8: Computing diffs...", dry_prefix),
            };

            // Collect diffs for phase 7 (parallel)
            let mut all_diffs: Vec<(String, String, crate::services::SyncDiff)> = Vec::new();

            {
                use tokio::task::JoinSet;
                let mut join_set = JoinSet::new();

                for (org_name, forge_name) in &pairs {
                    let state = state.clone();
                    let sync_service = sync_service.clone();
                    let org_name = org_name.clone();
                    let forge_name = forge_name.clone();

                    join_set.spawn(async move {
                        let local = state.get_local_forge(&org_name).await;
                        let ot = local.owner_type();
                        let adapter = match make_adapter(&forge_name, &org_name, ot) {
                            Ok(a) => a,
                            Err(e) => return (org_name, forge_name, Err(e)),
                        };
                        let result = sync_service.diff(local, adapter, &org_name).await
                            .map_err(|e| format!("Diff failed for {}/{}: {}", org_name, forge_name, e));
                        (org_name, forge_name, result)
                    });
                }

                while let Some(result) = join_set.join_next().await {
                    let (org_name, forge_name, diff_result) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                            continue;
                        }
                    };

                    match diff_result {
                        Ok(diff) => {
                            yield HyperforgeEvent::SyncSummary {
                                forge: forge_name.clone(),
                                total: diff.ops.len(),
                                to_create: diff.to_create().len(),
                                to_update: diff.to_update().len(),
                                to_delete: diff.to_delete().len(),
                                in_sync: diff.in_sync().len(),
                            };

                            if !diff.to_delete().is_empty() {
                                yield HyperforgeEvent::Info {
                                    message: format!(
                                        "  {} repos would be deleted on {} — skipped by sync (use 'workspace apply' to delete)",
                                        diff.to_delete().len(),
                                        forge_name,
                                    ),
                                };
                            }

                            all_diffs.push((org_name, forge_name, diff));
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                        }
                    }
                }
            }

            // ── Phase 7: Apply (safe — no deletes) ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 7/8: Applying creates and updates (no deletes)...", dry_prefix),
            };

            let mut total_created = 0usize;
            let mut total_updated = 0usize;

            // Parallel apply: spawn per org/forge pair, ops within a forge stay sequential
            {
                use tokio::task::JoinSet;
                let mut join_set = JoinSet::new();

                for (org_name, forge_name, diff) in all_diffs {
                    let state = state.clone();

                    join_set.spawn(async move {
                        let ot = state.get_local_forge(&org_name).await.owner_type();
                        let adapter = match make_adapter(&forge_name, &org_name, ot) {
                            Ok(a) => a,
                            Err(e) => return (vec![HyperforgeEvent::Error { message: e }], 0usize, 0usize),
                        };

                        let mut events = Vec::new();
                        let mut created = 0usize;
                        let mut updated = 0usize;

                        for repo_op in &diff.ops {
                            match repo_op.op {
                                SyncOp::Create => {
                                    if !is_dry_run {
                                        if let Err(e) = adapter.create_repo(&org_name, &repo_op.repo).await {
                                            events.push(HyperforgeEvent::Error {
                                                message: format!("  Failed to create {} on {}: {}", repo_op.repo.name, forge_name, e),
                                            });
                                            continue;
                                        }
                                    }
                                    created += 1;
                                    events.push(HyperforgeEvent::SyncOp {
                                        repo_name: repo_op.repo.name.clone(),
                                        operation: "create".to_string(),
                                        forge: forge_name.clone(),
                                    });
                                }
                                SyncOp::Update => {
                                    if !is_dry_run {
                                        if let Err(e) = adapter.update_repo(&org_name, &repo_op.repo).await {
                                            events.push(HyperforgeEvent::Error {
                                                message: format!("  Failed to update {} on {}: {}", repo_op.repo.name, forge_name, e),
                                            });
                                            continue;
                                        }
                                    }
                                    updated += 1;
                                    events.push(HyperforgeEvent::SyncOp {
                                        repo_name: repo_op.repo.name.clone(),
                                        operation: "update".to_string(),
                                        forge: forge_name.clone(),
                                    });
                                }
                                SyncOp::Delete => {
                                    let local = state.get_local_forge(&org_name).await;
                                    let record_info = local.get_record(&repo_op.repo.name).ok();

                                    if record_info.as_ref().map_or(false, |r| r.protected) {
                                        events.push(HyperforgeEvent::SyncOp {
                                            repo_name: repo_op.repo.name.clone(),
                                            operation: "skip_protected".to_string(),
                                            forge: forge_name.clone(),
                                        });
                                        continue;
                                    }

                                    let already_privatized = record_info.as_ref()
                                        .and_then(|rec| {
                                            crate::config::HyperforgeConfig::parse_forge(&forge_name)
                                                .map(|fe| rec.privatized_on.contains(&fe))
                                        })
                                        .unwrap_or(false);

                                    if already_privatized {
                                        events.push(HyperforgeEvent::SyncOp {
                                            repo_name: repo_op.repo.name.clone(),
                                            operation: "already_privatized".to_string(),
                                            forge: forge_name.clone(),
                                        });
                                    } else {
                                        let private_repo = crate::types::Repo::new(
                                            &repo_op.repo.name,
                                            repo_op.repo.origin.clone(),
                                        ).with_visibility(crate::types::Visibility::Private);

                                        if !is_dry_run {
                                            match adapter.update_repo(&org_name, &private_repo).await {
                                                Ok(_) => {
                                                    if let Some(forge_enum) = crate::config::HyperforgeConfig::parse_forge(&forge_name) {
                                                        if let Ok(mut rec) = local.get_record(&repo_op.repo.name) {
                                                            rec.privatized_on.insert(forge_enum);
                                                            let _ = local.update_record(&rec);
                                                            let _ = local.save_to_yaml().await;
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    events.push(HyperforgeEvent::Error {
                                                        message: format!("  Failed to privatize {} on {}: {}",
                                                            repo_op.repo.name, forge_name, e),
                                                    });
                                                }
                                            }
                                        }
                                        events.push(HyperforgeEvent::SyncOp {
                                            repo_name: repo_op.repo.name.clone(),
                                            operation: "privatize".to_string(),
                                            forge: forge_name.clone(),
                                        });
                                    }
                                }
                                SyncOp::InSync => {}
                            }
                        }

                        (events, created, updated)
                    });
                }

                while let Some(result) = join_set.join_next().await {
                    match result {
                        Ok((events, created, updated)) => {
                            total_created += created;
                            total_updated += updated;
                            for event in events {
                                yield event;
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {}{} created, {} updated on remotes",
                    dry_prefix, total_created, total_updated,
                ),
            };

            // ── Phase 7.5: Retire remote-only repos (reflect mode) ──
            let mut staged_count = 0usize;
            let mut purged_count = 0usize;
            let mut protected_skipped = 0usize;

            if is_reflect {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "{}Retire: {}retiring remote-only repos...",
                        dry_prefix,
                        if is_purge { "Purging previously " } else { "Staging and " },
                    ),
                };

                // Build set of local repo names from workspace discovery
                let local_names: HashSet<String> = ctx.repos.iter()
                    .filter_map(|r| repo_from_config(r).map(|repo| repo.name))
                    .collect();

                for (org_name, forge_name) in &pairs {
                    let local = state.get_local_forge(org_name).await;
                    let ot = local.owner_type();

                    let adapter = match make_adapter(forge_name, org_name, ot) {
                        Ok(a) => a,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                            continue;
                        }
                    };

                    // Get stored ETag for this forge
                    let forge_enum = HyperforgeConfig::parse_forge(forge_name);
                    let stored_etag = if let Some(ref fe) = forge_enum {
                        local.forge_states().ok()
                            .and_then(|states| states.get(fe).map(|s| s.etag.clone()))
                            .flatten()
                    } else {
                        None
                    };

                    // Use incremental list with ETag
                    let list_result = match adapter.list_repos_incremental(org_name, stored_etag).await {
                        Ok(lr) => lr,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to list remote repos for {}/{}: {}", org_name, forge_name, e),
                            };
                            continue;
                        }
                    };

                    if !list_result.modified {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}/{}: not modified (ETag match)", org_name, forge_name),
                        };
                        if let Some(ref fe) = forge_enum {
                            let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                                last_synced: Utc::now(),
                                etag: list_result.etag.clone(),
                            });
                        }
                        continue;
                    }

                    let remote_repos = list_result.repos.as_deref().unwrap_or_default();

                    for remote_repo in remote_repos {
                        if local_names.contains(&remote_repo.name) {
                            continue;
                        }

                        // Remote-only repo found
                        if remote_repo.protected {
                            protected_skipped += 1;
                            yield HyperforgeEvent::Info {
                                message: format!(
                                    "  Skipping {} on {} (protected/archived)",
                                    remote_repo.name, forge_name,
                                ),
                            };
                            continue;
                        }

                        if is_purge {
                            // Purge mode: only delete repos that were previously staged
                            let is_staged = match local.get_repo(org_name, &remote_repo.name).await {
                                Ok(existing) => existing.staged_for_deletion,
                                Err(_) => false,
                            };

                            if !is_staged {
                                // Not previously staged — stage it instead
                                let privatized = remote_repo.clone()
                                    .with_visibility(Visibility::Private)
                                    .with_staged_for_deletion(true);

                                if !is_dry_run {
                                    let _ = adapter.update_repo(org_name, &privatized).await;
                                }

                                match local.repo_exists(org_name, &remote_repo.name).await {
                                    Ok(true) => { let _ = local.update_repo(org_name, &privatized).await; }
                                    Ok(false) => { let _ = local.create_repo(org_name, &privatized).await; }
                                    Err(_) => {}
                                }

                                staged_count += 1;
                                yield HyperforgeEvent::SyncOp {
                                    repo_name: remote_repo.name.clone(),
                                    operation: "staged".to_string(),
                                    forge: forge_name.clone(),
                                };
                                continue;
                            }

                            // Check protection before purging
                            let is_protected = local.get_record(&remote_repo.name)
                                .ok()
                                .map_or(false, |r| r.protected);
                            if is_protected {
                                protected_skipped += 1;
                                yield HyperforgeEvent::Info {
                                    message: format!(
                                        "  Skipping purge of {} on {} (protected)",
                                        remote_repo.name, forge_name,
                                    ),
                                };
                                continue;
                            }

                            // Actually delete from remote
                            if !is_dry_run {
                                if let Err(e) = adapter.delete_repo(org_name, &remote_repo.name).await {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Failed to delete {} on {}: {}", remote_repo.name, forge_name, e),
                                    };
                                    continue;
                                }
                                // Soft-delete locally (record preserved)
                                let _ = local.delete_repo(org_name, &remote_repo.name).await;
                                let _ = local.save_to_yaml().await;
                            }

                            purged_count += 1;
                            yield HyperforgeEvent::SyncOp {
                                repo_name: remote_repo.name.clone(),
                                operation: "purged".to_string(),
                                forge: forge_name.clone(),
                            };
                        } else {
                            // Default: stage for deletion (make private + flag)
                            let privatized = remote_repo.clone()
                                .with_visibility(Visibility::Private)
                                .with_staged_for_deletion(true);

                            if !is_dry_run {
                                if let Err(e) = adapter.update_repo(org_name, &privatized).await {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Failed to make {} private on {}: {}", remote_repo.name, forge_name, e),
                                    };
                                    continue;
                                }
                            }

                            match local.repo_exists(org_name, &remote_repo.name).await {
                                Ok(true) => { let _ = local.update_repo(org_name, &privatized).await; }
                                Ok(false) => { let _ = local.create_repo(org_name, &privatized).await; }
                                Err(_) => {}
                            }

                            staged_count += 1;
                            yield HyperforgeEvent::SyncOp {
                                repo_name: remote_repo.name.clone(),
                                operation: "staged".to_string(),
                                forge: forge_name.clone(),
                            };
                        }
                    }

                    // Update forge sync state with new ETag
                    if let Some(ref fe) = forge_enum {
                        if !is_dry_run {
                            let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                                last_synced: Utc::now(),
                                etag: list_result.etag.clone(),
                            });
                        }
                    }

                    // Persist LocalForge changes
                    if !is_dry_run {
                        if let Err(e) = local.save_to_yaml().await {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                            };
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {}{} staged, {} purged, {} protected (skipped)",
                        dry_prefix, staged_count, purged_count, protected_skipped,
                    ),
                };
            }

            // ── Validation gate (if --validate) ──
            let mut validation_passed_result: Option<bool> = None;
            if is_validate {
                yield HyperforgeEvent::Info {
                    message: format!("{}Validation: Running containerized build check...", dry_prefix),
                };

                let mut dep_nodes = Vec::new();
                let mut dep_all = Vec::new();
                for (idx, repo) in ctx.repos.iter().enumerate() {
                    let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                    dep_nodes.push(crate::build_system::dep_graph::DepNode {
                        name,
                        version: repo.package_version.clone(),
                        build_system: format!("{}", repo.build_system),
                        path: repo.dir_name.clone(),
                    });
                    if !repo.dependencies.is_empty() {
                        dep_all.push((idx, repo.dependencies.clone()));
                    }
                }
                let graph = crate::build_system::dep_graph::DepGraph::build(dep_nodes, &dep_all);
                let plan = crate::build_system::validate::build_validation_plan(&graph, &[], false);
                match plan {
                    Ok(p) => {
                        let results = crate::build_system::validate::execute_validation(&p, &ctx.root, is_dry_run);
                        for r in &results {
                            yield HyperforgeEvent::ValidateStep {
                                repo_name: r.repo_name.clone(),
                                step: r.step.clone(),
                                status: format!("{}", r.status),
                                duration_ms: r.duration_ms,
                            };
                        }
                        let summary = crate::build_system::validate::summarize_results(&results);
                        let passed = summary.failed == 0;
                        validation_passed_result = Some(passed);
                        yield HyperforgeEvent::ValidateSummary {
                            total: summary.total,
                            passed: summary.passed,
                            failed: summary.failed,
                            skipped: summary.skipped,
                            duration_ms: summary.duration_ms,
                        };
                        if !passed {
                            yield HyperforgeEvent::Error {
                                message: format!(
                                    "Validation failed ({}/{} steps failed) — aborting push.",
                                    summary.failed, summary.total
                                ),
                            };
                        }
                    }
                    Err(e) => {
                        validation_passed_result = Some(false);
                        yield HyperforgeEvent::Error {
                            message: format!("Validation plan failed: {} — aborting push.", e),
                        };
                    }
                }
            }

            // ── Phase 8: Push git content ──
            let skip_push_for_validation = validation_passed_result == Some(false);
            if is_no_push || skip_push_for_validation {
                if skip_push_for_validation {
                    yield HyperforgeEvent::Info {
                        message: format!("{}Phase 8/8: Push skipped (validation failed).", dry_prefix),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("{}Phase 8/8: Push skipped (--no_push).", dry_prefix),
                    };
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 8/8: Pushing {} repos...", dry_prefix, ctx.repos.len()),
                };

                let mut push_success = 0usize;
                let mut push_failed = 0usize;

                // Parallel push: spawn_blocking per repo
                {
                    use tokio::task::JoinSet;
                    let concurrency = 8usize;
                    let git_repos: Vec<_> = ctx.repos.iter().filter(|r| r.is_git_repo).cloned().collect();

                    for chunk in git_repos.chunks(concurrency) {
                        let mut join_set = JoinSet::new();

                        for repo in chunk {
                            let dir_name = repo.dir_name.clone();
                            let path = repo.path.clone();
                            let mut options = PushOptions::new();
                            if is_dry_run { options = options.dry_run(); }

                            join_set.spawn(tokio::task::spawn_blocking(move || {
                                let result = push(&path, options);
                                (dir_name, path, result)
                            }));
                        }

                        while let Some(result) = join_set.join_next().await {
                            let inner = match result {
                                Ok(inner) => inner,
                                Err(e) => {
                                    yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                                    push_failed += 1;
                                    continue;
                                }
                            };
                            let (dir_name, path, push_result) = match inner {
                                Ok(v) => v,
                                Err(e) => {
                                    yield HyperforgeEvent::Error { message: format!("Spawn error: {}", e) };
                                    push_failed += 1;
                                    continue;
                                }
                            };

                            match push_result {
                                Ok(report) => {
                                    for r in &report.results {
                                        yield HyperforgeEvent::RepoPush {
                                            repo_name: dir_name.clone(),
                                            path: path.display().to_string(),
                                            forge: r.forge.clone(),
                                            success: r.success,
                                            error: r.error.clone(),
                                        };
                                    }
                                    if report.all_success { push_success += 1; } else { push_failed += 1; }
                                }
                                Err(e) => {
                                    yield HyperforgeEvent::RepoPush {
                                        repo_name: dir_name.clone(),
                                        path: path.display().to_string(),
                                        forge: "all".to_string(),
                                        success: false,
                                        error: Some(e.to_string()),
                                    };
                                    push_failed += 1;
                                }
                            }
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {}{} pushed successfully, {} failed",
                        dry_prefix, push_success, push_failed,
                    ),
                };
            }

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!("{}{}pipeline complete.", dry_prefix,
                    if is_reflect { "Reflect " } else { "Sync " }),
            };

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: None,
                push_failed: None,
                validation_passed: validation_passed_result,
            };
        }
    }

    /// Set default branch on all repos in a workspace
    #[plexus_macros::hub_method(
        description = "Set the default branch on all remote forges for every repo in a workspace, and optionally git checkout locally",
        params(
            path = "Path to workspace directory",
            branch = "Branch to set as default",
            checkout = "Also run git checkout locally in each repo (optional, default: false)",
            dry_run = "Preview changes without applying (optional, default: false)"
        )
    )]
    pub async fn set_default_branch(
        &self,
        path: String,
        branch: String,
        checkout: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);
        let is_checkout = checkout.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Setting default branch to '{}' for {} repos...",
                    dry_prefix, branch, ctx.repos.len(),
                ),
            };

            let mut success_count = 0usize;
            let mut error_count = 0usize;

            // Parallel set_default_branch: spawn per repo
            {
                use tokio::task::JoinSet;
                let concurrency = 8usize;

                // Filter repos that have config and org
                let eligible: Vec<_> = ctx.repos.iter()
                    .filter(|r| r.config.is_some() && r.org().is_some())
                    .cloned()
                    .collect();

                // Report repos without org
                for repo in &ctx.repos {
                    if repo.config.is_some() && repo.org().is_none() {
                        yield HyperforgeEvent::Error {
                            message: format!("  {}: no org configured, skipping", repo.dir_name),
                        };
                        error_count += 1;
                    }
                }

                for chunk in eligible.chunks(concurrency) {
                    let mut join_set = JoinSet::new();

                    for repo in chunk {
                        let config = repo.config.clone().unwrap();
                        let org = repo.org().unwrap().to_string();
                        let repo_name = config.repo_name.clone()
                            .unwrap_or_else(|| repo.dir_name.clone());
                        let dir_name = repo.dir_name.clone();
                        let forges: Vec<String> = repo.forges().into_iter().map(|s| s.to_string()).collect();
                        let branch = branch.clone();
                        let dry_prefix = dry_prefix.to_string();
                        let path = repo.path.clone();
                        let is_git = repo.is_git_repo;

                        join_set.spawn(async move {
                            let mut events = Vec::new();
                            let mut errors = Vec::new();

                            for forge_name in &forges {
                                if is_dry_run {
                                    events.push(HyperforgeEvent::Info {
                                        message: format!(
                                            "  {}Would set default branch on {}/{} ({})",
                                            dry_prefix, repo_name, forge_name, branch,
                                        ),
                                    });
                                    continue;
                                }

                                let adapter = match make_adapter(forge_name, &org, None) {
                                    Ok(a) => a,
                                    Err(e) => {
                                        errors.push(format!("{}: {}", forge_name, e));
                                        continue;
                                    }
                                };

                                match adapter.set_default_branch(&org, &repo_name, &branch).await {
                                    Ok(_) => {
                                        events.push(HyperforgeEvent::Info {
                                            message: format!("  {} → {} default branch set to '{}'", repo_name, forge_name, branch),
                                        });
                                    }
                                    Err(e) => {
                                        errors.push(format!("{}: {}", forge_name, e));
                                    }
                                }
                            }

                            // Optionally checkout locally (sync git op)
                            if is_checkout && is_git {
                                if is_dry_run {
                                    events.push(HyperforgeEvent::Info {
                                        message: format!("  {}Would checkout '{}' in {}", dry_prefix, branch, dir_name),
                                    });
                                } else {
                                    match tokio::task::spawn_blocking({
                                        let path = path.clone();
                                        let branch = branch.clone();
                                        move || Git::checkout(&path, &branch)
                                    }).await {
                                        Ok(Ok(_)) => {
                                            events.push(HyperforgeEvent::Info {
                                                message: format!("  {} → checked out '{}'", dir_name, branch),
                                            });
                                        }
                                        Ok(Err(e)) => {
                                            errors.push(format!("checkout: {}", e));
                                        }
                                        Err(e) => {
                                            errors.push(format!("checkout spawn error: {}", e));
                                        }
                                    }
                                }
                            }

                            (dir_name, events, errors)
                        });
                    }

                    while let Some(result) = join_set.join_next().await {
                        let (dir_name, events, errors) = match result {
                            Ok(v) => v,
                            Err(e) => {
                                yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                                error_count += 1;
                                continue;
                            }
                        };

                        for event in events {
                            yield event;
                        }

                        if errors.is_empty() {
                            success_count += 1;
                        } else {
                            error_count += 1;
                            for err in &errors {
                                yield HyperforgeEvent::Error {
                                    message: format!("  {} error: {}", dir_name, err),
                                };
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Set default branch complete: {} succeeded, {} failed",
                    dry_prefix, success_count, error_count,
                ),
            };
        }
    }

    /// Verify workspace sync state
    #[plexus_macros::hub_method(
        description = "Verify workspace configuration including orgs, SSH keys, and auth tokens. Use --path to discover from disk, or --org for registry access.",
        params(
            path = "Path to workspace directory (discovers orgs from disk)",
            org = "Organization to verify (optional, verifies all if not specified)"
        )
    )]
    pub async fn verify(
        &self,
        path: Option<String>,
        org: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let config_dir = self.state.config_dir.clone();

        stream! {
            yield HyperforgeEvent::Info {
                message: "Starting workspace verification...".to_string(),
            };

            // Determine orgs to check
            let orgs_to_check: Vec<String> = if let Some(ref workspace_path) = path {
                let workspace_path = PathBuf::from(workspace_path);
                let ctx = match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Discovery failed: {}", e),
                        };
                        return;
                    }
                };

                // If org specified, filter; otherwise use all discovered orgs
                if let Some(ref org_name) = org {
                    if ctx.orgs.contains(org_name) {
                        vec![org_name.clone()]
                    } else {
                        yield HyperforgeEvent::Error {
                            message: format!("Org '{}' not found in workspace", org_name),
                        };
                        return;
                    }
                } else {
                    ctx.orgs.clone()
                }
            } else if let Some(org_name) = org {
                vec![org_name]
            } else {
                // Legacy: list all orgs from config dir
                let orgs_path = config_dir.join("orgs");
                match tokio::fs::read_dir(&orgs_path).await {
                    Ok(mut entries) => {
                        let mut orgs = Vec::new();
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Some(name) = entry.file_name().to_str() {
                                if name != "." && name != ".." {
                                    orgs.push(name.to_string());
                                }
                            }
                        }
                        orgs
                    }
                    Err(_) => {
                        yield HyperforgeEvent::Error {
                            message: "No organizations configured. Provide --path or --org.".to_string(),
                        };
                        return;
                    }
                }
            };

            let mut total_repos = 0;
            let mut total_issues = 0;

            // Verify each org
            for org_name in orgs_to_check {
                yield HyperforgeEvent::Info {
                    message: format!("Verifying org: {}", org_name),
                };

                // Check org repos.yaml exists
                let repos_yaml = config_dir.join("orgs").join(&org_name).join("repos.yaml");
                if !repos_yaml.exists() {
                    yield HyperforgeEvent::Error {
                        message: format!("  ✗ Missing repos.yaml for org: {}", org_name),
                    };
                    total_issues += 1;
                    continue;
                }

                // Load and count repos
                let local_forge = state.get_local_forge(&org_name).await;
                match local_forge.all_repos() {
                    Ok(repos) => {
                        let repo_count = repos.len();
                        total_repos += repo_count;

                        yield HyperforgeEvent::Info {
                            message: format!("  ✓ Found {} repos in {}", repo_count, org_name),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  ✗ Failed to load repos: {}", e),
                        };
                        total_issues += 1;
                    }
                }

                // Check auth tokens for common forges
                for forge in &["github", "codeberg", "gitlab"] {
                    let _token_key = format!("{}/{}/token", forge, org_name);
                    yield HyperforgeEvent::Info {
                        message: format!("  ℹ Auth check for {}/{} (use auth hub to verify)", forge, org_name),
                    };
                }
            }

            // Check SSH keys
            let ssh_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".ssh");

            if ssh_dir.exists() {
                let ssh_keys = vec!["hyperforge_ed25519", "id_ed25519", "id_rsa"];
                let mut found_keys = Vec::new();

                for key_name in ssh_keys {
                    let key_path = ssh_dir.join(key_name);
                    if key_path.exists() {
                        found_keys.push(key_name);
                    }
                }

                if found_keys.is_empty() {
                    yield HyperforgeEvent::Error {
                        message: "✗ No SSH keys found in ~/.ssh/".to_string(),
                    };
                    total_issues += 1;
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("✓ Found SSH keys: {}", found_keys.join(", ")),
                    };
                }
            } else {
                yield HyperforgeEvent::Error {
                    message: "✗ ~/.ssh/ directory not found".to_string(),
                };
                total_issues += 1;
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: "=== Verification Summary ===".to_string(),
            };
            yield HyperforgeEvent::Info {
                message: format!("Total repositories: {}", total_repos),
            };
            yield HyperforgeEvent::Info {
                message: format!("Issues found: {}", total_issues),
            };

            if total_issues == 0 {
                yield HyperforgeEvent::Info {
                    message: "✓ Workspace configuration verified successfully!".to_string(),
                };
            } else {
                yield HyperforgeEvent::Error {
                    message: format!("✗ Found {} issues that need attention", total_issues),
                };
            }
        }
    }

    /// Generate/update native workspace manifests (Cargo.toml, cabal.project)
    #[plexus_macros::hub_method(
        description = "Generate workspace config files (.cargo/config.toml with [patch.crates-io], cabal.project) from detected build systems. Each repo stays independent while sibling crates resolve locally.",
        params(
            path = "Path to workspace directory",
            dry_run = "Preview without writing files (optional, default: false)"
        )
    )]
    pub async fn unify(
        &self,
        path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            // Discover workspace
            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("{}Workspace unify: {} repos discovered", dry_prefix, ctx.repos.len()),
            };

            // Collect Rust crates
            let rust_repos = ctx.repos_for_build_system(&crate::build_system::BuildSystemKind::Cargo);
            if !rust_repos.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("  Found {} Rust crates", rust_repos.len()),
                };

                let crates: Vec<crate::build_system::cargo_config::CrateInfo> = rust_repos
                    .iter()
                    .filter_map(|repo| {
                        let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                        let version = repo.package_version.clone().unwrap_or_else(|| "0.0.0".to_string());
                        let rel_path = repo.dir_name.clone();
                        Some(crate::build_system::cargo_config::CrateInfo {
                            name,
                            version,
                            path: rel_path,
                            dependencies: repo.dependencies.clone(),
                        })
                    })
                    .collect();

                match crate::build_system::cargo_config::generate_cargo_config(
                    &ctx.root,
                    &crates,
                    is_dry_run,
                ) {
                    Ok(report) => {
                        let action_str = match report.action {
                            crate::build_system::cargo_config::FileAction::Created => "created",
                            crate::build_system::cargo_config::FileAction::Updated => "updated",
                            crate::build_system::cargo_config::FileAction::Unchanged => "unchanged",
                            crate::build_system::cargo_config::FileAction::Removed => "removed",
                        };

                        yield HyperforgeEvent::UnifyResult {
                            language: "rust".to_string(),
                            file_path: ctx.root.join(".cargo/config.toml").to_string_lossy().to_string(),
                            action: action_str.to_string(),
                        };

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{}.cargo/config.toml: {} patches [{}]",
                                dry_prefix,
                                report.patches.len(),
                                action_str
                            ),
                        };

                        if !report.patches.is_empty() {
                            for (name, path) in &report.patches {
                                yield HyperforgeEvent::Info {
                                    message: format!("  patch: {} -> {}", name, path),
                                };
                            }
                        }

                        for (desc, cleanup_action) in &report.cleanup {
                            let cleanup_str = match cleanup_action {
                                crate::build_system::cargo_config::FileAction::Removed => "removed",
                                crate::build_system::cargo_config::FileAction::Updated => "updated",
                                crate::build_system::cargo_config::FileAction::Created => "created",
                                crate::build_system::cargo_config::FileAction::Unchanged => "unchanged",
                            };
                            yield HyperforgeEvent::UnifyResult {
                                language: "rust".to_string(),
                                file_path: desc.clone(),
                                action: cleanup_str.to_string(),
                            };
                            yield HyperforgeEvent::Info {
                                message: format!("  {}{} [{}]", dry_prefix, desc, cleanup_str),
                            };
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to generate .cargo/config.toml: {}", e),
                        };
                    }
                }
            }

            // Collect Haskell packages
            let cabal_repos = ctx.repos_for_build_system(&crate::build_system::BuildSystemKind::Cabal);
            if !cabal_repos.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("  Found {} Haskell packages", cabal_repos.len()),
                };

                let packages: Vec<crate::build_system::cabal_project::CabalPackageInfo> = cabal_repos
                    .iter()
                    .map(|repo| crate::build_system::cabal_project::CabalPackageInfo {
                        name: repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone()),
                        path: repo.dir_name.clone(),
                    })
                    .collect();

                match crate::build_system::cabal_project::generate_cabal_project(
                    &ctx.root,
                    &packages,
                    is_dry_run,
                ) {
                    Ok(report) => {
                        let action_str = match report.action {
                            crate::build_system::cabal_project::FileAction::Created => "created",
                            crate::build_system::cabal_project::FileAction::Updated => "updated",
                            crate::build_system::cabal_project::FileAction::Unchanged => "unchanged",
                        };

                        yield HyperforgeEvent::UnifyResult {
                            language: "haskell".to_string(),
                            file_path: ctx.root.join("cabal.project").to_string_lossy().to_string(),
                            action: action_str.to_string(),
                        };

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{}cabal.project: {} packages [{}]",
                                dry_prefix,
                                report.packages.len(),
                                action_str
                            ),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to generate cabal.project: {}", e),
                        };
                    }
                }
            }

            if rust_repos.is_empty() && cabal_repos.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No Rust or Haskell projects found — nothing to unify.".to_string(),
                };
            }
        }
    }

    /// Analyze workspace dependency graph and detect version mismatches
    #[plexus_macros::hub_method(
        description = "Analyze workspace dependency graph: show build tiers, dependency relationships, and version mismatches between pinned and local versions.",
        params(
            path = "Path to workspace directory",
            format = "Output format: 'summary' (default), 'graph', or 'mismatches'"
        )
    )]
    pub async fn analyze(
        &self,
        path: String,
        format: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let output_format = format.unwrap_or_else(|| "summary".to_string());

        stream! {
            let workspace_path = PathBuf::from(&path);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Build dep graph from all discovered repos
            let mut nodes = Vec::new();
            let mut all_deps = Vec::new();

            for (idx, repo) in ctx.repos.iter().enumerate() {
                let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                let version = repo.package_version.clone();

                nodes.push(crate::build_system::dep_graph::DepNode {
                    name,
                    version,
                    build_system: format!("{}", repo.build_system),
                    path: repo.dir_name.clone(),
                });

                if !repo.dependencies.is_empty() {
                    all_deps.push((idx, repo.dependencies.clone()));
                }
            }

            if nodes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace.".to_string(),
                };
                return;
            }

            let graph = crate::build_system::dep_graph::DepGraph::build(nodes, &all_deps);

            match output_format.as_str() {
                "summary" => {
                    // Build tiers
                    match graph.build_tiers() {
                        Ok(tiers) => {
                            yield HyperforgeEvent::Info {
                                message: format!(
                                    "Workspace: {} packages, {} internal deps, {} build tiers",
                                    graph.nodes.len(),
                                    graph.edges.len(),
                                    tiers.len()
                                ),
                            };

                            for (tier_idx, tier) in tiers.iter().enumerate() {
                                let names: Vec<&str> = tier
                                    .iter()
                                    .map(|&i| graph.nodes[i].name.as_str())
                                    .collect();
                                yield HyperforgeEvent::Info {
                                    message: format!("  Tier {}: {}", tier_idx, names.join(", ")),
                                };
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Cycle detected: {}", e),
                            };
                        }
                    }

                    // Show mismatches summary
                    let mismatches = graph.version_mismatches();
                    if !mismatches.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: format!("\n{} version mismatches:", mismatches.len()),
                        };
                        for m in &mismatches {
                            yield HyperforgeEvent::DepMismatch {
                                repo: m.repo_name.clone(),
                                dependency: m.dependency.clone(),
                                pinned_version: m.pinned_version.clone(),
                                local_version: m.local_version.clone(),
                            };
                        }
                    } else {
                        yield HyperforgeEvent::Info {
                            message: "No version mismatches detected.".to_string(),
                        };
                    }
                }

                "graph" => {
                    for (i, node) in graph.nodes.iter().enumerate() {
                        let deps = graph.direct_deps(i);
                        let rdeps = graph.reverse_deps(i);

                        let dep_names: Vec<&str> = deps
                            .iter()
                            .map(|&j| graph.nodes[j].name.as_str())
                            .collect();
                        let rdep_names: Vec<&str> = rdeps
                            .iter()
                            .map(|&j| graph.nodes[j].name.as_str())
                            .collect();

                        let version_str = node
                            .version
                            .as_deref()
                            .unwrap_or("?");

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{} v{} [{}] deps=[{}] rdeps=[{}]",
                                node.name,
                                version_str,
                                node.build_system,
                                dep_names.join(", "),
                                rdep_names.join(", ")
                            ),
                        };
                    }
                }

                "mismatches" => {
                    let mismatches = graph.version_mismatches();
                    if mismatches.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: "No version mismatches detected.".to_string(),
                        };
                    } else {
                        yield HyperforgeEvent::Info {
                            message: format!("{} version mismatches:", mismatches.len()),
                        };
                        for m in &mismatches {
                            yield HyperforgeEvent::DepMismatch {
                                repo: m.repo_name.clone(),
                                dependency: m.dependency.clone(),
                                pinned_version: m.pinned_version.clone(),
                                local_version: m.local_version.clone(),
                            };
                        }
                    }
                }

                other => {
                    yield HyperforgeEvent::Error {
                        message: format!(
                            "Unknown format '{}'. Valid: summary, graph, mismatches",
                            other
                        ),
                    };
                }
            }
        }
    }

    /// Validate workspace builds in Docker containers
    #[plexus_macros::hub_method(
        description = "Run containerized builds and tests in dependency order. Uses Docker to validate the entire workspace compiles before pushing.",
        params(
            path = "Path to workspace directory",
            test = "Also run tests after builds (optional, default: false)",
            dry_run = "Preview validation plan without running Docker (optional, default: false)",
            image = "Docker image to use (optional, default: rust:latest)"
        )
    )]
    pub async fn validate(
        &self,
        path: String,
        test: Option<bool>,
        dry_run: Option<bool>,
        image: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);
        let run_tests = test.unwrap_or(false);
        let docker_image = image.unwrap_or_else(|| "rust:latest".to_string());

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Build dep graph
            let mut nodes = Vec::new();
            let mut all_deps = Vec::new();

            for (idx, repo) in ctx.repos.iter().enumerate() {
                let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                let version = repo.package_version.clone();

                nodes.push(crate::build_system::dep_graph::DepNode {
                    name,
                    version,
                    build_system: format!("{}", repo.build_system),
                    path: repo.dir_name.clone(),
                });

                if !repo.dependencies.is_empty() {
                    all_deps.push((idx, repo.dependencies.clone()));
                }
            }

            let graph = crate::build_system::dep_graph::DepGraph::build(nodes, &all_deps);

            // Build CI configs from per-repo .hyperforge/config.toml [ci] sections
            let ci_configs: Vec<(String, crate::build_system::validate::RepoCiConfig)> = ctx
                .repos
                .iter()
                .filter_map(|repo| {
                    let config = repo.config.as_ref()?;
                    let ci = config.ci.as_ref()?;
                    let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());

                    let mut cfg = crate::build_system::validate::RepoCiConfig::default();
                    cfg.repo_name = name.clone();
                    if !ci.build.is_empty() {
                        cfg.build_command = ci.build.clone();
                    }
                    if !ci.test.is_empty() {
                        cfg.test_command = ci.test.clone();
                    }
                    cfg.dockerfile = ci.dockerfile.clone();
                    cfg.skip = ci.skip_validate;
                    cfg.timeout_secs = ci.timeout_secs;
                    cfg.env = ci.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                    Some((name, cfg))
                })
                .collect();

            // Build validation plan
            let plan = match crate::build_system::validate::build_validation_plan(
                &graph,
                &ci_configs,
                run_tests,
            ) {
                Ok(mut p) => {
                    p.default_image = docker_image;
                    p
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to build validation plan: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Validation plan: {} steps, tests={}",
                    dry_prefix,
                    plan.steps.len(),
                    run_tests
                ),
            };

            // Execute validation
            let results = crate::build_system::validate::execute_validation(
                &plan,
                &ctx.root,
                is_dry_run,
            );

            for result in &results {
                yield HyperforgeEvent::ValidateStep {
                    repo_name: result.repo_name.clone(),
                    step: result.step.clone(),
                    status: format!("{}", result.status),
                    duration_ms: result.duration_ms,
                };
            }

            let summary = crate::build_system::validate::summarize_results(&results);
            yield HyperforgeEvent::ValidateSummary {
                total: summary.total,
                passed: summary.passed,
                failed: summary.failed,
                skipped: summary.skipped,
                duration_ms: summary.duration_ms,
            };

            if summary.failed > 0 {
                yield HyperforgeEvent::Error {
                    message: format!(
                        "Validation failed: {}/{} steps failed",
                        summary.failed, summary.total
                    ),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "{}Validation passed: {}/{} steps succeeded",
                        dry_prefix, summary.passed, summary.total
                    ),
                };
            }
        }
    }

    /// Run a command across all workspace repos
    #[plexus_macros::hub_method(
        description = "Execute an arbitrary shell command in every workspace repo directory. Runs in parallel by default.",
        params(
            path = "Path to workspace directory",
            command = "Shell command to execute in each repo",
            filter = "Glob pattern to filter repos by name (optional)",
            sequential = "Run sequentially instead of in parallel (optional, default: false)",
            dirty = "Only run on repos with uncommitted changes (optional, default: false)"
        )
    )]
    pub async fn exec(
        &self,
        path: String,
        command: String,
        filter: Option<String>,
        sequential: Option<bool>,
        dirty: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_sequential = sequential.unwrap_or(false);
        let only_dirty = dirty.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Filter repos by name glob if provided
            let mut repos: Vec<&crate::commands::workspace::DiscoveredRepo> = if let Some(ref pattern) = filter {
                ctx.repos.iter().filter(|r| {
                    glob_match(pattern, &r.dir_name)
                }).collect()
            } else {
                ctx.repos.iter().collect()
            };

            // Filter to dirty repos only
            if only_dirty {
                repos.retain(|r| {
                    match Git::repo_status(&r.path) {
                        Ok(s) => s.has_changes || s.has_staged || s.has_untracked,
                        Err(_) => false,
                    }
                });
            }

            if repos.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No repos matched filter.".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Executing `{}` across {} repos{}...",
                    command,
                    repos.len(),
                    if is_sequential { " (sequential)" } else { " (parallel)" }
                ),
            };

            if is_sequential {
                for repo in &repos {
                    let output = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .current_dir(&repo.path)
                        .output()
                        .await;

                    match output {
                        Ok(output) => {
                            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            let exit_code = output.status.code().unwrap_or(-1);

                            yield HyperforgeEvent::ExecResult {
                                repo_name: repo.dir_name.clone(),
                                exit_code,
                                stdout,
                                stderr,
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::ExecResult {
                                repo_name: repo.dir_name.clone(),
                                exit_code: -1,
                                stdout: String::new(),
                                stderr: format!("Failed to execute: {}", e),
                            };
                        }
                    }
                }
            } else {
                // Parallel execution
                use tokio::task::JoinSet;

                let mut join_set = JoinSet::new();

                for repo in &repos {
                    let repo_name = repo.dir_name.clone();
                    let repo_path = repo.path.clone();
                    let cmd = command.clone();

                    join_set.spawn(async move {
                        let output = tokio::process::Command::new("sh")
                            .arg("-c")
                            .arg(&cmd)
                            .current_dir(&repo_path)
                            .output()
                            .await;

                        match output {
                            Ok(output) => {
                                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                                let exit_code = output.status.code().unwrap_or(-1);
                                (repo_name, exit_code, stdout, stderr)
                            }
                            Err(e) => {
                                (repo_name, -1, String::new(), format!("Failed to execute: {}", e))
                            }
                        }
                    });
                }

                while let Some(result) = join_set.join_next().await {
                    match result {
                        Ok((repo_name, exit_code, stdout, stderr)) => {
                            yield HyperforgeEvent::ExecResult {
                                repo_name,
                                exit_code,
                                stdout,
                                stderr,
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Task join error: {}", e),
                            };
                        }
                    }
                }
            }

            // Summary
            let total = repos.len();
            yield HyperforgeEvent::Info {
                message: format!("Exec complete: ran across {} repos", total),
            };
        }
    }

    /// Clone all repos for an org from LocalForge into a workspace directory
    #[plexus_macros::hub_method(
        description = "Clone all repos for an org from LocalForge into a workspace directory. Skips repos already on disk.",
        params(
            org = "Organization name (must have repos in LocalForge)",
            path = "Target workspace directory",
            filter = "Filter repos by name glob (optional, e.g. 'plexus-*')",
            forge = "Preferred forge to clone from (optional, defaults to first in present_on)",
            concurrency = "Max parallel clones (optional, default: 4)"
        )
    )]
    pub async fn clone(
        &self,
        org: String,
        path: String,
        filter: Option<String>,
        forge: Option<String>,
        concurrency: Option<u32>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let max_concurrent = concurrency.unwrap_or(4) as usize;

        stream! {
            let workspace_path = PathBuf::from(&path);

            // 1. Load LocalForge
            let local = state.get_local_forge(&org).await;

            // 2. Get all records
            let records = match local.all_records() {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to load repos: {}", e),
                    };
                    return;
                }
            };

            if records.is_empty() {
                yield HyperforgeEvent::Error {
                    message: format!("No repos found in LocalForge for org '{}'", org),
                };
                return;
            }

            // 3. Filter by glob
            let filtered: Vec<_> = if let Some(ref pattern) = filter {
                records.into_iter().filter(|r| glob_match(pattern, &r.name)).collect()
            } else {
                records
            };

            // 4. Create workspace dir if needed
            if let Err(e) = std::fs::create_dir_all(&workspace_path) {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create workspace directory: {}", e),
                };
                return;
            }

            // 5. Skip repos already on disk
            let total_filtered = filtered.len();
            let to_clone: Vec<_> = filtered.into_iter().filter(|r| {
                !workspace_path.join(&r.name).exists()
            }).collect();

            let skipped_count = total_filtered - to_clone.len();

            if to_clone.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "All {} repos already exist on disk. Nothing to clone.",
                        total_filtered,
                    ),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Cloning {} repos (skipping {} already on disk, concurrency: {})...",
                    to_clone.len(), skipped_count, max_concurrent,
                ),
            };

            // Validate forge preference if provided
            if let Some(ref f) = forge {
                if HyperforgeConfig::parse_forge(f).is_none() {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", f),
                    };
                    return;
                }
            }

            // 6. Clone in batches using JoinSet for concurrency control
            let mut success_count = 0usize;
            let mut failed_count = 0usize;

            for chunk in to_clone.chunks(max_concurrent) {
                use tokio::task::JoinSet;
                let mut join_set = JoinSet::new();

                for record in chunk {
                    let record = record.clone();
                    let org = org.clone();
                    let forge_pref = forge.clone();
                    let ws_path = workspace_path.clone();

                    join_set.spawn(async move {
                        // Pick forge to clone from
                        let clone_forge = if let Some(ref f) = forge_pref {
                            f.to_lowercase()
                        } else {
                            match record.present_on.iter().next() {
                                Some(f) => format!("{:?}", f).to_lowercase(),
                                None => {
                                    return (record.name.clone(), Err("No forges in present_on".to_string()));
                                }
                            }
                        };

                        let clone_url = crate::git::build_remote_url(&clone_forge, &org, &record.name);
                        let target = ws_path.join(&record.name);
                        let target_str = target.display().to_string();

                        // Clone
                        if let Err(e) = crate::git::Git::clone(&clone_url, &target_str) {
                            return (record.name.clone(), Err(format!("Clone failed: {}", e)));
                        }

                        // Generate .hyperforge/config.toml if missing
                        if !crate::config::HyperforgeConfig::exists(&target) {
                            let forges: Vec<String> = record.present_on.iter()
                                .map(|f| format!("{:?}", f).to_lowercase())
                                .collect();
                            let mut config = crate::config::HyperforgeConfig::new(forges)
                                .with_org(&org)
                                .with_repo_name(&record.name)
                                .with_visibility(record.visibility.clone());
                            if let Some(ref desc) = record.description {
                                config = config.with_description(desc);
                            }
                            let _ = config.save(&target);
                        }

                        // Add remotes for other forges in present_on
                        let clone_forge_parsed = crate::config::HyperforgeConfig::parse_forge(&clone_forge);
                        for f in &record.present_on {
                            if Some(f.clone()) == clone_forge_parsed {
                                continue; // Already set as "origin" by git clone
                            }
                            let f_str = format!("{:?}", f).to_lowercase();
                            let remote_url = crate::git::build_remote_url(&f_str, &org, &record.name);
                            let _ = crate::git::Git::add_remote(&target, &f_str, &remote_url);
                        }

                        (record.name.clone(), Ok(()))
                    });
                }

                // Collect results from this batch
                while let Some(result) = join_set.join_next().await {
                    match result {
                        Ok((name, Ok(()))) => {
                            success_count += 1;
                            yield HyperforgeEvent::Info {
                                message: format!("  Cloned: {}", name),
                            };
                        }
                        Ok((name, Err(e))) => {
                            failed_count += 1;
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed: {} — {}", name, e),
                            };
                        }
                        Err(e) => {
                            failed_count += 1;
                            yield HyperforgeEvent::Error {
                                message: format!("  Task join error: {}", e),
                            };
                        }
                    }
                }
            }

            // Summary
            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: success_count + failed_count + skipped_count,
                configured_repos: success_count,
                unconfigured_repos: 0,
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: Some(success_count),
                push_failed: Some(failed_count),
                validation_passed: None,
            };
        }
    }

    /// Compare local package versions against their registries
    #[plexus_macros::hub_method(
        description = "Show local vs published versions for workspace packages",
        params(
            path = "Path to workspace root directory",
            filter = "Glob pattern to filter packages by name (optional)"
        )
    )]
    pub async fn package_diff(
        &self,
        path: String,
        filter: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Build dep graph
            let mut nodes = Vec::new();
            let mut all_deps = Vec::new();

            for (idx, repo) in ctx.repos.iter().enumerate() {
                let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                let version = repo.package_version.clone();

                nodes.push(crate::build_system::dep_graph::DepNode {
                    name,
                    version,
                    build_system: format!("{}", repo.build_system),
                    path: repo.dir_name.clone(),
                });

                // Exclude dev-deps for publish graph
                let non_dev_deps: Vec<_> = repo.dependencies.iter()
                    .filter(|d| !d.is_dev)
                    .cloned()
                    .collect();
                if !non_dev_deps.is_empty() {
                    all_deps.push((idx, non_dev_deps));
                }
            }

            if nodes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace.".to_string(),
                };
                return;
            }

            let graph = crate::build_system::dep_graph::DepGraph::build(nodes, &all_deps);

            // Filter packages
            let indices: Vec<usize> = graph.nodes.iter().enumerate()
                .filter(|(_, node)| {
                    if let Some(ref pat) = filter {
                        glob_match(pat, &node.name)
                    } else {
                        true
                    }
                })
                .map(|(i, _)| i)
                .collect();

            if indices.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages matched filter.".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Checking {} packages against registries...", indices.len()),
            };

            // Query registries for each package
            for &idx in &indices {
                let node = &graph.nodes[idx];
                let build_system = match node.build_system.as_str() {
                    "cargo" => crate::build_system::BuildSystemKind::Cargo,
                    "cabal" => crate::build_system::BuildSystemKind::Cabal,
                    "node" => crate::build_system::BuildSystemKind::Node,
                    _ => crate::build_system::BuildSystemKind::Unknown,
                };

                let registry = match crate::package::registry_for(&build_system) {
                    Some(r) => r,
                    None => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: skipped (no registry for {})", node.name, node.build_system),
                        };
                        continue;
                    }
                };

                let local_version = match &node.version {
                    Some(v) => v.clone(),
                    None => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: skipped (no version)", node.name),
                        };
                        continue;
                    }
                };

                let registry_kind = registry.registry_kind();

                let published = match registry.published_version(&node.name).await {
                    Ok(pv) => pv,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  {}: registry query failed: {}", node.name, e),
                        };
                        continue;
                    }
                };

                let published_version = published.as_ref().map(|p| p.version.clone());

                let status = match &published_version {
                    None => crate::hub::PackageStatus::Unpublished,
                    Some(pub_v) => {
                        match crate::build_system::version::compare_versions(&local_version, pub_v) {
                            Some(std::cmp::Ordering::Greater) => crate::hub::PackageStatus::Ahead,
                            Some(std::cmp::Ordering::Equal) => crate::hub::PackageStatus::UpToDate,
                            Some(std::cmp::Ordering::Less) => crate::hub::PackageStatus::Stale,
                            None => crate::hub::PackageStatus::Stale,
                        }
                    }
                };

                yield HyperforgeEvent::PackageDiff {
                    package_name: node.name.clone(),
                    build_system: build_system.clone(),
                    local_version,
                    published_version,
                    registry: registry_kind,
                    status,
                };
            }
        }
    }

    /// Publish packages with transitive dependency resolution
    #[plexus_macros::hub_method(
        description = "Publish workspace packages in dependency order, auto-publishing transitive deps first. Dry-run by default — pass --execute to actually publish.",
        params(
            path = "Path to workspace root directory",
            filter = "Glob pattern to filter target packages by name (optional, default: all)",
            execute = "Actually publish to registries (default: false, dry-run unless set)",
            no_tag = "Skip creating git tags after publish (optional, default: false)",
            no_commit = "Skip auto-commit after version bumps (optional, default: false)",
            bump = "Version bump kind for auto-bump: patch, minor, major (optional, default: patch)"
        )
    )]
    pub async fn publish(
        &self,
        path: String,
        filter: Option<String>,
        execute: Option<bool>,
        no_tag: Option<bool>,
        no_commit: Option<bool>,
        bump: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = !execute.unwrap_or(false);
        let skip_tags = no_tag.unwrap_or(false);
        let skip_commits = no_commit.unwrap_or(false);
        let bump_kind = match bump.as_deref() {
            Some("minor") => crate::types::VersionBump::Minor,
            Some("major") => crate::types::VersionBump::Major,
            _ => crate::types::VersionBump::Patch,
        };
        let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

        stream! {
            let workspace_path = PathBuf::from(&path);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            // Build dep graph
            let mut nodes = Vec::new();
            let mut all_deps = Vec::new();

            for (idx, repo) in ctx.repos.iter().enumerate() {
                let name = repo.package_name.clone().unwrap_or_else(|| repo.dir_name.clone());
                let version = repo.package_version.clone();

                nodes.push(crate::build_system::dep_graph::DepNode {
                    name,
                    version,
                    build_system: format!("{}", repo.build_system),
                    path: repo.dir_name.clone(),
                });

                // Exclude dev-deps for publish graph
                let non_dev_deps: Vec<_> = repo.dependencies.iter()
                    .filter(|d| !d.is_dev)
                    .cloned()
                    .collect();
                if !non_dev_deps.is_empty() {
                    all_deps.push((idx, non_dev_deps));
                }
            }

            if nodes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace.".to_string(),
                };
                return;
            }

            let graph = crate::build_system::dep_graph::DepGraph::build(nodes, &all_deps);

            // Resolve targets from filter
            let targets: Vec<usize> = graph.nodes.iter().enumerate()
                .filter(|(_, node)| {
                    if let Some(ref pat) = filter {
                        glob_match(pat, &node.name)
                    } else {
                        // Default: all packages with a registry
                        match node.build_system.as_str() {
                            "cargo" | "cabal" => true,
                            _ => false,
                        }
                    }
                })
                .map(|(i, _)| i)
                .collect();

            if targets.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No publishable packages matched filter.".to_string(),
                };
                return;
            }

            // Build publish plan (queries registries, computes transitive closure)
            let plan = match crate::build_system::publish::build_publish_plan(
                &graph,
                &targets,
                &workspace_path,
                &bump_kind,
            ).await {
                Ok(p) => p,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to build publish plan: {}", e),
                    };
                    return;
                }
            };

            // Report exclusions
            for (name, reason) in &plan.excluded {
                yield HyperforgeEvent::Info {
                    message: format!("{}Excluded {}: {}", dry_prefix, name, reason),
                };
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Publish plan: {} packages in dependency order",
                    dry_prefix,
                    plan.steps.len()
                ),
            };

            // Track failed nodes to skip dependents
            let mut failed_nodes: HashSet<usize> = HashSet::new();
            let mut published_count = 0usize;
            let mut auto_bumped_count = 0usize;
            let mut skipped_count = 0usize;
            let mut failed_count = 0usize;
            let mut tags_created = 0usize;

            for step in &plan.steps {
                // Check if any dependency failed
                let dep_failed = graph.direct_deps(step.node_idx)
                    .iter()
                    .any(|dep_idx| failed_nodes.contains(dep_idx));

                if dep_failed {
                    failed_nodes.insert(step.node_idx);
                    failed_count += 1;
                    yield HyperforgeEvent::PublishStep {
                        package_name: step.name.clone(),
                        version: step.target_version.clone(),
                        registry: crate::hub::PackageRegistry::CratesIo, // placeholder
                        action: crate::hub::PublishActionKind::Failed,
                        success: false,
                        error: Some("dependency failed to publish".to_string()),
                    };
                    continue;
                }

                let build_system = &step.build_system;
                let registry = match crate::package::registry_for(build_system) {
                    Some(r) => r,
                    None => continue,
                };
                let registry_kind = registry.registry_kind();

                match &step.action {
                    crate::build_system::publish::PublishAction::Skip => {
                        skipped_count += 1;
                        yield HyperforgeEvent::PublishStep {
                            package_name: step.name.clone(),
                            version: step.target_version.clone(),
                            registry: registry_kind,
                            action: crate::hub::PublishActionKind::Skip,
                            success: true,
                            error: None,
                        };
                    }
                    crate::build_system::publish::PublishAction::Error(msg) => {
                        failed_nodes.insert(step.node_idx);
                        failed_count += 1;
                        yield HyperforgeEvent::PublishStep {
                            package_name: step.name.clone(),
                            version: step.target_version.clone(),
                            registry: registry_kind,
                            action: crate::hub::PublishActionKind::Failed,
                            success: false,
                            error: Some(msg.clone()),
                        };
                    }
                    action => {
                        let is_auto_bump = matches!(action, crate::build_system::publish::PublishAction::AutoBump);
                        let action_kind = match action {
                            crate::build_system::publish::PublishAction::Publish => crate::hub::PublishActionKind::Publish,
                            crate::build_system::publish::PublishAction::AutoBump => crate::hub::PublishActionKind::AutoBump,
                            crate::build_system::publish::PublishAction::InitialPublish => crate::hub::PublishActionKind::InitialPublish,
                            _ => unreachable!(),
                        };

                        // Auto-bump: edit manifest and optionally commit
                        if is_auto_bump && !is_dry_run {
                            if let Err(e) = crate::build_system::version::set_package_version(
                                &step.path,
                                build_system,
                                &step.target_version,
                            ) {
                                failed_nodes.insert(step.node_idx);
                                failed_count += 1;
                                yield HyperforgeEvent::PublishStep {
                                    package_name: step.name.clone(),
                                    version: step.target_version.clone(),
                                    registry: registry_kind,
                                    action: crate::hub::PublishActionKind::Failed,
                                    success: false,
                                    error: Some(format!("version bump failed: {}", e)),
                                };
                                continue;
                            }

                            if !skip_commits {
                                // Stage and commit the version bump
                                let manifest_file = match build_system {
                                    crate::build_system::BuildSystemKind::Cargo => "Cargo.toml",
                                    crate::build_system::BuildSystemKind::Cabal => {
                                        // Find .cabal file name
                                        &step.name
                                    }
                                    _ => "package.json",
                                };

                                // For cabal, we need the actual filename
                                if *build_system == crate::build_system::BuildSystemKind::Cabal {
                                    // Stage all .cabal files
                                    let _ = Git::add(&step.path, "*.cabal");
                                } else {
                                    let _ = Git::add(&step.path, manifest_file);
                                }

                                let commit_msg = format!(
                                    "chore: bump {} to {}",
                                    step.name, step.target_version
                                );
                                let _ = Git::commit(&step.path, &commit_msg);
                            }

                            auto_bumped_count += 1;
                        } else if is_auto_bump {
                            // Dry run auto-bump
                            auto_bumped_count += 1;
                        }

                        // Publish
                        let result = registry.publish(&step.path, &step.name, is_dry_run).await;

                        match result {
                            Ok(pr) if pr.success => {
                                published_count += 1;

                                yield HyperforgeEvent::PublishStep {
                                    package_name: step.name.clone(),
                                    version: step.target_version.clone(),
                                    registry: registry_kind.clone(),
                                    action: action_kind,
                                    success: true,
                                    error: None,
                                };

                                // Git tag
                                if !skip_tags && !is_dry_run {
                                    let tag_name = format!("{}-v{}", step.name, step.target_version);
                                    let tag_msg = format!("Release {} v{}", step.name, step.target_version);
                                    if let Err(e) = Git::tag(&step.path, &tag_name, Some(&tag_msg)) {
                                        yield HyperforgeEvent::Info {
                                            message: format!("  Warning: failed to create tag {}: {}", tag_name, e),
                                        };
                                    } else {
                                        tags_created += 1;
                                        yield HyperforgeEvent::PublishStep {
                                            package_name: step.name.clone(),
                                            version: step.target_version.clone(),
                                            registry: registry_kind.clone(),
                                            action: crate::hub::PublishActionKind::Tag,
                                            success: true,
                                            error: None,
                                        };
                                    }
                                }
                            }
                            Ok(pr) => {
                                // Publish returned but was not successful
                                failed_nodes.insert(step.node_idx);
                                failed_count += 1;
                                yield HyperforgeEvent::PublishStep {
                                    package_name: step.name.clone(),
                                    version: step.target_version.clone(),
                                    registry: registry_kind,
                                    action: crate::hub::PublishActionKind::Failed,
                                    success: false,
                                    error: pr.error,
                                };
                            }
                            Err(e) => {
                                failed_nodes.insert(step.node_idx);
                                failed_count += 1;
                                yield HyperforgeEvent::PublishStep {
                                    package_name: step.name.clone(),
                                    version: step.target_version.clone(),
                                    registry: registry_kind,
                                    action: crate::hub::PublishActionKind::Failed,
                                    success: false,
                                    error: Some(format!("{}", e)),
                                };
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::PublishSummary {
                total: plan.steps.len(),
                published: published_count,
                auto_bumped: auto_bumped_count,
                skipped: skipped_count,
                failed: failed_count,
                tags_created,
            };
        }
    }

    /// Bump versions for workspace packages
    #[plexus_macros::hub_method(
        description = "Bump package versions across the workspace",
        params(
            path = "Path to workspace root directory",
            filter = "Glob pattern to filter packages by name (optional, default: all)",
            bump = "Version bump kind: patch, minor, major (default: patch)",
            commit = "Auto-commit after bumping (optional, default: false)",
            dry_run = "Preview without writing changes (optional, default: false)"
        )
    )]
    pub async fn bump(
        &self,
        path: String,
        filter: Option<String>,
        bump: Option<String>,
        commit: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let bump_kind = match bump.as_deref() {
            Some("minor") => crate::types::VersionBump::Minor,
            Some("major") => crate::types::VersionBump::Major,
            _ => crate::types::VersionBump::Patch,
        };
        let auto_commit = commit.unwrap_or(false);
        let is_dry_run = dry_run.unwrap_or(false);
        let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

        stream! {
            let workspace_path = PathBuf::from(&path);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            let repos: Vec<_> = if let Some(ref pattern) = filter {
                ctx.repos.iter().filter(|r| {
                    let name = r.package_name.as_deref().unwrap_or(&r.dir_name);
                    glob_match(pattern, name)
                }).collect()
            } else {
                ctx.repos.iter().collect()
            };

            if repos.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages matched filter.".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("{}Bumping {} packages ({:?})...", dry_prefix, repos.len(), bump_kind),
            };

            let mut bumped = 0usize;
            let mut failed = 0usize;

            for repo in &repos {
                let name = repo.package_name.as_deref().unwrap_or(&repo.dir_name);
                let current_version = match &repo.package_version {
                    Some(v) => v.clone(),
                    None => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: skipped (no version)", name),
                        };
                        continue;
                    }
                };

                let parsed = match crate::build_system::version::SemVer::parse(&current_version) {
                    Some(v) => v,
                    None => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: skipped (unparseable version: {})", name, current_version),
                        };
                        continue;
                    }
                };

                let new_version = parsed.bump(&bump_kind).to_string();

                if !is_dry_run {
                    match crate::build_system::version::set_package_version(
                        &repo.path,
                        &repo.build_system,
                        &new_version,
                    ) {
                        Ok(_) => {
                            bumped += 1;

                            if auto_commit {
                                let manifest_file = match repo.build_system {
                                    crate::build_system::BuildSystemKind::Cargo => "Cargo.toml",
                                    crate::build_system::BuildSystemKind::Cabal => "*.cabal",
                                    _ => "package.json",
                                };
                                let _ = Git::add(&repo.path, manifest_file);
                                let commit_msg = format!("chore: bump {} to {}", name, new_version);
                                let _ = Git::commit(&repo.path, &commit_msg);
                            }
                        }
                        Err(e) => {
                            failed += 1;
                            yield HyperforgeEvent::Error {
                                message: format!("  {}: bump failed: {}", name, e),
                            };
                            continue;
                        }
                    }
                } else {
                    bumped += 1;
                }

                yield HyperforgeEvent::PublishStep {
                    package_name: name.to_string(),
                    version: new_version.clone(),
                    registry: match repo.build_system {
                        crate::build_system::BuildSystemKind::Cargo => crate::hub::PackageRegistry::CratesIo,
                        crate::build_system::BuildSystemKind::Cabal => crate::hub::PackageRegistry::Hackage,
                        _ => crate::hub::PackageRegistry::Npm,
                    },
                    action: crate::hub::PublishActionKind::AutoBump,
                    success: true,
                    error: None,
                };
            }

            yield HyperforgeEvent::Info {
                message: format!("{}Bump complete: {} bumped, {} failed", dry_prefix, bumped, failed),
            };
        }
    }
}

#[async_trait]
impl ChildRouter for WorkspaceHub {
    fn router_namespace(&self) -> &str {
        "workspace"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
