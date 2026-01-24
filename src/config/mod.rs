//! Configuration management for hyperforge

use crate::types::{Forge, Visibility};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Repository configuration (.hyperforge/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperforgeConfig {
    /// Repository name
    pub repo_name: String,

    /// Organization/user name
    pub org: Option<String>,

    /// List of forges to sync to
    pub forges: Vec<Forge>,

    /// Repository visibility
    pub visibility: Visibility,

    /// Repository description
    pub description: Option<String>,

    /// SSH key paths per forge
    #[serde(default)]
    pub ssh: HashMap<String, String>,
}

impl HyperforgeConfig {
    /// Load config from .hyperforge/config.toml
    pub fn load(_path: &Path) -> anyhow::Result<Self> {
        todo!("Load config from path")
    }

    /// Save config to .hyperforge/config.toml
    pub fn save(&self, _path: &Path) -> anyhow::Result<()> {
        todo!("Save config to path")
    }
}
