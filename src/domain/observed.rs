//! Observed state types - what actually exists on forges.
//!
//! This module defines types that represent the current state of repositories
//! as observed from forge APIs. This is reality, not intent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{Forge, Visibility};
use super::RepoIdentity;

/// Observed state of a repository on a specific forge.
///
/// This represents what actually exists on a forge, as discovered
/// through API queries. It may differ from desired state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForgeRepoState {
    /// The forge this state is from
    pub forge: Forge,
    /// Whether the repo exists on this forge
    pub exists: bool,
    /// The URL on the forge (if it exists)
    pub url: Option<String>,
    /// The forge-specific ID (if it exists)
    pub forge_id: Option<String>,
    /// Current visibility on this forge
    pub visibility: Option<Visibility>,
    /// Current description on this forge
    pub description: Option<String>,
}

impl ForgeRepoState {
    /// Create a state representing a non-existent repo on a forge
    pub fn not_found(forge: Forge) -> Self {
        Self {
            forge,
            exists: false,
            url: None,
            forge_id: None,
            visibility: None,
            description: None,
        }
    }

    /// Create a state representing an existing repo on a forge
    pub fn found(
        forge: Forge,
        url: String,
        visibility: Visibility,
        forge_id: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            forge,
            exists: true,
            url: Some(url),
            forge_id,
            visibility: Some(visibility),
            description,
        }
    }
}

/// The complete observed state of a repository across all forges.
///
/// This aggregates ForgeRepoState for each forge where we have
/// queried for the repository's existence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservedRepo {
    /// Identity of the repository
    pub identity: RepoIdentity,
    /// State on each forge (keyed by forge)
    pub forge_states: Vec<ForgeRepoState>,
}

impl ObservedRepo {
    /// Create a new observed repo with no forge states
    pub fn new(identity: RepoIdentity) -> Self {
        Self {
            identity,
            forge_states: Vec::new(),
        }
    }

    /// Add an observed state for a forge
    pub fn with_forge_state(mut self, state: ForgeRepoState) -> Self {
        // Remove any existing state for this forge
        self.forge_states.retain(|s| s.forge != state.forge);
        self.forge_states.push(state);
        self
    }

    /// Get the state for a specific forge
    pub fn get_forge_state(&self, forge: &Forge) -> Option<&ForgeRepoState> {
        self.forge_states.iter().find(|s| &s.forge == forge)
    }

    /// Check if the repo exists on a specific forge
    pub fn exists_on(&self, forge: &Forge) -> bool {
        self.get_forge_state(forge)
            .map(|s| s.exists)
            .unwrap_or(false)
    }

    /// Get all forges where the repo exists
    pub fn existing_forges(&self) -> Vec<Forge> {
        self.forge_states
            .iter()
            .filter(|s| s.exists)
            .map(|s| s.forge.clone())
            .collect()
    }

    /// Check if this repo exists on any forge
    pub fn exists_anywhere(&self) -> bool {
        self.forge_states.iter().any(|s| s.exists)
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

    fn github_found() -> ForgeRepoState {
        ForgeRepoState::found(
            Forge::GitHub,
            "https://github.com/hypermemetic/hyperforge".to_string(),
            Visibility::Public,
            Some("gh-123".to_string()),
            Some("Description".to_string()),
        )
    }

    fn codeberg_found() -> ForgeRepoState {
        ForgeRepoState::found(
            Forge::Codeberg,
            "https://codeberg.org/hypermemetic/hyperforge".to_string(),
            Visibility::Public,
            Some("cb-456".to_string()),
            None,
        )
    }

    // ==========================================================================
    // exists_on() - Check if repo exists on a specific forge
    // ==========================================================================

    /// exists_on returns true only when forge state exists AND is marked as existing
    #[test]
    fn test_exists_on_scenarios() {
        // No forge states at all
        let empty = ObservedRepo::new(test_identity());
        assert!(!empty.exists_on(&Forge::GitHub));
        assert!(!empty.exists_on(&Forge::Codeberg));

        // Has GitHub found, Codeberg not found
        let mixed = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg));

        assert!(mixed.exists_on(&Forge::GitHub));
        assert!(!mixed.exists_on(&Forge::Codeberg));
        assert!(!mixed.exists_on(&Forge::GitLab)); // No state at all

        // Both found
        let both = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(codeberg_found());

        assert!(both.exists_on(&Forge::GitHub));
        assert!(both.exists_on(&Forge::Codeberg));
        assert!(!both.exists_on(&Forge::GitLab));
    }

    // ==========================================================================
    // exists_anywhere() - Check if repo exists on any forge
    // ==========================================================================

    #[test]
    fn test_exists_anywhere() {
        // No states = doesn't exist anywhere
        let empty = ObservedRepo::new(test_identity());
        assert!(!empty.exists_anywhere());

        // Only not_found states = doesn't exist anywhere
        let all_not_found = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitHub))
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg));
        assert!(!all_not_found.exists_anywhere());

        // At least one found = exists somewhere
        let one_found = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitHub))
            .with_forge_state(codeberg_found());
        assert!(one_found.exists_anywhere());
    }

    // ==========================================================================
    // existing_forges() - List all forges where repo exists
    // ==========================================================================

    #[test]
    fn test_existing_forges_returns_only_existing() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg))
            .with_forge_state(ForgeRepoState::found(
                Forge::GitLab,
                "https://gitlab.com/hypermemetic/hyperforge".to_string(),
                Visibility::Private,
                None,
                None,
            ));

        let existing = repo.existing_forges();
        assert_eq!(existing.len(), 2);
        assert!(existing.contains(&Forge::GitHub));
        assert!(existing.contains(&Forge::GitLab));
        assert!(!existing.contains(&Forge::Codeberg));
    }

    #[test]
    fn test_existing_forges_empty_when_none_exist() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitHub))
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg));

        assert!(repo.existing_forges().is_empty());
    }

    // ==========================================================================
    // with_forge_state() - State replacement behavior
    // ==========================================================================

    /// Adding a state for the same forge replaces the previous state
    #[test]
    fn test_with_forge_state_replaces_existing() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitHub))
            .with_forge_state(github_found()); // Replaces the not_found

        // Should only have one GitHub entry
        let github_count = repo.forge_states.iter()
            .filter(|s| s.forge == Forge::GitHub)
            .count();
        assert_eq!(github_count, 1);

        // And it should be the "found" one
        assert!(repo.exists_on(&Forge::GitHub));
    }

    /// Adding states for different forges accumulates
    #[test]
    fn test_with_forge_state_accumulates_different_forges() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(codeberg_found())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitLab));

        assert_eq!(repo.forge_states.len(), 3);
    }

    // ==========================================================================
    // get_forge_state() - Retrieve state for a specific forge
    // ==========================================================================

    #[test]
    fn test_get_forge_state_returns_correct_data() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(codeberg_found());

        // GitHub state should have the right data
        let gh = repo.get_forge_state(&Forge::GitHub).unwrap();
        assert_eq!(gh.forge_id, Some("gh-123".to_string()));
        assert_eq!(gh.description, Some("Description".to_string()));
        assert_eq!(gh.visibility, Some(Visibility::Public));

        // Codeberg state should have the right data
        let cb = repo.get_forge_state(&Forge::Codeberg).unwrap();
        assert_eq!(cb.forge_id, Some("cb-456".to_string()));
        assert_eq!(cb.description, None);

        // GitLab not queried
        assert!(repo.get_forge_state(&Forge::GitLab).is_none());
    }

    // ==========================================================================
    // JSON serialization roundtrip
    // ==========================================================================

    #[test]
    fn test_json_roundtrip_with_mixed_states() {
        let original = ObservedRepo::new(test_identity())
            .with_forge_state(github_found())
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg));

        let json = serde_json::to_string(&original).unwrap();
        let restored: ObservedRepo = serde_json::from_str(&json).unwrap();

        assert_eq!(original, restored);
        assert!(restored.exists_on(&Forge::GitHub));
        assert!(!restored.exists_on(&Forge::Codeberg));
    }
}
