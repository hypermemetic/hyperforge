//! Desired state types - what the user wants a repository to be.
//!
//! This module defines the desired state of a repository as declared
//! in configuration files. It represents intent, not current reality.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::types::{Forge, Visibility};
use super::RepoIdentity;

/// The desired state of a repository as declared in configuration.
///
/// This represents what the user wants the repository to look like
/// across all forges. It does not reflect current state - just intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DesiredRepo {
    /// Identity of the repository
    pub identity: RepoIdentity,
    /// Human-readable description
    pub description: Option<String>,
    /// Repository visibility (public or private)
    pub visibility: Visibility,
    /// Which forges this repo should exist on
    pub forges: HashSet<Forge>,
    /// Whether the repo is protected from deletion
    pub protected: bool,
    /// Whether the repo is marked for deletion
    pub marked_for_deletion: bool,
}

impl DesiredRepo {
    /// Create a new desired repo with minimal required fields
    pub fn new(identity: RepoIdentity, visibility: Visibility, forges: HashSet<Forge>) -> Self {
        Self {
            identity,
            description: None,
            visibility,
            forges,
            protected: false,
            marked_for_deletion: false,
        }
    }

    /// Builder method to set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder method to set protected status
    pub fn with_protected(mut self, protected: bool) -> Self {
        self.protected = protected;
        self
    }

    /// Builder method to mark for deletion
    pub fn with_deletion_mark(mut self, marked: bool) -> Self {
        self.marked_for_deletion = marked;
        self
    }

    /// Check if this repo should exist on a specific forge
    pub fn should_exist_on(&self, forge: &Forge) -> bool {
        self.forges.contains(forge) && !self.marked_for_deletion
    }

    /// Get the repository name
    pub fn name(&self) -> &str {
        &self.identity.name
    }

    /// Get the organization name
    pub fn org(&self) -> &str {
        &self.identity.org
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> RepoIdentity {
        RepoIdentity::new("hypermemetic", "hyperforge")
    }

    fn test_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges
    }

    #[test]
    fn test_desired_repo_new() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, test_forges());

        assert_eq!(repo.name(), "hyperforge");
        assert_eq!(repo.org(), "hypermemetic");
        assert_eq!(repo.visibility, Visibility::Public);
        assert!(repo.forges.contains(&Forge::GitHub));
        assert!(repo.forges.contains(&Forge::Codeberg));
        assert!(!repo.protected);
        assert!(!repo.marked_for_deletion);
    }

    #[test]
    fn test_desired_repo_builder() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Private, test_forges())
            .with_description("A test repository")
            .with_protected(true);

        assert_eq!(repo.description, Some("A test repository".to_string()));
        assert!(repo.protected);
        assert_eq!(repo.visibility, Visibility::Private);
    }

    #[test]
    fn test_should_exist_on() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, test_forges());

        assert!(repo.should_exist_on(&Forge::GitHub));
        assert!(repo.should_exist_on(&Forge::Codeberg));
        assert!(!repo.should_exist_on(&Forge::GitLab));
    }

    #[test]
    fn test_should_exist_on_with_deletion_mark() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, test_forges())
            .with_deletion_mark(true);

        // Even though it's in forges, marked_for_deletion means it shouldn't exist
        assert!(!repo.should_exist_on(&Forge::GitHub));
        assert!(!repo.should_exist_on(&Forge::Codeberg));
    }

    #[test]
    fn test_desired_repo_equality() {
        let repo1 = DesiredRepo::new(test_identity(), Visibility::Public, test_forges());
        let repo2 = DesiredRepo::new(test_identity(), Visibility::Public, test_forges());
        let repo3 = DesiredRepo::new(test_identity(), Visibility::Private, test_forges());

        assert_eq!(repo1, repo2);
        assert_ne!(repo1, repo3);
    }

    #[test]
    fn test_desired_repo_serialization() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, test_forges())
            .with_description("Test repo");

        let json = serde_json::to_string(&repo).unwrap();
        let deserialized: DesiredRepo = serde_json::from_str(&json).unwrap();

        assert_eq!(repo, deserialized);
    }
}
