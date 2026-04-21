//! Core types for hyperforge

pub mod config;
pub mod registry;
pub mod repo;

use serde::{Deserialize, Serialize};

// Re-export Repo types
pub use repo::Repo;
pub use repo::RepoRecord;

// Re-export config types
pub use config::{CiConfig, DistChannel, DistConfig, ForgeConfig};

// Re-export registry types
pub use registry::{ContainerRegistry, ImageRef, RegistryAuth};

/// Supported git forges
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    Codeberg,
    GitLab,
}

impl Forge {
    /// Return the lowercase string representation used in config files and adapters.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::Codeberg => "codeberg",
            Self::GitLab => "gitlab",
        }
    }
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
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            _ => Err(format!(
                "Invalid visibility: {s}. Must be public or private"
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
            Some("minor") => Self::Minor,
            Some("major") => Self::Major,
            _ => Self::Patch,
        }
    }
}
