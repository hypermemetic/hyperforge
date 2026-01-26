//! Status command - Show repository sync status
//!
//! `hyperforge status --path .`
//!
//! This command shows:
//! - Current branch and tracking info
//! - Sync status for each configured forge (ahead/behind)
//! - Working tree status (clean/dirty)

use std::path::Path;
use thiserror::Error;

use crate::config::HyperforgeConfig;
use crate::git::{Git, GitError};

/// Errors that can occur during status
#[derive(Debug, Error)]
pub enum StatusError {
    #[error("Not a hyperforge repository. Run 'hyperforge init' first.")]
    NotInitialized,

    #[error("Not a git repository: {path}")]
    NotAGitRepo { path: String },

    #[error("Git error: {0}")]
    GitError(#[from] GitError),

    #[error("Config error: {0}")]
    ConfigError(#[from] crate::config::ConfigError),
}

pub type StatusResult<T> = Result<T, StatusError>;

/// Status of a single forge remote
#[derive(Debug, Clone)]
pub struct ForgeStatus {
    /// Forge name (e.g., "github")
    pub forge: String,

    /// Git remote name (e.g., "origin")
    pub remote_name: String,

    /// Remote URL
    pub remote_url: Option<String>,

    /// Number of commits ahead of remote
    pub ahead: u32,

    /// Number of commits behind remote
    pub behind: u32,

    /// Whether the remote exists in git
    pub remote_exists: bool,

    /// Any error message
    pub error: Option<String>,
}

impl ForgeStatus {
    /// Check if this forge is up to date
    pub fn is_up_to_date(&self) -> bool {
        self.remote_exists && self.ahead == 0 && self.behind == 0 && self.error.is_none()
    }

    /// Check if needs push (ahead of remote)
    pub fn needs_push(&self) -> bool {
        self.ahead > 0
    }

    /// Check if needs pull (behind remote)
    pub fn needs_pull(&self) -> bool {
        self.behind > 0
    }

    /// Get a status symbol
    pub fn symbol(&self) -> &'static str {
        if self.error.is_some() {
            "✗"
        } else if !self.remote_exists {
            "?"
        } else if self.is_up_to_date() {
            "✓"
        } else if self.ahead > 0 && self.behind > 0 {
            "↕"
        } else if self.ahead > 0 {
            "↑"
        } else if self.behind > 0 {
            "↓"
        } else {
            "✓"
        }
    }

    /// Get a human-readable status message
    pub fn message(&self) -> String {
        if let Some(ref err) = self.error {
            return format!("error: {}", err);
        }

        if !self.remote_exists {
            return "remote not configured".to_string();
        }

        if self.is_up_to_date() {
            return "up to date".to_string();
        }

        let mut parts = Vec::new();
        if self.ahead > 0 {
            parts.push(format!("{} ahead", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("{} behind", self.behind));
        }

        parts.join(", ")
    }
}

/// Overall repository status
#[derive(Debug)]
pub struct RepoStatusReport {
    /// Path to the repository
    pub repo_path: String,

    /// Current branch name
    pub branch: String,

    /// Status for each configured forge
    pub forges: Vec<ForgeStatus>,

    /// Whether working tree has uncommitted changes
    pub has_changes: bool,

    /// Whether there are staged changes
    pub has_staged: bool,

    /// Whether there are untracked files
    pub has_untracked: bool,
}

impl RepoStatusReport {
    /// Check if all forges are up to date
    pub fn all_up_to_date(&self) -> bool {
        self.forges.iter().all(|f| f.is_up_to_date())
    }

    /// Check if any forge needs push
    pub fn needs_push(&self) -> bool {
        self.forges.iter().any(|f| f.needs_push())
    }

    /// Check if any forge needs pull
    pub fn needs_pull(&self) -> bool {
        self.forges.iter().any(|f| f.needs_pull())
    }

    /// Check if working tree is clean
    pub fn is_clean(&self) -> bool {
        !self.has_changes && !self.has_staged && !self.has_untracked
    }

    /// Format as a human-readable string
    pub fn format(&self) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push(format!("Repository: {}", self.repo_path));
        lines.push(format!("Branch: {}", self.branch));

        // Working tree status
        if self.is_clean() {
            lines.push("Working tree: clean".to_string());
        } else {
            let mut status_parts = Vec::new();
            if self.has_staged {
                status_parts.push("staged changes");
            }
            if self.has_changes {
                status_parts.push("unstaged changes");
            }
            if self.has_untracked {
                status_parts.push("untracked files");
            }
            lines.push(format!("Working tree: {}", status_parts.join(", ")));
        }

        lines.push(String::new());

        // Forge status
        lines.push("Forges:".to_string());
        for forge in &self.forges {
            lines.push(format!(
                "  {} {} ({}): {}",
                forge.symbol(),
                forge.forge,
                forge.remote_name,
                forge.message()
            ));
        }

        lines.join("\n")
    }
}

/// Get status for a hyperforge repository
///
/// # Arguments
/// * `path` - Path to the repository
///
/// # Returns
/// RepoStatusReport with status information
pub fn status(path: &Path) -> StatusResult<RepoStatusReport> {
    // Check if hyperforge config exists
    if !HyperforgeConfig::exists(path) {
        return Err(StatusError::NotInitialized);
    }

    // Check if it's a git repo
    if !Git::is_repo(path) {
        return Err(StatusError::NotAGitRepo {
            path: path.display().to_string(),
        });
    }

    // Load config
    let config = HyperforgeConfig::load(path)?;

    // Get git status
    let repo_status = Git::repo_status(path)?;

    // Fetch all remotes to get accurate ahead/behind counts
    // (ignore errors - remote might not be reachable)
    let _ = Git::fetch_all(path);

    // Check status for each forge
    let mut forge_statuses = Vec::new();

    for forge in &config.forges {
        let remote_name = config.remote_for_forge(forge);
        let mut forge_status = ForgeStatus {
            forge: forge.clone(),
            remote_name: remote_name.clone(),
            remote_url: None,
            ahead: 0,
            behind: 0,
            remote_exists: false,
            error: None,
        };

        // Check if remote exists
        match Git::get_remote(path, &remote_name) {
            Ok(remote_info) => {
                forge_status.remote_exists = true;
                forge_status.remote_url = Some(remote_info.fetch_url);

                // Get ahead/behind count
                match Git::ahead_behind(path, &remote_name, &repo_status.branch) {
                    Ok((ahead, behind)) => {
                        forge_status.ahead = ahead;
                        forge_status.behind = behind;
                    }
                    Err(e) => {
                        forge_status.error = Some(format!("Failed to get sync status: {}", e));
                    }
                }
            }
            Err(GitError::RemoteNotFound { .. }) => {
                forge_status.remote_exists = false;
            }
            Err(e) => {
                forge_status.error = Some(e.to_string());
            }
        }

        forge_statuses.push(forge_status);
    }

    Ok(RepoStatusReport {
        repo_path: path.display().to_string(),
        branch: repo_status.branch,
        forges: forge_statuses,
        has_changes: repo_status.has_changes,
        has_staged: repo_status.has_staged,
        has_untracked: repo_status.has_untracked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::init::{init, InitOptions};
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_repo_with_commit(path: &Path) {
        // Configure git user
        Git::config_set(path, "user.email", "test@test.com").unwrap();
        Git::config_set(path, "user.name", "Test").unwrap();

        // Create initial commit
        fs::write(path.join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(path)
            .output()
            .unwrap();
    }

    #[test]
    fn test_status_not_initialized() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        let result = status(temp.path());
        assert!(matches!(result, Err(StatusError::NotInitialized)));
    }

    #[test]
    fn test_status_basic() {
        let temp = TempDir::new().unwrap();

        // Initialize hyperforge
        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice")
            .with_repo_name("test-repo");
        init(temp.path(), options).unwrap();

        // Create a commit
        setup_repo_with_commit(temp.path());

        // Get status
        let report = status(temp.path()).unwrap();

        assert!(!report.branch.is_empty());
        assert_eq!(report.forges.len(), 1);
        assert_eq!(report.forges[0].forge, "github");
        assert_eq!(report.forges[0].remote_name, "origin");
        assert!(report.forges[0].remote_exists);
    }

    #[test]
    fn test_status_multiple_forges() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string(), "codeberg".to_string()])
            .with_org("alice");
        init(temp.path(), options).unwrap();
        setup_repo_with_commit(temp.path());

        let report = status(temp.path()).unwrap();

        assert_eq!(report.forges.len(), 2);
        let github = report.forges.iter().find(|f| f.forge == "github").unwrap();
        let codeberg = report.forges.iter().find(|f| f.forge == "codeberg").unwrap();

        assert_eq!(github.remote_name, "origin");
        assert_eq!(codeberg.remote_name, "codeberg");
    }

    #[test]
    fn test_status_clean_working_tree() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");
        init(temp.path(), options).unwrap();
        setup_repo_with_commit(temp.path());

        let report = status(temp.path()).unwrap();

        assert!(report.is_clean());
    }

    #[test]
    fn test_status_dirty_working_tree() {
        let temp = TempDir::new().unwrap();

        let options = InitOptions::new(vec!["github".to_string()])
            .with_org("alice");
        init(temp.path(), options).unwrap();
        setup_repo_with_commit(temp.path());

        // Make changes
        fs::write(temp.path().join("new-file.txt"), "content").unwrap();

        let report = status(temp.path()).unwrap();

        assert!(report.has_untracked);
        assert!(!report.is_clean());
    }

    #[test]
    fn test_forge_status_symbols() {
        let up_to_date = ForgeStatus {
            forge: "github".to_string(),
            remote_name: "origin".to_string(),
            remote_url: Some("git@github.com:test/repo.git".to_string()),
            ahead: 0,
            behind: 0,
            remote_exists: true,
            error: None,
        };
        assert_eq!(up_to_date.symbol(), "✓");

        let needs_push = ForgeStatus {
            ahead: 2,
            behind: 0,
            ..up_to_date.clone()
        };
        assert_eq!(needs_push.symbol(), "↑");

        let needs_pull = ForgeStatus {
            ahead: 0,
            behind: 3,
            ..up_to_date.clone()
        };
        assert_eq!(needs_pull.symbol(), "↓");

        let diverged = ForgeStatus {
            ahead: 1,
            behind: 1,
            ..up_to_date.clone()
        };
        assert_eq!(diverged.symbol(), "↕");

        let not_configured = ForgeStatus {
            remote_exists: false,
            ..up_to_date.clone()
        };
        assert_eq!(not_configured.symbol(), "?");

        let error = ForgeStatus {
            error: Some("network error".to_string()),
            ..up_to_date.clone()
        };
        assert_eq!(error.symbol(), "✗");
    }

    #[test]
    fn test_forge_status_messages() {
        let up_to_date = ForgeStatus {
            forge: "github".to_string(),
            remote_name: "origin".to_string(),
            remote_url: Some("url".to_string()),
            ahead: 0,
            behind: 0,
            remote_exists: true,
            error: None,
        };
        assert_eq!(up_to_date.message(), "up to date");

        let needs_push = ForgeStatus {
            ahead: 2,
            ..up_to_date.clone()
        };
        assert_eq!(needs_push.message(), "2 ahead");

        let diverged = ForgeStatus {
            ahead: 1,
            behind: 3,
            ..up_to_date.clone()
        };
        assert_eq!(diverged.message(), "1 ahead, 3 behind");

        let not_configured = ForgeStatus {
            remote_exists: false,
            ..up_to_date.clone()
        };
        assert_eq!(not_configured.message(), "remote not configured");
    }

    #[test]
    fn test_status_format() {
        let report = RepoStatusReport {
            repo_path: "/test/repo".to_string(),
            branch: "main".to_string(),
            forges: vec![
                ForgeStatus {
                    forge: "github".to_string(),
                    remote_name: "origin".to_string(),
                    remote_url: Some("git@github.com:test/repo.git".to_string()),
                    ahead: 0,
                    behind: 0,
                    remote_exists: true,
                    error: None,
                },
                ForgeStatus {
                    forge: "codeberg".to_string(),
                    remote_name: "codeberg".to_string(),
                    remote_url: Some("git@codeberg.org:test/repo.git".to_string()),
                    ahead: 2,
                    behind: 0,
                    remote_exists: true,
                    error: None,
                },
            ],
            has_changes: false,
            has_staged: false,
            has_untracked: false,
        };

        let formatted = report.format();
        assert!(formatted.contains("Repository: /test/repo"));
        assert!(formatted.contains("Branch: main"));
        assert!(formatted.contains("Working tree: clean"));
        assert!(formatted.contains("✓ github"));
        assert!(formatted.contains("↑ codeberg"));
    }
}
