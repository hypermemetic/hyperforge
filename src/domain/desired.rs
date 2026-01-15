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

    fn github_only() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges
    }

    fn github_codeberg() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges
    }

    fn all_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges.insert(Forge::GitLab);
        forges
    }

    // ==========================================================================
    // should_exist_on() - Core business logic for determining forge presence
    // ==========================================================================

    /// Table-driven test: should_exist_on returns true only for forges in the set
    #[test]
    fn test_should_exist_on_respects_forge_set() {
        let cases = [
            // (forges, query_forge, expected)
            (github_only(), Forge::GitHub, true),
            (github_only(), Forge::Codeberg, false),
            (github_only(), Forge::GitLab, false),
            (github_codeberg(), Forge::GitHub, true),
            (github_codeberg(), Forge::Codeberg, true),
            (github_codeberg(), Forge::GitLab, false),
            (all_forges(), Forge::GitHub, true),
            (all_forges(), Forge::Codeberg, true),
            (all_forges(), Forge::GitLab, true),
        ];

        for (forges, query_forge, expected) in cases {
            let repo = DesiredRepo::new(test_identity(), Visibility::Public, forges.clone());
            assert_eq!(
                repo.should_exist_on(&query_forge),
                expected,
                "should_exist_on({:?}) with forges {:?}",
                query_forge,
                forges
            );
        }
    }

    /// marked_for_deletion overrides forge membership - repo should NOT exist anywhere
    #[test]
    fn test_should_exist_on_deletion_mark_overrides_all() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, all_forges())
            .with_deletion_mark(true);

        // Even though all forges are in the set, deletion mark means false for all
        assert!(!repo.should_exist_on(&Forge::GitHub));
        assert!(!repo.should_exist_on(&Forge::Codeberg));
        assert!(!repo.should_exist_on(&Forge::GitLab));
    }

    /// Empty forge set means repo shouldn't exist anywhere
    #[test]
    fn test_should_exist_on_empty_forges() {
        let repo = DesiredRepo::new(test_identity(), Visibility::Public, HashSet::new());

        assert!(!repo.should_exist_on(&Forge::GitHub));
        assert!(!repo.should_exist_on(&Forge::Codeberg));
        assert!(!repo.should_exist_on(&Forge::GitLab));
    }

    // ==========================================================================
    // Equality behavior with different field combinations
    // ==========================================================================

    /// Repos with same identity but different properties are not equal
    #[test]
    fn test_equality_considers_all_fields() {
        let base = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg());

        // Same everything = equal
        let same = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg());
        assert_eq!(base, same);

        // Different visibility = not equal
        let diff_visibility = DesiredRepo::new(test_identity(), Visibility::Private, github_codeberg());
        assert_ne!(base, diff_visibility);

        // Different forges = not equal
        let diff_forges = DesiredRepo::new(test_identity(), Visibility::Public, github_only());
        assert_ne!(base, diff_forges);

        // Different description = not equal
        let with_desc = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg())
            .with_description("some desc");
        assert_ne!(base, with_desc);

        // Different protected = not equal
        let protected = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg())
            .with_protected(true);
        assert_ne!(base, protected);

        // Different deletion mark = not equal
        let marked = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg())
            .with_deletion_mark(true);
        assert_ne!(base, marked);
    }

    // ==========================================================================
    // JSON serialization roundtrip
    // ==========================================================================

    /// Full roundtrip with all fields populated
    #[test]
    fn test_json_roundtrip_all_fields() {
        let original = DesiredRepo::new(test_identity(), Visibility::Private, github_codeberg())
            .with_description("Multi-forge repository manager")
            .with_protected(true)
            .with_deletion_mark(false);

        let json = serde_json::to_string(&original).unwrap();
        let restored: DesiredRepo = serde_json::from_str(&json).unwrap();

        assert_eq!(original, restored);
    }
}
