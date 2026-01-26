//! Git operations and remote management
//!
//! This module provides git command execution and parsing for hyperforge.
//! It uses git as the source of truth for repository state.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

/// Errors that can occur during git operations
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git command failed: {message}")]
    CommandFailed { message: String },

    #[error("Not a git repository: {path}")]
    NotARepo { path: String },

    #[error("Remote not found: {name}")]
    RemoteNotFound { name: String },

    #[error("Remote already exists: {name}")]
    RemoteAlreadyExists { name: String },

    #[error("Failed to parse git output: {message}")]
    ParseError { message: String },

    #[error("Git not installed or not in PATH")]
    GitNotFound,

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;

/// Information about a git remote
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// Branch tracking information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchStatus {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// Repository status
#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: String,
    pub tracking: Option<BranchStatus>,
    pub has_changes: bool,
    pub has_staged: bool,
    pub has_untracked: bool,
}

/// Git operations helper
pub struct Git;

impl Git {
    /// Check if a path is a git repository
    pub fn is_repo(path: &Path) -> bool {
        path.join(".git").exists() || path.join(".git").is_file()
    }

    /// Initialize a new git repository
    pub fn init(path: &Path) -> GitResult<()> {
        let output = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Clone a git repository
    pub fn clone(url: &str, target_path: &str) -> GitResult<()> {
        let output = Command::new("git")
            .args(["clone", url, target_path])
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// List all remotes with their URLs
    pub fn list_remotes(path: &Path) -> GitResult<Vec<RemoteInfo>> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["remote", "-v"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_remotes(&stdout)
    }

    /// Get a specific remote's info
    pub fn get_remote(path: &Path, name: &str) -> GitResult<RemoteInfo> {
        let remotes = Self::list_remotes(path)?;
        remotes
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| GitError::RemoteNotFound {
                name: name.to_string(),
            })
    }

    /// Add a new remote
    pub fn add_remote(path: &Path, name: &str, url: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["remote", "add", name, url])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already exists") {
                return Err(GitError::RemoteAlreadyExists {
                    name: name.to_string(),
                });
            }
            return Err(GitError::CommandFailed {
                message: stderr.to_string(),
            });
        }

        Ok(())
    }

    /// Remove a remote
    pub fn remove_remote(path: &Path, name: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["remote", "remove", name])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such remote") {
                return Err(GitError::RemoteNotFound {
                    name: name.to_string(),
                });
            }
            return Err(GitError::CommandFailed {
                message: stderr.to_string(),
            });
        }

        Ok(())
    }

    /// Set a remote's URL
    pub fn set_remote_url(path: &Path, name: &str, url: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["remote", "set-url", name, url])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such remote") {
                return Err(GitError::RemoteNotFound {
                    name: name.to_string(),
                });
            }
            return Err(GitError::CommandFailed {
                message: stderr.to_string(),
            });
        }

        Ok(())
    }

    /// Configure SSH key for the repository using core.sshCommand
    pub fn configure_ssh(path: &Path, key_path: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        // Expand ~ in key path
        let expanded_path = if key_path.starts_with("~/") {
            dirs::home_dir()
                .map(|h| h.join(&key_path[2..]))
                .unwrap_or_else(|| key_path.into())
        } else {
            key_path.into()
        };

        let ssh_command = format!(
            "ssh -i {} -o IdentitiesOnly=yes",
            expanded_path.display()
        );

        let output = Command::new("git")
            .args(["config", "core.sshCommand", &ssh_command])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Get current branch name
    pub fn current_branch(path: &Path) -> GitResult<String> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get branch tracking status (ahead/behind)
    pub fn branch_status(path: &Path) -> GitResult<Option<BranchStatus>> {
        Self::ensure_repo(path)?;

        // Get verbose branch info
        let output = Command::new("git")
            .args(["branch", "-vv", "--no-color"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_branch_status(&stdout)
    }

    /// Get repository status (changes, staged, untracked)
    pub fn repo_status(path: &Path) -> GitResult<RepoStatus> {
        Self::ensure_repo(path)?;

        let branch = Self::current_branch(path)?;
        let tracking = Self::branch_status(path)?;

        // Get porcelain status for parsing
        let output = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut has_changes = false;
        let mut has_staged = false;
        let mut has_untracked = false;

        for line in stdout.lines() {
            if line.len() < 2 {
                continue;
            }
            let index_status = line.chars().next().unwrap_or(' ');
            let worktree_status = line.chars().nth(1).unwrap_or(' ');

            if index_status == '?' {
                has_untracked = true;
            } else {
                if index_status != ' ' {
                    has_staged = true;
                }
                if worktree_status != ' ' {
                    has_changes = true;
                }
            }
        }

        Ok(RepoStatus {
            branch,
            tracking,
            has_changes,
            has_staged,
            has_untracked,
        })
    }

    /// Push to a remote
    pub fn push(path: &Path, remote: &str, branch: Option<&str>) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let mut args = vec!["push", remote];
        if let Some(b) = branch {
            args.push(b);
        }

        let output = Command::new("git")
            .args(&args)
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Push with upstream tracking
    pub fn push_set_upstream(path: &Path, remote: &str, branch: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["push", "-u", remote, branch])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Pull from a remote
    pub fn pull(path: &Path, remote: Option<&str>) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let mut args = vec!["pull"];
        if let Some(r) = remote {
            args.push(r);
        }

        let output = Command::new("git")
            .args(&args)
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Fetch from all remotes
    pub fn fetch_all(path: &Path) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["fetch", "--all"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Fetch from a specific remote
    pub fn fetch(path: &Path, remote: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["fetch", remote])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    /// Get ahead/behind count for a specific remote
    pub fn ahead_behind(path: &Path, remote: &str, branch: &str) -> GitResult<(u32, u32)> {
        Self::ensure_repo(path)?;

        let upstream = format!("{}/{}", remote, branch);
        let output = Command::new("git")
            .args(["rev-list", "--left-right", "--count", &format!("{}...HEAD", upstream)])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            // Remote branch might not exist yet
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("unknown revision") {
                return Ok((0, 0));
            }
            return Err(GitError::CommandFailed {
                message: stderr.to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
        if parts.len() != 2 {
            return Ok((0, 0));
        }

        let behind = parts[0].parse().unwrap_or(0);
        let ahead = parts[1].parse().unwrap_or(0);

        Ok((ahead, behind))
    }

    /// Get git config value
    pub fn config_get(path: &Path, key: &str) -> GitResult<Option<String>> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["config", "--get", key])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            // Key not found is not an error
            return Ok(None);
        }

        Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_string()))
    }

    /// Set git config value
    pub fn config_set(path: &Path, key: &str, value: &str) -> GitResult<()> {
        Self::ensure_repo(path)?;

        let output = Command::new("git")
            .args(["config", key, value])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    // --- Private helper methods ---

    fn ensure_repo(path: &Path) -> GitResult<()> {
        if !Self::is_repo(path) {
            return Err(GitError::NotARepo {
                path: path.display().to_string(),
            });
        }
        Ok(())
    }

    fn parse_remotes(output: &str) -> GitResult<Vec<RemoteInfo>> {
        let mut remotes: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

        for line in output.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }

            let name = parts[0].to_string();
            let url = parts[1].to_string();
            let kind = parts[2]; // (fetch) or (push)

            let entry = remotes.entry(name).or_insert((None, None));
            if kind.contains("fetch") {
                entry.0 = Some(url);
            } else if kind.contains("push") {
                entry.1 = Some(url);
            }
        }

        Ok(remotes
            .into_iter()
            .filter_map(|(name, (fetch, push))| {
                let fetch_url = fetch?;
                let push_url = push.unwrap_or_else(|| fetch_url.clone());
                Some(RemoteInfo {
                    name,
                    fetch_url,
                    push_url,
                })
            })
            .collect())
    }

    fn parse_branch_status(output: &str) -> GitResult<Option<BranchStatus>> {
        // Find the current branch (marked with *)
        for line in output.lines() {
            if !line.starts_with('*') {
                continue;
            }

            // Format: * branch_name hash [upstream: ahead N, behind M] commit message
            let line = line.trim_start_matches('*').trim();
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let branch_name = parts[0].to_string();

            // Look for tracking info in brackets
            let mut upstream = None;
            let mut ahead = 0u32;
            let mut behind = 0u32;

            if let Some(start) = line.find('[') {
                if let Some(end) = line.find(']') {
                    let tracking_info = &line[start + 1..end];

                    // Parse upstream name (before :)
                    if let Some(colon_pos) = tracking_info.find(':') {
                        upstream = Some(tracking_info[..colon_pos].to_string());
                        let status_part = &tracking_info[colon_pos + 1..];

                        // Parse ahead/behind
                        if status_part.contains("ahead") {
                            if let Some(n) = Self::extract_number(status_part, "ahead") {
                                ahead = n;
                            }
                        }
                        if status_part.contains("behind") {
                            if let Some(n) = Self::extract_number(status_part, "behind") {
                                behind = n;
                            }
                        }
                    } else {
                        // No colon means just the upstream name (up to date)
                        upstream = Some(tracking_info.trim().to_string());
                    }
                }
            }

            return Ok(Some(BranchStatus {
                name: branch_name,
                upstream,
                ahead,
                behind,
            }));
        }

        Ok(None)
    }

    fn extract_number(s: &str, prefix: &str) -> Option<u32> {
        let idx = s.find(prefix)?;
        let rest = &s[idx + prefix.len()..];
        // Take only consecutive digits after skipping whitespace
        let num_str: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        num_str.parse().ok()
    }
}

/// Build a git remote URL for a forge
pub fn build_remote_url(forge: &str, org: &str, repo: &str) -> String {
    match forge.to_lowercase().as_str() {
        "github" => format!("git@github.com:{}/{}.git", org, repo),
        "codeberg" => format!("git@codeberg.org:{}/{}.git", org, repo),
        "gitlab" => format!("git@gitlab.com:{}/{}.git", org, repo),
        _ => format!("git@{}:{}/{}.git", forge, org, repo),
    }
}

/// Extract org and repo from a git remote URL
pub fn parse_remote_url(url: &str) -> Option<(String, String, String)> {
    // SSH format: git@github.com:org/repo.git
    if url.starts_with("git@") {
        let rest = url.trim_start_matches("git@");
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() != 2 {
            return None;
        }

        let host = parts[0];
        let forge = match host {
            "github.com" => "github",
            "codeberg.org" => "codeberg",
            "gitlab.com" => "gitlab",
            _ => host,
        };

        let path = parts[1].trim_end_matches(".git");
        let path_parts: Vec<&str> = path.splitn(2, '/').collect();
        if path_parts.len() != 2 {
            return None;
        }

        return Some((forge.to_string(), path_parts[0].to_string(), path_parts[1].to_string()));
    }

    // HTTPS format: https://github.com/org/repo.git
    if url.starts_with("https://") || url.starts_with("http://") {
        let url = url.trim_start_matches("https://").trim_start_matches("http://");
        let parts: Vec<&str> = url.splitn(2, '/').collect();
        if parts.len() != 2 {
            return None;
        }

        let host = parts[0];
        let forge = match host {
            "github.com" => "github",
            "codeberg.org" => "codeberg",
            "gitlab.com" => "gitlab",
            _ => host,
        };

        let path = parts[1].trim_end_matches(".git");
        let path_parts: Vec<&str> = path.splitn(2, '/').collect();
        if path_parts.len() != 2 {
            return None;
        }

        return Some((forge.to_string(), path_parts[0].to_string(), path_parts[1].to_string()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_build_remote_url() {
        assert_eq!(
            build_remote_url("github", "alice", "my-repo"),
            "git@github.com:alice/my-repo.git"
        );
        assert_eq!(
            build_remote_url("codeberg", "bob", "tool"),
            "git@codeberg.org:bob/tool.git"
        );
        assert_eq!(
            build_remote_url("gitlab", "org", "project"),
            "git@gitlab.com:org/project.git"
        );
    }

    #[test]
    fn test_parse_remote_url_ssh() {
        let result = parse_remote_url("git@github.com:alice/my-repo.git");
        assert_eq!(
            result,
            Some(("github".to_string(), "alice".to_string(), "my-repo".to_string()))
        );

        let result = parse_remote_url("git@codeberg.org:bob/tool.git");
        assert_eq!(
            result,
            Some(("codeberg".to_string(), "bob".to_string(), "tool".to_string()))
        );
    }

    #[test]
    fn test_parse_remote_url_https() {
        let result = parse_remote_url("https://github.com/alice/my-repo.git");
        assert_eq!(
            result,
            Some(("github".to_string(), "alice".to_string(), "my-repo".to_string()))
        );
    }

    #[test]
    fn test_parse_remotes() {
        let output = "origin\tgit@github.com:alice/repo.git (fetch)\n\
                      origin\tgit@github.com:alice/repo.git (push)\n\
                      upstream\tgit@github.com:bob/repo.git (fetch)\n\
                      upstream\tgit@github.com:bob/repo.git (push)";

        let remotes = Git::parse_remotes(output).unwrap();
        assert_eq!(remotes.len(), 2);

        let origin = remotes.iter().find(|r| r.name == "origin").unwrap();
        assert_eq!(origin.fetch_url, "git@github.com:alice/repo.git");
    }

    #[test]
    fn test_parse_branch_status_with_tracking() {
        let output = "* main abc1234 [origin/main: ahead 2, behind 1] Latest commit";
        let status = Git::parse_branch_status(output).unwrap().unwrap();

        assert_eq!(status.name, "main");
        assert_eq!(status.upstream, Some("origin/main".to_string()));
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
    }

    #[test]
    fn test_parse_branch_status_up_to_date() {
        let output = "* main abc1234 [origin/main] Latest commit";
        let status = Git::parse_branch_status(output).unwrap().unwrap();

        assert_eq!(status.name, "main");
        assert_eq!(status.upstream, Some("origin/main".to_string()));
        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn test_parse_branch_status_no_tracking() {
        let output = "* feature abc1234 Work in progress";
        let status = Git::parse_branch_status(output).unwrap().unwrap();

        assert_eq!(status.name, "feature");
        assert_eq!(status.upstream, None);
    }

    #[test]
    fn test_is_repo_false() {
        let temp = TempDir::new().unwrap();
        assert!(!Git::is_repo(temp.path()));
    }

    #[test]
    fn test_init_and_is_repo() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();
        assert!(Git::is_repo(temp.path()));
    }

    #[test]
    fn test_remote_operations() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        // Add remote
        Git::add_remote(temp.path(), "origin", "git@github.com:test/repo.git").unwrap();

        // List remotes
        let remotes = Git::list_remotes(temp.path()).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");

        // Get specific remote
        let origin = Git::get_remote(temp.path(), "origin").unwrap();
        assert_eq!(origin.fetch_url, "git@github.com:test/repo.git");

        // Set URL
        Git::set_remote_url(temp.path(), "origin", "git@github.com:test/new-repo.git").unwrap();
        let origin = Git::get_remote(temp.path(), "origin").unwrap();
        assert_eq!(origin.fetch_url, "git@github.com:test/new-repo.git");

        // Remove remote
        Git::remove_remote(temp.path(), "origin").unwrap();
        let remotes = Git::list_remotes(temp.path()).unwrap();
        assert!(remotes.is_empty());
    }

    #[test]
    fn test_add_remote_already_exists() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        Git::add_remote(temp.path(), "origin", "git@github.com:test/repo.git").unwrap();
        let result = Git::add_remote(temp.path(), "origin", "git@github.com:test/other.git");

        assert!(matches!(result, Err(GitError::RemoteAlreadyExists { .. })));
    }

    #[test]
    fn test_remove_remote_not_found() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        let result = Git::remove_remote(temp.path(), "nonexistent");
        assert!(matches!(result, Err(GitError::RemoteNotFound { .. })));
    }

    #[test]
    fn test_config_operations() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        // Set config
        Git::config_set(temp.path(), "user.name", "Test User").unwrap();

        // Get config
        let value = Git::config_get(temp.path(), "user.name").unwrap();
        assert_eq!(value, Some("Test User".to_string()));

        // Get non-existent config
        let value = Git::config_get(temp.path(), "nonexistent.key").unwrap();
        assert_eq!(value, None);
    }

    #[test]
    fn test_current_branch() {
        let temp = TempDir::new().unwrap();
        Git::init(temp.path()).unwrap();

        // Configure git user for commit
        Git::config_set(temp.path(), "user.email", "test@test.com").unwrap();
        Git::config_set(temp.path(), "user.name", "Test").unwrap();

        // Create initial commit to have a branch
        fs::write(temp.path().join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(temp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial"])
            .current_dir(temp.path())
            .output()
            .unwrap();

        let branch = Git::current_branch(temp.path()).unwrap();
        // Could be "main" or "master" depending on git config
        assert!(!branch.is_empty());
    }
}
