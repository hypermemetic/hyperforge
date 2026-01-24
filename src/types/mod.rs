//! Core types for hyperforge

use serde::{Deserialize, Serialize};

/// Supported git forges
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    Codeberg,
    GitLab,
}

/// Repository visibility
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
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
