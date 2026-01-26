//! Core types for hyperforge

pub mod repo;

use serde::{Deserialize, Serialize};

// Re-export Repo type
pub use repo::Repo;

/// Supported git forges
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    Codeberg,
    GitLab,
}

/// Repository visibility
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Private,
}

/// Version bump type for package publishing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionBump {
    Patch,
    Minor,
    Major,
}
