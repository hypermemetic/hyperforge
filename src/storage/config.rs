use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::types::{Forge, ForgesConfig, Visibility, SecretProvider};
use crate::error::Result;
use super::HyperforgePaths;

/// Root config.yaml structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_org: Option<String>,
    #[serde(default)]
    pub secret_provider: SecretProvider,
    pub organizations: HashMap<String, OrgConfig>,
    #[serde(default)]
    pub workspaces: HashMap<PathBuf, String>,
}

/// Organization configuration in config.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgConfig {
    pub owner: String,
    pub ssh_key: String,
    pub origin: Forge,
    /// Forges configuration - supports both legacy array format and new object format
    pub forges: ForgesConfig,
    #[serde(default)]
    pub default_visibility: Visibility,
}

impl OrgConfig {
    /// Get the SSH host alias for a forge
    /// Pattern: `<forge>-<org_name>` (e.g., `github-hypermemetic`)
    pub fn ssh_host(&self, forge: &Forge, org_name: &str) -> String {
        format!("{}-{}", forge.to_string().to_lowercase(), org_name)
    }

    /// Get the SSH URL for a repository on a forge
    /// Pattern: `git@<ssh_host>:<owner>/<repo>.git`
    pub fn ssh_url(&self, forge: &Forge, org_name: &str, repo_name: &str) -> String {
        format!("git@{}:{}/{}.git", self.ssh_host(forge, org_name), self.owner, repo_name)
    }

    /// Get the SSH URL for origin forge
    pub fn origin_url(&self, org_name: &str, repo_name: &str) -> String {
        self.ssh_url(&self.origin, org_name, repo_name)
    }
}

impl GlobalConfig {
    /// Load the global config from disk
    pub async fn load(paths: &HyperforgePaths) -> Result<Self> {
        let path = paths.config_file();
        let contents = tokio::fs::read_to_string(&path).await?;
        let mut config: GlobalConfig = serde_yaml::from_str(&contents)?;

        // Expand tildes in workspace paths at load time
        config.workspaces = config.workspaces
            .into_iter()
            .map(|(dir, org)| (expand_tilde(&dir), org))
            .collect();

        Ok(config)
    }

    /// Save the global config to disk
    pub async fn save(&self, paths: &HyperforgePaths) -> Result<()> {
        let path = paths.config_file();
        let contents = serde_yaml::to_string(self)?;
        tokio::fs::write(&path, contents).await?;
        Ok(())
    }

    /// Get an organization's configuration by name
    pub fn get_org(&self, name: &str) -> Option<&OrgConfig> {
        self.organizations.get(name)
    }

    /// Get a list of all organization names
    pub fn org_names(&self) -> Vec<String> {
        self.organizations.keys().cloned().collect()
    }

    /// Resolve which org a workspace path belongs to using longest prefix match
    pub fn resolve_workspace(&self, path: &PathBuf) -> Option<String> {
        // Try to canonicalize the input path
        let canonical = path.canonicalize().ok()?;

        // Note: workspace paths have tildes expanded at load time
        self.workspaces
            .iter()
            .filter(|(ws_dir, _)| {
                ws_dir.canonicalize()
                    .map(|p| canonical.starts_with(&p))
                    .unwrap_or(false)
            })
            .max_by_key(|(ws_dir, _)| ws_dir.components().count())
            .map(|(_, org)| org.clone())
    }
}

/// Expand tilde (~) to home directory in a directory path
fn expand_tilde(dir: &PathBuf) -> PathBuf {
    if let Some(dir_str) = dir.to_str() {
        if dir_str.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&dir_str[2..]);
            }
        } else if dir_str == "~" {
            if let Some(home) = dirs::home_dir() {
                return home;
            }
        }
    }
    dir.clone()
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_org: None,
            secret_provider: SecretProvider::default(),
            organizations: HashMap::new(),
            workspaces: HashMap::new(),
        }
    }
}
