//! Core types for hyperforge

pub mod config;
pub mod repo;

use serde::{Deserialize, Serialize};

// Re-export Repo types
pub use repo::Repo;
pub use repo::RepoRecord;

// Re-export config types
pub use config::{CiConfig, ForgeConfig};

/// Supported git forges
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl Visibility {
    /// Parse a visibility string ("public" or "private"), case-insensitive.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "public" => Ok(Visibility::Public),
            "private" => Ok(Visibility::Private),
            _ => Err(format!(
                "Invalid visibility: {}. Must be public or private",
                s
            )),
        }
    }
}

/// Whether the org name refers to a user account or an organization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerType {
    User,
    Org,
}

/// Version bump type for package publishing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionBump {
    Patch,
    Minor,
    Major,
}

impl VersionBump {
    /// Parse a bump kind from an optional string, defaulting to Patch.
    pub fn from_str_or_patch(s: Option<&str>) -> Self {
        match s {
            Some("minor") => VersionBump::Minor,
            Some("major") => VersionBump::Major,
            _ => VersionBump::Patch,
        }
    }
}
