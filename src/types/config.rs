//! Shared configuration types used by both HyperforgeConfig (per-repo) and RepoRecord (registry)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
