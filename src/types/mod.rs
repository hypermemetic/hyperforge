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

impl Forge {
    /// Get the lowercase string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Forge::GitHub => "github",
            Forge::Codeberg => "codeberg",
            Forge::GitLab => "gitlab",
        }
    }
}

impl std::fmt::Display for Forge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

/// Account type - user or organization
/// Affects which API endpoints are used on forges
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    /// Personal user account
    #[default]
    User,
    /// Organization account
    Org,
}

/// Version bump type for package publishing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBump {
    Patch,
    Minor,
    Major,
}
