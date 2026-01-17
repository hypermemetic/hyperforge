use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::types::{Forge, ForgesConfig, Visibility, SecretProvider};
use crate::error::Result;
use super::HyperforgePaths;

/// Raw config.yaml structure (before template resolution)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawGlobalConfig {
    pub default_org: Option<String>,
    #[serde(default)]
    pub secret_provider: SecretProvider,
    pub organizations: HashMap<String, RawOrgConfig>,
    #[serde(default)]
    pub workspaces: HashMap<PathBuf, String>,
}

/// Raw organization configuration (may have template reference)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawOrgConfig {
    /// Template org to inherit from (makes other fields optional)
    pub template: Option<String>,
    /// Owner username (required if no template)
    pub owner: Option<String>,
    /// SSH key name (required if no template)
    pub ssh_key: Option<String>,
    /// Primary forge (required if no template)
    pub origin: Option<Forge>,
    /// Forges configuration
    pub forges: Option<ForgesConfig>,
    /// Default visibility for new repos
    #[serde(default)]
    pub default_visibility: Option<Visibility>,
}

/// Root config.yaml structure (resolved)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_org: Option<String>,
    #[serde(default)]
    pub secret_provider: SecretProvider,
    pub organizations: HashMap<String, OrgConfig>,
    #[serde(default)]
    pub workspaces: HashMap<PathBuf, String>,
}

/// Organization configuration in config.yaml (resolved from template)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgConfig {
    pub owner: String,
    pub ssh_key: String,
    pub origin: Forge,
    /// Forges configuration - supports both legacy array format and new object format
    pub forges: ForgesConfig,
    #[serde(default)]
    pub default_visibility: Visibility,
    /// The org this was templated from (for credential resolution)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_source: Option<String>,
}

impl OrgConfig {
    /// Get the org name to use for credential lookup (API tokens, etc.)
    /// For sub-orgs with templates, this returns the template org name
    pub fn credential_org<'a>(&'a self, org_name: &'a str) -> &'a str {
        self.credential_source.as_deref().unwrap_or(org_name)
    }

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
        let raw: RawGlobalConfig = serde_yaml::from_str(&contents)?;

        // Two-pass resolution: first non-templated orgs, then templated
        let mut resolved_orgs: HashMap<String, OrgConfig> = HashMap::new();

        // Pass 1: Resolve orgs without templates
        for (name, raw_org) in &raw.organizations {
            if raw_org.template.is_none() {
                let org = Self::resolve_org(name, raw_org, None)?;
                resolved_orgs.insert(name.clone(), org);
            }
        }

        // Pass 2: Resolve orgs with templates
        for (name, raw_org) in &raw.organizations {
            if let Some(template_name) = &raw_org.template {
                let template = resolved_orgs.get(template_name)
                    .ok_or_else(|| crate::error::HyperforgeError::ConfigError(
                        format!("Template '{}' not found for org '{}'", template_name, name)
                    ))?;
                let org = Self::resolve_org(name, raw_org, Some(template))?;
                resolved_orgs.insert(name.clone(), org);
            }
        }

        // Expand tildes in workspace paths
        let workspaces = raw.workspaces
            .into_iter()
            .map(|(dir, org)| (expand_tilde(&dir), org))
            .collect();

        Ok(GlobalConfig {
            default_org: raw.default_org,
            secret_provider: raw.secret_provider,
            organizations: resolved_orgs,
            workspaces,
        })
    }

    /// Resolve a raw org config, optionally inheriting from a template
    fn resolve_org(name: &str, raw: &RawOrgConfig, template: Option<&OrgConfig>) -> Result<OrgConfig> {
        let (owner, ssh_key, origin, forges, default_visibility, credential_source) = match template {
            Some(t) => (
                raw.owner.clone().unwrap_or_else(|| t.owner.clone()),
                raw.ssh_key.clone().unwrap_or_else(|| t.ssh_key.clone()),
                raw.origin.clone().unwrap_or_else(|| t.origin.clone()),
                raw.forges.clone().unwrap_or_else(|| t.forges.clone()),
                raw.default_visibility.unwrap_or(t.default_visibility),
                Some(raw.template.clone().unwrap()),  // Use template name for credential lookup
            ),
            None => (
                raw.owner.clone().ok_or_else(|| crate::error::HyperforgeError::ConfigError(
                    format!("Org '{}' missing required field 'owner'", name)
                ))?,
                raw.ssh_key.clone().ok_or_else(|| crate::error::HyperforgeError::ConfigError(
                    format!("Org '{}' missing required field 'ssh_key'", name)
                ))?,
                raw.origin.clone().ok_or_else(|| crate::error::HyperforgeError::ConfigError(
                    format!("Org '{}' missing required field 'origin'", name)
                ))?,
                raw.forges.clone().ok_or_else(|| crate::error::HyperforgeError::ConfigError(
                    format!("Org '{}' missing required field 'forges'", name)
                ))?,
                raw.default_visibility.unwrap_or_default(),
                None,
            ),
        };

        Ok(OrgConfig {
            owner,
            ssh_key,
            origin,
            forges,
            default_visibility,
            credential_source,
        })
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
