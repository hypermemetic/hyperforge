//! Init command - Initialize hyperforge for a repository
//!
//! `hyperforge init --path . --forges github,codeberg`
//!
//! This command:
//! 1. Creates a git repo if needed
//! 2. Creates .hyperforge/config.toml
//! 3. Configures git remotes for each forge
//! 4. Sets up SSH keys if specified

use std::path::Path;
use thiserror::Error;

use crate::config::HyperforgeConfig;
use crate::git::{self, Git, GitError};
use crate::types::Visibility;

/// Errors that can occur during init
#[derive(Debug, Error)]
pub enum InitError {
    #[error("Config already exists at {path}. Use --force to reinitialize.")]
    AlreadyExists { path: String },

    #[error("Git error: {0}")]
    GitError(#[from] GitError),

    #[error("Config error: {0}")]
    ConfigError(#[from] crate::config::ConfigError),

    #[error("Invalid forge: {forge}. Valid forges: github, codeberg, gitlab")]
    InvalidForge { forge: String },

    #[error("Organization required. Use --org to specify.")]
    OrgRequired,

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type InitResult<T> = Result<T, InitError>;

/// Options for the init command
#[derive(Debug, Clone)]
pub struct InitOptions {
    /// Forges to configure (e.g., ["github", "codeberg"])
    pub forges: Vec<String>,

    /// Organization/username on forges
    pub org: Option<String>,

    /// Repository name (defaults to directory name)
    pub repo_name: Option<String>,

    /// Repository visibility
    pub visibility: Visibility,

    /// Repository description
    pub description: Option<String>,

    /// SSH key paths per forge
    pub ssh_keys: Vec<(String, String)>,

    /// Force reinitialize even if config exists
    pub force: bool,

    /// Dry run - don't actually make changes
    pub dry_run: bool,

    /// Skip installing hooks
    pub no_hooks: bool,

    /// Skip configuring SSH wrapper
    pub no_ssh_wrapper: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            forges: vec!["github".to_string()],
            org: None,
            repo_name: None,
            visibility: Visibility::Public,
            description: None,
            ssh_keys: Vec::new(),
            force: false,
            dry_run: false,
            no_hooks: false,
            no_ssh_wrapper: false,
        }
    }
}

impl InitOptions {
    pub fn new(forges: Vec<String>) -> Self {
        Self {
            forges,
            ..Default::default()
        }
    }

    pub fn with_org(mut self, org: impl Into<String>) -> Self {
        self.org = Some(org.into());
        self
    }

    pub fn with_repo_name(mut self, name: impl Into<String>) -> Self {
        self.repo_name = Some(name.into());
        self
    }

    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_ssh_key(mut self, forge: impl Into<String>, key_path: impl Into<String>) -> Self {
        self.ssh_keys.push((forge.into(), key_path.into()));
        self
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    pub fn no_hooks(mut self) -> Self {
        self.no_hooks = true;
        self
    }

    pub fn no_ssh_wrapper(mut self) -> Self {
        self.no_ssh_wrapper = true;
        self
    }
}

/// Result of init operation
#[derive(Debug)]
pub struct InitReport {
    /// Path to the repository
    pub repo_path: String,

    /// Whether git was initialized
    pub git_initialized: bool,

    /// Config that was created
    pub config: HyperforgeConfig,

    /// Remotes that were added
    pub remotes_added: Vec<RemoteAdded>,

    /// Whether this was a dry run
    pub dry_run: bool,

    /// Whether hooks were installed
    pub hooks_installed: bool,

    /// Whether SSH wrapper was configured
    pub ssh_configured: bool,
}

#[derive(Debug, Clone)]
pub struct RemoteAdded {
    pub name: String,
    pub url: String,
    pub forge: String,
}

/// Initialize hyperforge for a repository
///
/// # Arguments
/// * `path` - Path to the repository
/// * `options` - Init options
///
/// # Returns
/// InitReport describing what was done
pub fn init(path: &Path, options: InitOptions) -> InitResult<InitReport> {
    // Validate forges
    for forge in &options.forges {
        if HyperforgeConfig::parse_forge(forge).is_none() {
            return Err(InitError::InvalidForge {
                forge: forge.clone(),
            });
        }
    }

    // Check if already initialized
    if HyperforgeConfig::exists(path) && !options.force {
        return Err(InitError::AlreadyExists {
            path: HyperforgeConfig::config_path(path).display().to_string(),
        });
    }

    let mut report = InitReport {
        repo_path: path.display().to_string(),
        git_initialized: false,
        config: HyperforgeConfig::default(),
        remotes_added: Vec::new(),
        dry_run: options.dry_run,
        hooks_installed: false,
        ssh_configured: false,
    };

    // Initialize git if needed
    if !Git::is_repo(path) {
        if !options.dry_run {
            Git::init(path)?;
        }
        report.git_initialized = true;
    }

    // Build config
    let mut config = HyperforgeConfig::new(options.forges.clone());

    if let Some(ref org) = options.org {
        config = config.with_org(org);
    }

    if let Some(ref name) = options.repo_name {
        config = config.with_repo_name(name);
    }

    config = config.with_visibility(options.visibility.clone());

    if let Some(ref desc) = options.description {
        config = config.with_description(desc);
    }

    for (forge, key_path) in &options.ssh_keys {
        config = config.with_ssh_key(forge, key_path);
    }

    // Validate config
    config.validate()?;

    // Get org (required for remote setup)
    let org = options.org.as_deref().or(config.org.as_deref());

    // Configure git remotes
    if let Some(org) = org {
        let repo_name = config.get_repo_name(path);

        for forge in &options.forges {
            let remote_name = config.remote_for_forge(forge);
            let remote_url = git::build_remote_url(forge, org, &repo_name);

            if !options.dry_run {
                // Check if remote already exists
                match Git::get_remote(path, &remote_name) {
                    Ok(existing) => {
                        // Remote exists - update URL if different
                        if existing.fetch_url != remote_url {
                            Git::set_remote_url(path, &remote_name, &remote_url)?;
                        }
                    }
                    Err(GitError::RemoteNotFound { .. }) => {
                        // Add new remote
                        Git::add_remote(path, &remote_name, &remote_url)?;
                    }
                    Err(e) => return Err(e.into()),
                }

                // Configure SSH key if specified
                if let Some(key_path) = config.ssh_key_for_forge(forge) {
                    Git::configure_ssh(path, key_path)?;
                }
            }

            report.remotes_added.push(RemoteAdded {
                name: remote_name,
                url: remote_url,
                forge: forge.clone(),
            });
        }
    }

    // Save config
    if !options.dry_run {
        config.save(path)?;
    }

    // Install pre-push hook
    if !options.no_hooks {
        if !options.dry_run {
            match crate::commands::hooks::install_pre_push_hook(path, false) {
                Ok(installed) => {
                    if installed {
                        // Set git hooksPath to .hyperforge/hooks
                        let _ = Git::config_set(path, "core.hooksPath", ".hyperforge/hooks");
                    }
                }
                Err(e) => {
                    // Non-fatal: log but continue
                    eprintln!("Warning: failed to install pre-push hook: {}", e);
                }
            }
        }
        report.hooks_installed = true;
    }

    // Configure SSH wrapper (skip if per-forge SSH keys were explicitly configured,
    // since those set core.sshCommand to a more specific value)
    if !options.no_ssh_wrapper && options.ssh_keys.is_empty() {
        if !options.dry_run {
            let _ = Git::config_set(path, "core.sshCommand", "hyperforge-ssh");
        }
        report.ssh_configured = true;
    }

    report.config = config;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_new_repo() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice")
            .with_repo_name("test-repo");

        let report = init(temp.path(), options).unwrap();

        assert!(report.git_initialized);
        assert!(HyperforgeConfig::exists(temp.path()));
        assert_eq!(report.remotes_added.len(), 1);
        assert_eq!(report.remotes_added[0].name, "origin");
        assert_eq!(
            report.remotes_added[0].url,
            "git@github.com:alice/test-repo.git"
        );
    }

    #[test]
    fn test_init_existing_git_repo() {
        let temp = TempDir::new().unwrap();

        // Pre-initialize git
        Git::init(temp.path()).unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");

        let report = init(temp.path(), options).unwrap();

        assert!(!report.git_initialized); // Already existed
        assert!(HyperforgeConfig::exists(temp.path()));
    }

    #[test]
    fn test_init_multiple_forges() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string(), "codeberg".to_string()])
            .with_org("alice")
            .with_repo_name("multi-forge");

        let report = init(temp.path(), options).unwrap();

        assert_eq!(report.remotes_added.len(), 2);

        let github = report.remotes_added.iter().find(|r| r.forge == "github").unwrap();
        let codeberg = report.remotes_added.iter().find(|r| r.forge == "codeberg").unwrap();

        assert_eq!(github.name, "origin"); // First forge is origin
        assert_eq!(codeberg.name, "codeberg");

        // Verify remotes in git
        let remotes = Git::list_remotes(temp.path()).unwrap();
        assert_eq!(remotes.len(), 2);
    }

    #[test]
    fn test_init_already_exists() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");

        // First init
        init(temp.path(), options.clone()).unwrap();

        // Second init should fail
        let result = init(temp.path(), options);
        assert!(matches!(result, Err(InitError::AlreadyExists { .. })));
    }

    #[test]
    fn test_init_force_reinit() {
        let temp = TempDir::new().unwrap();

        let options1 = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");

        init(temp.path(), options1).unwrap();

        // Force reinit with different forges
        let options2 = InitOptions::new(vec!["codeberg".to_string()])
            .with_org("alice")
            .force();

        let report = init(temp.path(), options2).unwrap();

        // Should have codeberg config now
        assert_eq!(report.config.forges, vec!["codeberg"]);
    }

    #[test]
    fn test_init_dry_run() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice")
            .dry_run();

        let report = init(temp.path(), options).unwrap();

        assert!(report.dry_run);
        // Git should NOT be initialized
        assert!(!Git::is_repo(temp.path()));
        // Config should NOT exist
        assert!(!HyperforgeConfig::exists(temp.path()));
        // But report should show what would be done
        assert!(report.git_initialized);
        assert_eq!(report.remotes_added.len(), 1);
    }

    #[test]
    fn test_init_invalid_forge() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["invalid-forge".to_string()])
            .with_org("alice");

        let result = init(temp.path(), options);
        assert!(matches!(result, Err(InitError::InvalidForge { .. })));
    }

    #[test]
    fn test_init_repo_name_from_path() {
        let temp = TempDir::new().unwrap();

        // Don't specify repo_name, should use directory name
        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");

        let report = init(temp.path(), options).unwrap();

        // Remote URL should use temp directory name
        let dir_name = temp.path().file_name().unwrap().to_str().unwrap();
        assert!(report.remotes_added[0].url.contains(dir_name));
    }

    #[test]
    fn test_init_with_ssh_key() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice")
            .with_ssh_key("github", "~/.ssh/github_key");

        init(temp.path(), options).unwrap();

        // Verify SSH command was set
        let ssh_cmd = Git::config_get(temp.path(), "core.sshCommand").unwrap();
        assert!(ssh_cmd.is_some());
        assert!(ssh_cmd.unwrap().contains("github_key"));
    }

    #[test]
    fn test_init_updates_existing_remote() {
        let temp = TempDir::new().unwrap();

        // Pre-setup: init git and add a remote with different URL
        Git::init(temp.path()).unwrap();
        Git::add_remote(temp.path(), "origin", "git@github.com:old/url.git").unwrap();

        // Init hyperforge - should update the remote URL
        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice")
            .with_repo_name("new-repo");

        init(temp.path(), options).unwrap();

        // Verify remote was updated
        let remote = Git::get_remote(temp.path(), "origin").unwrap();
        assert_eq!(remote.fetch_url, "git@github.com:alice/new-repo.git");
    }
}
