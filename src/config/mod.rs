//! Configuration management for hyperforge
//!
//! This module handles `.hyperforge/config.toml` files which store
//! per-repository forge configuration.

use crate::types::{Forge, Visibility};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration directory name
pub const CONFIG_DIR: &str = ".hyperforge";

/// Configuration file name
pub const CONFIG_FILE: &str = "config.toml";

/// Errors that can occur during config operations
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config not found at {path}")]
    NotFound { path: PathBuf },

    #[error("Failed to read config: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Failed to serialize config: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("Config already exists at {path}")]
    AlreadyExists { path: PathBuf },

    #[error("Invalid config: {message}")]
    Invalid { message: String },
}

pub type ConfigResult<T> = Result<T, ConfigError>;

/// CI/validation configuration for a repo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiConfig {
    /// Path to Dockerfile for containerized builds (relative to repo root)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,

    /// Build command (default: inferred from build system)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<String>,

    /// Test command (default: inferred from build system)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test: Vec<String>,

    /// Skip validation for this repo
    #[serde(default)]
    pub skip_validate: bool,

    /// Timeout in seconds for validation steps
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Environment variables for CI
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

fn default_timeout() -> u64 {
    300
}

impl Default for CiConfig {
    fn default() -> Self {
        Self {
            dockerfile: None,
            build: Vec::new(),
            test: Vec::new(),
            skip_validate: false,
            timeout_secs: 300,
            env: HashMap::new(),
        }
    }
}

/// Per-forge configuration overrides
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgeConfig {
    /// Override organization for this forge
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,

    /// Git remote name for this forge
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// Repository configuration (.hyperforge/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperforgeConfig {
    /// Repository name (inferred from directory if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,

    /// Default organization/user name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,

    /// List of forges to sync to
    #[serde(default)]
    pub forges: Vec<String>,

    /// Repository visibility
    #[serde(default)]
    pub visibility: Visibility,

    /// Repository description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// SSH key paths per forge
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ssh: HashMap<String, String>,

    /// Per-forge configuration overrides
    #[serde(default, rename = "forge", skip_serializing_if = "HashMap::is_empty")]
    pub forge_config: HashMap<String, ForgeConfig>,

    /// Default branch name (defaults to "main" if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,

    /// CI/validation configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiConfig>,
}

impl Default for HyperforgeConfig {
    fn default() -> Self {
        Self {
            repo_name: None,
            org: None,
            forges: vec!["github".to_string()],
            visibility: Visibility::Public,
            description: None,
            ssh: HashMap::new(),
            forge_config: HashMap::new(),
            default_branch: None,
            ci: None,
        }
    }
}

impl HyperforgeConfig {
    /// Create a new config with specified forges
    pub fn new(forges: Vec<String>) -> Self {
        Self {
            forges,
            ..Default::default()
        }
    }

    /// Builder method: set organization
    pub fn with_org(mut self, org: impl Into<String>) -> Self {
        self.org = Some(org.into());
        self
    }

    /// Builder method: set repo name
    pub fn with_repo_name(mut self, name: impl Into<String>) -> Self {
        self.repo_name = Some(name.into());
        self
    }

    /// Builder method: set visibility
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Builder method: set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder method: set default branch
    pub fn with_default_branch(mut self, branch: impl Into<String>) -> Self {
        self.default_branch = Some(branch.into());
        self
    }

    /// Get the effective default branch (falls back to "main")
    pub fn effective_default_branch(&self) -> &str {
        self.default_branch.as_deref().unwrap_or("main")
    }

    /// Builder method: add SSH key for a forge
    pub fn with_ssh_key(mut self, forge: impl Into<String>, key_path: impl Into<String>) -> Self {
        self.ssh.insert(forge.into(), key_path.into());
        self
    }

    /// Get the config directory path for a repo
    pub fn config_dir(repo_path: &Path) -> PathBuf {
        repo_path.join(CONFIG_DIR)
    }

    /// Get the config file path for a repo
    pub fn config_path(repo_path: &Path) -> PathBuf {
        Self::config_dir(repo_path).join(CONFIG_FILE)
    }

    /// Check if a hyperforge config exists at the given path
    pub fn exists(repo_path: &Path) -> bool {
        Self::config_path(repo_path).exists()
    }

    /// Load config from .hyperforge/config.toml in the given repo
    pub fn load(repo_path: &Path) -> ConfigResult<Self> {
        let config_path = Self::config_path(repo_path);

        if !config_path.exists() {
            return Err(ConfigError::NotFound { path: config_path });
        }

        let content = fs::read_to_string(&config_path)?;
        let config: Self = toml::from_str(&content)?;

        Ok(config)
    }

    /// Save config to .hyperforge/config.toml in the given repo
    pub fn save(&self, repo_path: &Path) -> ConfigResult<()> {
        let config_dir = Self::config_dir(repo_path);
        let config_path = Self::config_path(repo_path);

        // Create .hyperforge directory if it doesn't exist
        fs::create_dir_all(&config_dir)?;

        let content = toml::to_string_pretty(self)?;
        fs::write(&config_path, content)?;

        Ok(())
    }

    /// Get the effective org for a forge (checks forge-specific override first)
    pub fn org_for_forge(&self, forge: &str) -> Option<&str> {
        // Check forge-specific override first
        if let Some(forge_config) = self.forge_config.get(forge) {
            if let Some(ref org) = forge_config.org {
                return Some(org);
            }
        }

        // Fall back to default org
        self.org.as_deref()
    }

    /// Get the remote name for a forge
    pub fn remote_for_forge(&self, forge: &str) -> String {
        // Check forge-specific override
        if let Some(forge_config) = self.forge_config.get(forge) {
            if let Some(ref remote) = forge_config.remote {
                return remote.clone();
            }
        }

        // Default: first forge is "origin", others use forge name
        if self.forges.first().map(|f| f.as_str()) == Some(forge) {
            "origin".to_string()
        } else {
            forge.to_string()
        }
    }

    /// Get SSH key path for a forge
    pub fn ssh_key_for_forge(&self, forge: &str) -> Option<&str> {
        self.ssh.get(forge).map(|s| s.as_str())
    }

    /// Get the repo name (explicit or from path)
    pub fn get_repo_name(&self, repo_path: &Path) -> String {
        self.repo_name
            .clone()
            .or_else(|| {
                repo_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Parse forge string to Forge enum
    pub fn parse_forge(forge: &str) -> Option<Forge> {
        match forge.to_lowercase().as_str() {
            "github" => Some(Forge::GitHub),
            "codeberg" => Some(Forge::Codeberg),
            "gitlab" => Some(Forge::GitLab),
            _ => None,
        }
    }

    /// Validate the config
    pub fn validate(&self) -> ConfigResult<()> {
        if self.forges.is_empty() {
            return Err(ConfigError::Invalid {
                message: "At least one forge must be specified".to_string(),
            });
        }

        // Validate forge names
        for forge in &self.forges {
            if Self::parse_forge(forge).is_none() {
                return Err(ConfigError::Invalid {
                    message: format!(
                        "Unknown forge: {}. Valid forges: github, codeberg, gitlab",
                        forge
                    ),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = HyperforgeConfig::default();
        assert_eq!(config.forges, vec!["github"]);
        assert_eq!(config.visibility, Visibility::Public);
    }

    #[test]
    fn test_config_builder() {
        let config = HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()])
            .with_org("alice")
            .with_repo_name("my-tool")
            .with_visibility(Visibility::Private)
            .with_ssh_key("github", "~/.ssh/github_key");

        assert_eq!(config.forges, vec!["github", "codeberg"]);
        assert_eq!(config.org, Some("alice".to_string()));
        assert_eq!(config.repo_name, Some("my-tool".to_string()));
        assert_eq!(config.visibility, Visibility::Private);
        assert_eq!(
            config.ssh.get("github"),
            Some(&"~/.ssh/github_key".to_string())
        );
    }

    #[test]
    fn test_config_save_load() {
        let temp = TempDir::new().unwrap();

        let config = HyperforgeConfig::new(vec!["github".to_string()])
            .with_org("alice")
            .with_repo_name("test-repo");

        config.save(temp.path()).unwrap();

        // Verify file exists
        assert!(HyperforgeConfig::exists(temp.path()));

        // Load it back
        let loaded = HyperforgeConfig::load(temp.path()).unwrap();
        assert_eq!(loaded.org, Some("alice".to_string()));
        assert_eq!(loaded.repo_name, Some("test-repo".to_string()));
        assert_eq!(loaded.forges, vec!["github"]);
    }

    #[test]
    fn test_config_not_found() {
        let temp = TempDir::new().unwrap();
        let result = HyperforgeConfig::load(temp.path());
        assert!(matches!(result, Err(ConfigError::NotFound { .. })));
    }

    #[test]
    fn test_org_for_forge_default() {
        let config = HyperforgeConfig::new(vec!["github".to_string()]).with_org("default-org");

        assert_eq!(config.org_for_forge("github"), Some("default-org"));
        assert_eq!(config.org_for_forge("codeberg"), Some("default-org"));
    }

    #[test]
    fn test_org_for_forge_override() {
        let mut config =
            HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()])
                .with_org("default-org");

        config.forge_config.insert(
            "codeberg".to_string(),
            ForgeConfig {
                org: Some("codeberg-org".to_string()),
                remote: None,
            },
        );

        assert_eq!(config.org_for_forge("github"), Some("default-org"));
        assert_eq!(config.org_for_forge("codeberg"), Some("codeberg-org"));
    }

    #[test]
    fn test_remote_for_forge() {
        let config =
            HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()]);

        // First forge is "origin"
        assert_eq!(config.remote_for_forge("github"), "origin");
        // Others use forge name
        assert_eq!(config.remote_for_forge("codeberg"), "codeberg");
    }

    #[test]
    fn test_get_repo_name_explicit() {
        let config = HyperforgeConfig::default().with_repo_name("explicit-name");
        let temp = TempDir::new().unwrap();

        assert_eq!(config.get_repo_name(temp.path()), "explicit-name");
    }

    #[test]
    fn test_get_repo_name_from_path() {
        let config = HyperforgeConfig::default();
        let path = Path::new("/home/user/projects/my-project");

        assert_eq!(config.get_repo_name(path), "my-project");
    }

    #[test]
    fn test_validate_empty_forges() {
        let config = HyperforgeConfig {
            forges: vec![],
            ..Default::default()
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn test_validate_unknown_forge() {
        let config = HyperforgeConfig::new(vec!["unknown-forge".to_string()]);

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn test_validate_valid() {
        let config =
            HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()]);

        config.validate().unwrap();
    }

    #[test]
    fn test_toml_roundtrip() {
        let mut config =
            HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()])
                .with_org("alice")
                .with_repo_name("my-tool")
                .with_visibility(Visibility::Private)
                .with_description("A cool tool")
                .with_ssh_key("github", "~/.ssh/github_key");

        config.forge_config.insert(
            "codeberg".to_string(),
            ForgeConfig {
                org: Some("different-org".to_string()),
                remote: Some("cb".to_string()),
            },
        );

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: HyperforgeConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.org, config.org);
        assert_eq!(parsed.repo_name, config.repo_name);
        assert_eq!(parsed.forges, config.forges);
        assert_eq!(parsed.visibility, config.visibility);
    }
}
