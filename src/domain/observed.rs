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

    #[test]
    fn test_forge_repo_state_not_found() {
        let state = ForgeRepoState::not_found(Forge::GitHub);

        assert_eq!(state.forge, Forge::GitHub);
        assert!(!state.exists);
        assert!(state.url.is_none());
        assert!(state.forge_id.is_none());
        assert!(state.visibility.is_none());
    }

    #[test]
    fn test_forge_repo_state_found() {
        let state = ForgeRepoState::found(
            Forge::GitHub,
            "https://github.com/hypermemetic/hyperforge".to_string(),
            Visibility::Public,
            Some("123456".to_string()),
            Some("A multi-forge tool".to_string()),
        );

        assert_eq!(state.forge, Forge::GitHub);
        assert!(state.exists);
        assert_eq!(state.url, Some("https://github.com/hypermemetic/hyperforge".to_string()));
        assert_eq!(state.forge_id, Some("123456".to_string()));
        assert_eq!(state.visibility, Some(Visibility::Public));
        assert_eq!(state.description, Some("A multi-forge tool".to_string()));
    }

    #[test]
    fn test_observed_repo_new() {
        let repo = ObservedRepo::new(test_identity());

        assert_eq!(repo.name(), "hyperforge");
        assert_eq!(repo.org(), "hypermemetic");
        assert!(repo.forge_states.is_empty());
        assert!(!repo.exists_anywhere());
    }

    #[test]
    fn test_observed_repo_with_forge_states() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ))
            .with_forge_state(ForgeRepoState::not_found(Forge::Codeberg));

        assert!(repo.exists_on(&Forge::GitHub));
        assert!(!repo.exists_on(&Forge::Codeberg));
        assert!(repo.exists_anywhere());

        let existing = repo.existing_forges();
        assert_eq!(existing.len(), 1);
        assert!(existing.contains(&Forge::GitHub));
    }

    #[test]
    fn test_observed_repo_get_forge_state() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                Some("123".to_string()),
                None,
            ));

        let github_state = repo.get_forge_state(&Forge::GitHub);
        assert!(github_state.is_some());
        assert_eq!(github_state.unwrap().forge_id, Some("123".to_string()));

        let codeberg_state = repo.get_forge_state(&Forge::Codeberg);
        assert!(codeberg_state.is_none());
    }

    #[test]
    fn test_observed_repo_replaces_existing_forge_state() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::not_found(Forge::GitHub))
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        // Should only have one GitHub state (the latest)
        let github_states: Vec<_> = repo.forge_states.iter()
            .filter(|s| s.forge == Forge::GitHub)
            .collect();
        assert_eq!(github_states.len(), 1);
        assert!(github_states[0].exists);
    }

    #[test]
    fn test_observed_repo_serialization() {
        let repo = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let json = serde_json::to_string(&repo).unwrap();
        let deserialized: ObservedRepo = serde_json::from_str(&json).unwrap();

        assert_eq!(repo, deserialized);
    }
}
