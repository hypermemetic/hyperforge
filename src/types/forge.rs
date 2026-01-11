use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Sync policy determines how repositories are synchronized across forges
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncPolicy {
    /// All forges with sync: true receive the same repos (default)
    #[default]
    Mirror,
    /// Only origin forge is authoritative, other forges are read-only
    Primary,
    /// No automatic sync - sync command becomes a no-op
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    Codeberg,
    GitLab,
}

/// Per-forge configuration options
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeConfig {
    /// Whether to sync repositories to this forge (default: true)
    #[serde(default = "default_sync")]
    pub sync: bool,
}

fn default_sync() -> bool {
    true
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self { sync: true }
    }
}

/// Forges configuration that supports both legacy array format and new object format.
///
/// Legacy format (array of forge names):
/// ```yaml
/// forges:
///   - github
///   - codeberg
/// ```
///
/// New format (object with per-forge config):
/// ```yaml
/// forges:
///   github:
///     sync: true
///   codeberg:
///     sync: false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ForgesConfig {
    /// New object format: map of forge -> config
    Object(HashMap<Forge, ForgeConfig>),
    /// Legacy array format: list of forge names (all treated as sync: true)
    Legacy(Vec<Forge>),
}

impl ForgesConfig {
    /// Get all forge names, regardless of sync status
    pub fn all_forges(&self) -> Vec<Forge> {
        match self {
            ForgesConfig::Object(map) => map.keys().cloned().collect(),
            ForgesConfig::Legacy(list) => list.clone(),
        }
    }

    /// Get only forges that should be synced (sync: true)
    pub fn synced_forges(&self) -> Vec<Forge> {
        match self {
            ForgesConfig::Object(map) => map
                .iter()
                .filter(|(_, config)| config.sync)
                .map(|(forge, _)| forge.clone())
                .collect(),
            // Legacy format treats all forges as sync: true
            ForgesConfig::Legacy(list) => list.clone(),
        }
    }

    /// Check if a specific forge should be synced
    pub fn is_synced(&self, forge: &Forge) -> bool {
        match self {
            ForgesConfig::Object(map) => map.get(forge).map(|c| c.sync).unwrap_or(false),
            ForgesConfig::Legacy(list) => list.contains(forge),
        }
    }

    /// Get the config for a specific forge
    pub fn get(&self, forge: &Forge) -> Option<ForgeConfig> {
        match self {
            ForgesConfig::Object(map) => map.get(forge).cloned(),
            ForgesConfig::Legacy(list) => {
                if list.contains(forge) {
                    Some(ForgeConfig::default())
                } else {
                    None
                }
            }
        }
    }

    /// Check if this config contains a forge
    pub fn contains(&self, forge: &Forge) -> bool {
        match self {
            ForgesConfig::Object(map) => map.contains_key(forge),
            ForgesConfig::Legacy(list) => list.contains(forge),
        }
    }

    /// Convert to the new object format (for migration)
    pub fn to_object_format(&self) -> HashMap<Forge, ForgeConfig> {
        match self {
            ForgesConfig::Object(map) => map.clone(),
            ForgesConfig::Legacy(list) => list
                .iter()
                .map(|f| (f.clone(), ForgeConfig::default()))
                .collect(),
        }
    }

    /// Create from a list of forges (legacy compatibility)
    pub fn from_forges(forges: Vec<Forge>) -> Self {
        ForgesConfig::Legacy(forges)
    }

    /// Create with explicit sync settings
    pub fn from_map(map: HashMap<Forge, ForgeConfig>) -> Self {
        ForgesConfig::Object(map)
    }

    /// Iterate over forges (all forges, not just synced)
    pub fn iter(&self) -> impl Iterator<Item = &Forge> {
        match self {
            ForgesConfig::Object(map) => {
                let forges: Vec<_> = map.keys().collect();
                forges.into_iter()
            }
            ForgesConfig::Legacy(list) => {
                let forges: Vec<_> = list.iter().collect();
                forges.into_iter()
            }
        }
    }
}

impl Default for ForgesConfig {
    fn default() -> Self {
        ForgesConfig::Legacy(vec![])
    }
}

impl Forge {
    pub fn api_base(&self) -> &'static str {
        match self {
            Forge::GitHub => "https://api.github.com",
            Forge::Codeberg => "https://codeberg.org/api/v1",
            Forge::GitLab => "https://gitlab.com/api/v4",
        }
    }

    pub fn ssh_host(&self) -> &'static str {
        match self {
            Forge::GitHub => "github.com",
            Forge::Codeberg => "codeberg.org",
            Forge::GitLab => "gitlab.com",
        }
    }
}

impl std::fmt::Display for Forge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Forge::GitHub => write!(f, "github"),
            Forge::Codeberg => write!(f, "codeberg"),
            Forge::GitLab => write!(f, "gitlab"),
        }
    }
}

impl std::str::FromStr for Forge {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "github" => Ok(Forge::GitHub),
            "codeberg" => Ok(Forge::Codeberg),
            "gitlab" => Ok(Forge::GitLab),
            _ => Err(format!("Unknown forge: {}", s)),
        }
    }
}
