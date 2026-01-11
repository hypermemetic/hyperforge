//! SSH config bridge for managing Host entries
//!
//! This module manages ~/.ssh/config entries for hyperforge organizations.
//! Each org gets Host entries for each forge with the format:
//! `Host {hostname}-{org_name}` pointing to the org's SSH key.

use std::path::PathBuf;
use tokio::fs;

use crate::types::Forge;

/// Marker comment used to identify hyperforge-managed blocks
const HYPERFORGE_MARKER: &str = "# hyperforge:";

/// Bridge to manage SSH config Host entries for organizations
pub struct SshConfigBridge {
    config_path: PathBuf,
}

/// A Host entry in SSH config
#[derive(Debug, Clone)]
struct HostEntry {
    host_alias: String,
    hostname: String,
    user: String,
    identity_file: String,
}

impl HostEntry {
    fn to_config_block(&self, org_name: &str) -> String {
        format!(
            "{} {}\nHost {}\n    HostName {}\n    User {}\n    IdentityFile {}\n",
            HYPERFORGE_MARKER,
            org_name,
            self.host_alias,
            self.hostname,
            self.user,
            self.identity_file
        )
    }
}

impl SshConfigBridge {
    /// Create a new SshConfigBridge
    ///
    /// Uses ~/.ssh/config by default
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let config_path = PathBuf::from(home).join(".ssh/config");
        Self { config_path }
    }

    /// Create with a custom config path (useful for testing)
    pub fn with_path(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    /// Update SSH config with Host entries for an organization
    ///
    /// This will:
    /// 1. Remove any existing entries for this org
    /// 2. Add new entries for each forge
    ///
    /// Returns the list of host aliases that were added.
    pub async fn update_org(
        &self,
        org_name: &str,
        ssh_key: &str,
        forges: &[Forge],
    ) -> Result<Vec<String>, String> {
        // Ensure .ssh directory exists
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create .ssh directory: {}", e))?;
        }

        // Read existing config or start fresh
        let existing_content = fs::read_to_string(&self.config_path)
            .await
            .unwrap_or_default();

        // Remove existing entries for this org
        let cleaned_content = self.remove_org_entries(&existing_content, org_name);

        // Build new entries
        let mut hosts = Vec::new();
        let mut new_entries = String::new();

        // Resolve SSH key path
        let identity_file = if ssh_key.starts_with('/') || ssh_key.starts_with('~') {
            ssh_key.to_string()
        } else {
            format!("~/.ssh/{}", ssh_key)
        };

        for forge in forges {
            let hostname = forge.ssh_host();
            let host_alias = format!("{}-{}", hostname, org_name);

            let entry = HostEntry {
                host_alias: host_alias.clone(),
                hostname: hostname.to_string(),
                user: "git".to_string(),
                identity_file: identity_file.clone(),
            };

            new_entries.push_str(&entry.to_config_block(org_name));
            new_entries.push('\n');
            hosts.push(host_alias);
        }

        // Combine: existing (cleaned) + new entries
        let final_content = if cleaned_content.is_empty() {
            new_entries
        } else {
            format!("{}\n{}", cleaned_content.trim_end(), new_entries)
        };

        // Write back
        fs::write(&self.config_path, final_content)
            .await
            .map_err(|e| format!("Failed to write SSH config: {}", e))?;

        Ok(hosts)
    }

    /// Remove all Host entries for an organization from SSH config
    pub async fn remove_org(&self, org_name: &str) -> Result<(), String> {
        let existing_content = match fs::read_to_string(&self.config_path).await {
            Ok(content) => content,
            Err(_) => return Ok(()), // No config file, nothing to remove
        };

        let cleaned_content = self.remove_org_entries(&existing_content, org_name);

        fs::write(&self.config_path, cleaned_content)
            .await
            .map_err(|e| format!("Failed to write SSH config: {}", e))?;

        Ok(())
    }

    /// Remove hyperforge entries for a specific org from config content
    fn remove_org_entries(&self, content: &str, org_name: &str) -> String {
        let marker = format!("{} {}", HYPERFORGE_MARKER, org_name);
        let mut result = Vec::new();
        let mut skip_until_next_host = false;

        for line in content.lines() {
            if line.starts_with(&marker) {
                // Start skipping this hyperforge block
                skip_until_next_host = true;
                continue;
            }

            if skip_until_next_host {
                // Check if we've hit a new Host or another hyperforge marker
                if line.starts_with("Host ") || line.starts_with(HYPERFORGE_MARKER) {
                    skip_until_next_host = false;
                    // If it's a hyperforge marker for a different org, keep it
                    if line.starts_with(HYPERFORGE_MARKER) && !line.starts_with(&marker) {
                        result.push(line);
                    } else if line.starts_with("Host ") {
                        result.push(line);
                    }
                }
                // Otherwise continue skipping
                continue;
            }

            result.push(line);
        }

        // Join and clean up excessive blank lines
        let joined = result.join("\n");
        self.clean_blank_lines(&joined)
    }

    /// Clean up excessive blank lines (more than 2 consecutive)
    fn clean_blank_lines(&self, content: &str) -> String {
        let mut result = String::new();
        let mut blank_count = 0;

        for line in content.lines() {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count <= 2 {
                    result.push('\n');
                }
            } else {
                blank_count = 0;
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str(line);
            }
        }

        // Ensure single trailing newline
        let trimmed = result.trim_end();
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("{}\n", trimmed)
        }
    }

    /// Get the list of host aliases for an organization
    pub fn get_host_aliases(org_name: &str, forges: &[Forge]) -> Vec<String> {
        forges
            .iter()
            .map(|forge| format!("{}-{}", forge.ssh_host(), org_name))
            .collect()
    }
}

impl Default for SshConfigBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_org_entries() {
        let bridge = SshConfigBridge::new();

        let content = r#"# Some existing config
Host existing-host
    HostName example.com
    User user

# hyperforge: test-org
Host github.com-test-org
    HostName github.com
    User git
    IdentityFile ~/.ssh/test-org

# hyperforge: other-org
Host github.com-other-org
    HostName github.com
    User git
    IdentityFile ~/.ssh/other-org
"#;

        let result = bridge.remove_org_entries(content, "test-org");

        assert!(!result.contains("test-org"));
        assert!(result.contains("other-org"));
        assert!(result.contains("existing-host"));
    }

    #[test]
    fn test_host_entry_format() {
        let entry = HostEntry {
            host_alias: "github.com-myorg".to_string(),
            hostname: "github.com".to_string(),
            user: "git".to_string(),
            identity_file: "~/.ssh/myorg".to_string(),
        };

        let block = entry.to_config_block("myorg");

        assert!(block.contains("# hyperforge: myorg"));
        assert!(block.contains("Host github.com-myorg"));
        assert!(block.contains("HostName github.com"));
        assert!(block.contains("User git"));
        assert!(block.contains("IdentityFile ~/.ssh/myorg"));
    }

    #[test]
    fn test_get_host_aliases() {
        let aliases = SshConfigBridge::get_host_aliases(
            "myorg",
            &[Forge::GitHub, Forge::Codeberg],
        );

        assert_eq!(aliases, vec!["github.com-myorg", "codeberg.org-myorg"]);
    }
}
