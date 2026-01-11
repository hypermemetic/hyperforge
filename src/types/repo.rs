use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

use super::{Forge, Visibility};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Repo {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub forges: Vec<Forge>,
    #[serde(default)]
    pub protected: bool,
    #[serde(default, rename = "_delete")]
    pub marked_for_deletion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoSummary {
    pub name: String,
    pub visibility: Visibility,
    pub forges: Vec<Forge>,
    pub synced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoDetails {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub forge_urls: HashMap<Forge, String>,
}

/// Configuration file format for repos.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReposConfig {
    pub owner: String,
    pub repos: HashMap<String, RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub description: Option<String>,
    pub visibility: Option<Visibility>,
    pub forges: Option<Vec<Forge>>,
    #[serde(default)]
    pub protected: bool,
    #[serde(default, rename = "_delete")]
    pub delete: bool,

    // System-managed state (prefixed with _)
    #[serde(default, rename = "_synced", skip_serializing_if = "Option::is_none")]
    pub synced: Option<SyncedState>,

    #[serde(default, rename = "_discovered", skip_serializing_if = "Option::is_none")]
    pub discovered: Option<DiscoveredState>,
}

/// State synced via Pulumi (outputs captured)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SyncedState {
    #[serde(flatten)]
    pub forges: HashMap<Forge, ForgeSyncedState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeSyncedState {
    pub url: String,
    pub id: Option<String>,
    pub synced_at: DateTime<Utc>,
}

/// State discovered via forge API queries
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DiscoveredState {
    #[serde(flatten)]
    pub forges: HashMap<Forge, ForgeDiscoveredState>,
    pub last_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeDiscoveredState {
    pub exists: bool,
    pub url: Option<String>,
    pub id: Option<String>,
}
