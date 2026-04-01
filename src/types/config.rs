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

/// Resolve CI config for a repo: use existing config if present, otherwise generate defaults.
///
/// This is the single entry point for CI config resolution. Used by:
/// - `build init_configs` to persist defaults to disk
/// - `build run` at runtime to determine what runners to execute
/// - `workspace init` Phase 4 to inject CI into newly-initialized repos
pub fn resolve_ci_config(
    existing: Option<&CiConfig>,
    build_systems: &[BuildSystemKind],
) -> CiConfig {
    if let Some(ci) = existing {
        return ci.clone();
    }
    default_ci_config(build_systems)
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
                    build: vec!["npm install && npm run build".into()],
                    test: vec!["npm".into(), "test".into()],
                    image: None,
                    env: HashMap::new(),
                    timeout_secs: 300,
                },
                RunnerConfig {
                    runner_type: RunnerType::Docker,
                    build: vec!["npm install && npm run build".into()],
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

/// Distribution channel for binary releases
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DistChannel {
    ForgeRelease,
    CratesIo,
    Hackage,
    Brew,
    Ghcr,
    Binstall,
}

impl std::fmt::Display for DistChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForgeRelease => write!(f, "forge-release"),
            Self::CratesIo => write!(f, "crates-io"),
            Self::Hackage => write!(f, "hackage"),
            Self::Brew => write!(f, "brew"),
            Self::Ghcr => write!(f, "ghcr"),
            Self::Binstall => write!(f, "binstall"),
        }
    }
}

/// Per-repo distribution configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DistConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<DistChannel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brew_tap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brew_tap_path: Option<String>,
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
