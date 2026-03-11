//! Shared configuration types used by both HyperforgeConfig (per-repo) and RepoRecord (registry)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::build_system::BuildSystemKind;

/// Runner execution mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerType {
    Local,
    Docker,
}

/// A single runner in the layered CI pipeline.
/// Runners are ordered by rigor: index 0 = quickest, higher = more thorough.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// Runner type: local (direct execution) or docker (containerized)
    #[serde(rename = "type")]
    pub runner_type: RunnerType,

    /// Build command
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<String>,

    /// Test command
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test: Vec<String>,

    /// Docker image (required for docker type, ignored for local)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// Environment variables
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Timeout in seconds (default: 300)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    300
}

/// CI/validation configuration for a repo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiConfig {
    /// Skip all CI for this repo
    #[serde(default)]
    pub skip_validate: bool,

    /// Ordered list of runners (escalating rigor)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<RunnerConfig>,
}

impl Default for CiConfig {
    fn default() -> Self {
        Self {
            skip_validate: false,
            runners: Vec::new(),
        }
    }
}

/// Generate default CI config based on detected build systems.
/// Returns layered runners appropriate for each build system kind.
pub fn default_ci_config(build_systems: &[BuildSystemKind]) -> CiConfig {
    // Use the first non-Unknown build system
    let primary = build_systems
        .iter()
        .find(|bs| **bs != BuildSystemKind::Unknown)
        .unwrap_or(&BuildSystemKind::Unknown);

    match primary {
        BuildSystemKind::Cargo => CiConfig {
            skip_validate: false,
            runners: vec![
                RunnerConfig {
                    runner_type: RunnerType::Local,
                    build: vec!["cargo".into(), "check".into()],
                    test: vec!["cargo".into(), "test".into(), "--lib".into()],
                    image: None,
                    env: HashMap::new(),
                    timeout_secs: 300,
                },
                RunnerConfig {
                    runner_type: RunnerType::Local,
                    build: vec!["cargo".into(), "build".into()],
                    test: vec!["cargo".into(), "test".into()],
                    image: None,
                    env: HashMap::new(),
                    timeout_secs: 600,
                },
                RunnerConfig {
                    runner_type: RunnerType::Docker,
                    build: vec!["cargo".into(), "build".into()],
                    test: vec!["cargo".into(), "test".into()],
                    image: Some("rust:latest".into()),
                    env: HashMap::new(),
                    timeout_secs: 900,
                },
            ],
        },
        BuildSystemKind::Cabal => CiConfig {
            skip_validate: false,
            runners: vec![
                RunnerConfig {
                    runner_type: RunnerType::Local,
                    build: vec!["cabal".into(), "build".into()],
                    test: vec!["cabal".into(), "test".into(), "all".into()],
                    image: None,
                    env: HashMap::new(),
                    timeout_secs: 600,
                },
                RunnerConfig {
                    runner_type: RunnerType::Docker,
                    build: vec!["cabal".into(), "build".into()],
                    test: vec!["cabal".into(), "test".into(), "all".into()],
                    image: Some("haskell:latest".into()),
                    env: HashMap::new(),
                    timeout_secs: 900,
                },
            ],
        },
        BuildSystemKind::Node => CiConfig {
            skip_validate: false,
            runners: vec![
                RunnerConfig {
                    runner_type: RunnerType::Local,
                    build: vec!["npm".into(), "run".into(), "build".into()],
                    test: vec!["npm".into(), "test".into()],
                    image: None,
                    env: HashMap::new(),
                    timeout_secs: 300,
                },
                RunnerConfig {
                    runner_type: RunnerType::Docker,
                    build: vec![
                        "sh".into(),
                        "-c".into(),
                        "npm install && npm run build".into(),
                    ],
                    test: vec!["npm".into(), "test".into()],
                    image: Some("node:lts".into()),
                    env: HashMap::new(),
                    timeout_secs: 600,
                },
            ],
        },
        BuildSystemKind::Unknown => CiConfig {
            skip_validate: true,
            runners: Vec::new(),
        },
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
