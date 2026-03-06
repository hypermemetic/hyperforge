//! Org-level configuration (~/.config/hyperforge/orgs/{org}.toml)
//!
//! Stores org-wide defaults like SSH keys per forge. Per-repo config
//! can override these, but this provides a sensible default so every
//! `repo init` doesn't need `--ssh-keys`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Org-level configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgConfig {
    /// SSH key paths per forge (forge_name -> key_path)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ssh: HashMap<String, String>,
}

impl OrgConfig {
    /// Path to the org config file: ~/.config/hyperforge/orgs/{org}.toml
    pub fn config_path(config_dir: &Path, org: &str) -> PathBuf {
        config_dir.join("orgs").join(format!("{}.toml", org))
    }

    /// Load org config from disk. Returns default if file doesn't exist.
    pub fn load(config_dir: &Path, org: &str) -> Self {
        let path = Self::config_path(config_dir, org);
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save org config to disk.
    pub fn save(&self, config_dir: &Path, org: &str) -> Result<(), String> {
        let path = Self::config_path(config_dir, org);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create org config dir: {}", e))?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize org config: {}", e))?;
        std::fs::write(&path, content)
            .map_err(|e| format!("Failed to write org config: {}", e))?;
        Ok(())
    }

    /// Check if any SSH keys are configured
    pub fn has_ssh_keys(&self) -> bool {
        !self.ssh.is_empty()
    }

    /// Get SSH key for a specific forge
    pub fn ssh_key_for_forge(&self, forge: &str) -> Option<&str> {
        self.ssh.get(forge).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_missing_returns_default() {
        let tmp = TempDir::new().unwrap();
        let config = OrgConfig::load(tmp.path(), "nonexistent");
        assert!(config.ssh.is_empty());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut config = OrgConfig::default();
        config.ssh.insert("github".to_string(), "~/.ssh/gh_key".to_string());
        config.ssh.insert("codeberg".to_string(), "~/.ssh/cb_key".to_string());

        config.save(tmp.path(), "myorg").unwrap();
        let loaded = OrgConfig::load(tmp.path(), "myorg");

        assert_eq!(loaded.ssh.get("github").unwrap(), "~/.ssh/gh_key");
        assert_eq!(loaded.ssh.get("codeberg").unwrap(), "~/.ssh/cb_key");
    }

    #[test]
    fn test_has_ssh_keys() {
        let empty = OrgConfig::default();
        assert!(!empty.has_ssh_keys());

        let mut with_keys = OrgConfig::default();
        with_keys.ssh.insert("github".to_string(), "~/.ssh/key".to_string());
        assert!(with_keys.has_ssh_keys());
    }
}
