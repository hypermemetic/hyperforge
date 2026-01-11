//! Git remote bridge for validating and setting up repository remotes
//!
//! This module provides functionality to check, add, and validate git remotes
//! for repositories that need to be synced to multiple forges.

use std::path::PathBuf;
use tokio::process::Command;

use crate::types::Forge;

/// Bridge to manage git remotes for a local repository
pub struct GitRemoteBridge {
    repo_path: PathBuf,
    org_name: String,
    owner: String,
}

impl GitRemoteBridge {
    /// Create a new GitRemoteBridge for a repository
    ///
    /// # Arguments
    /// * `repo_path` - Path to the local git repository
    /// * `org_name` - Organization name (used for SSH host alias)
    /// * `owner` - Owner name on the forge (e.g., GitHub username or org)
    pub fn new(repo_path: PathBuf, org_name: String, owner: String) -> Self {
        Self {
            repo_path,
            org_name,
            owner,
        }
    }

    /// List all remotes in the repository
    ///
    /// Returns a vector of (name, url) tuples for each remote.
    pub async fn list_remotes(&self) -> Result<Vec<(String, String)>, String> {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(["remote", "-v"])
            .output()
            .await
            .map_err(|e| format!("Failed to run git remote: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git remote failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut remotes = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for line in stdout.lines() {
            // Format: "origin  git@github.com:user/repo.git (fetch)"
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                let url = parts[1].to_string();
                // Only add each remote once (we get both fetch and push lines)
                if !seen.contains(&name) {
                    seen.insert(name.clone());
                    remotes.push((name, url));
                }
            }
        }

        Ok(remotes)
    }

    /// Ensure a remote exists with the given name and URL
    ///
    /// Returns `Ok(true)` if the remote was added, `Ok(false)` if it already existed.
    /// If the remote exists with a different URL, it will be updated.
    pub async fn ensure_remote(&self, name: &str, url: &str) -> Result<bool, String> {
        let remotes = self.list_remotes().await?;

        // Check if remote already exists
        if let Some((_, existing_url)) = remotes.iter().find(|(n, _)| n == name) {
            if existing_url == url {
                // Remote exists with correct URL
                return Ok(false);
            }
            // Remote exists with different URL - update it
            let output = Command::new("git")
                .current_dir(&self.repo_path)
                .args(["remote", "set-url", name, url])
                .output()
                .await
                .map_err(|e| format!("Failed to update remote: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("git remote set-url failed: {}", stderr));
            }
            return Ok(true);
        }

        // Remote doesn't exist - add it
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(["remote", "add", name, url])
            .output()
            .await
            .map_err(|e| format!("Failed to add remote: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git remote add failed: {}", stderr));
        }

        Ok(true)
    }

    /// Build the SSH URL for a forge using the org's SSH host alias
    ///
    /// Format: `git@{forge}-{org_name}:{owner}/{repo_name}.git`
    /// Example: `git@github-hypermemetic:hypermemetic/substrate.git`
    ///
    /// This uses SSH config host aliases to support multiple SSH keys per forge.
    fn build_remote_url(&self, forge: &Forge, repo_name: &str) -> String {
        format!(
            "git@{}-{}:{}/{}.git",
            forge.to_string().to_lowercase(), self.org_name, self.owner, repo_name
        )
    }

    /// Get the remote name for a forge
    ///
    /// Uses the forge's lowercase name (e.g., "github", "codeberg")
    fn remote_name(&self, forge: &Forge) -> String {
        forge.to_string()
    }

    /// Set up remotes for all configured forges
    ///
    /// Checks if remotes exist for each forge and adds any missing ones.
    /// Returns a list of remotes that were added.
    pub async fn setup_forge_remotes(
        &self,
        forges: &[Forge],
        repo_name: &str,
    ) -> Result<Vec<String>, String> {
        let mut added_remotes = Vec::new();

        for forge in forges {
            let remote_name = self.remote_name(forge);
            let url = self.build_remote_url(forge, repo_name);

            match self.ensure_remote(&remote_name, &url).await {
                Ok(true) => {
                    added_remotes.push(format!("{}={}", remote_name, url));
                }
                Ok(false) => {
                    // Remote already exists with correct URL
                }
                Err(e) => {
                    return Err(format!("Failed to setup {} remote: {}", forge, e));
                }
            }
        }

        Ok(added_remotes)
    }

    /// Check if the path is a valid git repository
    pub async fn is_git_repo(&self) -> bool {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(["rev-parse", "--git-dir"])
            .output()
            .await;

        output.map(|o| o.status.success()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_build_remote_url() {
        let bridge = GitRemoteBridge::new(
            PathBuf::from("/tmp/test"),
            "hypermemetic".to_string(),
            "shmendez".to_string(),
        );

        // Pattern: git@<forge>-<org>:<owner>/<repo>.git
        let url = bridge.build_remote_url(&Forge::GitHub, "substrate");
        assert_eq!(url, "git@github-hypermemetic:shmendez/substrate.git");

        let url = bridge.build_remote_url(&Forge::Codeberg, "dotfiles");
        assert_eq!(url, "git@codeberg-hypermemetic:shmendez/dotfiles.git");
    }

    #[test]
    fn test_remote_name() {
        let bridge = GitRemoteBridge::new(
            PathBuf::from("/tmp/test"),
            "hypermemetic".to_string(),
            "shmendez".to_string(),
        );

        assert_eq!(bridge.remote_name(&Forge::GitHub), "github");
        assert_eq!(bridge.remote_name(&Forge::Codeberg), "codeberg");
        assert_eq!(bridge.remote_name(&Forge::GitLab), "gitlab");
    }
}
