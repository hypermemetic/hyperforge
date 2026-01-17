//! Unified repository type for symmetric forge operations.
//!
//! A Repo represents a repository as it exists (or should exist) in any forge.
//! This unified type replaces the previous DesiredRepo/ObservedRepo split,
//! enabling symmetric sync operations where local is just another forge.
//!
//! # Example
//!
//! ```ignore
//! use hyperforge::domain::{Repo, RepoIdentity};
//! use hyperforge::types::Visibility;
//!
//! let repo = Repo::new(RepoIdentity::new("myorg", "myrepo"), Visibility::Public)
//!     .with_description("A great project");
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::Visibility;
use super::RepoIdentity;

/// A repository as it exists in any forge.
///
/// This unified type works for both local config and remote forges.
/// Previously this was split into DesiredRepo (intent) and ObservedRepo (reality),
/// but with LocalForge treating local as a forge, we unify them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repo {
    /// Unique identifier (org + name)
    pub identity: RepoIdentity,
    /// Human-readable description
    pub description: Option<String>,
    /// Public or private
    pub visibility: Visibility,
    /// Optional homepage URL
    pub homepage: Option<String>,
}

impl Repo {
    /// Create a new repo with required fields
    pub fn new(identity: RepoIdentity, visibility: Visibility) -> Self {
        Self {
            identity,
            description: None,
            visibility,
            homepage: None,
        }
    }

    /// Builder: set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder: set homepage
    pub fn with_homepage(mut self, homepage: impl Into<String>) -> Self {
        self.homepage = Some(homepage.into());
        self
    }

    /// Get repository name
    pub fn name(&self) -> &str {
        &self.identity.name
    }

    /// Get organization name
    pub fn org(&self) -> &str {
        &self.identity.org
    }
}

/// Convert from legacy DesiredRepo
impl From<super::DesiredRepo> for Repo {
    fn from(desired: super::DesiredRepo) -> Self {
        Self {
            identity: desired.identity,
            description: desired.description,
            visibility: desired.visibility,
            homepage: None,
        }
    }
}

/// Convert from legacy ObservedRepo (takes first forge state's properties)
impl From<super::ObservedRepo> for Repo {
    fn from(observed: super::ObservedRepo) -> Self {
        let first_state = observed.forge_states.first();
        Self {
            identity: observed.identity,
            description: first_state.and_then(|s| s.description.clone()),
            visibility: first_state
                .and_then(|s| s.visibility.clone())
                .unwrap_or(Visibility::Private),
            homepage: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_creation() {
        let repo = Repo::new(RepoIdentity::new("org", "name"), Visibility::Public);
        assert_eq!(repo.name(), "name");
        assert_eq!(repo.org(), "org");
        assert_eq!(repo.visibility, Visibility::Public);
        assert!(repo.description.is_none());
    }

    #[test]
    fn test_repo_with_description() {
        let repo = Repo::new(RepoIdentity::new("org", "name"), Visibility::Private)
            .with_description("Test repo");
        assert_eq!(repo.description, Some("Test repo".to_string()));
    }

    #[test]
    fn test_repo_with_homepage() {
        let repo = Repo::new(RepoIdentity::new("org", "name"), Visibility::Public)
            .with_homepage("https://example.com");
        assert_eq!(repo.homepage, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_repo_equality() {
        let repo1 = Repo::new(RepoIdentity::new("org", "name"), Visibility::Public);
        let repo2 = Repo::new(RepoIdentity::new("org", "name"), Visibility::Public);
        assert_eq!(repo1, repo2);

        let repo3 = Repo::new(RepoIdentity::new("org", "name"), Visibility::Private);
        assert_ne!(repo1, repo3);
    }

    #[test]
    fn test_json_roundtrip() {
        let original = Repo::new(RepoIdentity::new("org", "name"), Visibility::Private)
            .with_description("Test")
            .with_homepage("https://example.com");

        let json = serde_json::to_string(&original).unwrap();
        let restored: Repo = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }
}
