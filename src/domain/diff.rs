//! Diff types - the difference between desired and observed state.
//!
//! This module defines types that represent the actions needed to
//! reconcile desired state with observed state.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{Forge, Visibility};
use super::{RepoIdentity, DesiredRepo, ObservedRepo};

/// What changed about a repository's properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PropertyChanges {
    /// Visibility changed
    pub visibility: Option<(Visibility, Visibility)>,
    /// Description changed (old, new)
    pub description: Option<(Option<String>, Option<String>)>,
}

impl PropertyChanges {
    /// Check if there are any changes
    pub fn is_empty(&self) -> bool {
        self.visibility.is_none() && self.description.is_none()
    }
}

impl Default for PropertyChanges {
    fn default() -> Self {
        Self {
            visibility: None,
            description: None,
        }
    }
}

/// Action needed for a repository on a specific forge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ForgeAction {
    /// Repository needs to be created on this forge
    Create {
        forge: Forge,
        visibility: Visibility,
        description: Option<String>,
    },
    /// Repository needs to be updated on this forge
    Update {
        forge: Forge,
        changes: PropertyChanges,
    },
    /// Repository needs to be deleted from this forge
    Delete {
        forge: Forge,
        /// URL of the repo to delete (for confirmation)
        url: Option<String>,
    },
    /// No action needed - repo is in sync on this forge
    NoOp {
        forge: Forge,
    },
}

impl ForgeAction {
    /// Get the forge this action applies to
    pub fn forge(&self) -> &Forge {
        match self {
            ForgeAction::Create { forge, .. } => forge,
            ForgeAction::Update { forge, .. } => forge,
            ForgeAction::Delete { forge, .. } => forge,
            ForgeAction::NoOp { forge } => forge,
        }
    }

    /// Check if this is a no-op
    pub fn is_noop(&self) -> bool {
        matches!(self, ForgeAction::NoOp { .. })
    }

    /// Check if this action would create a repo
    pub fn is_create(&self) -> bool {
        matches!(self, ForgeAction::Create { .. })
    }

    /// Check if this action would delete a repo
    pub fn is_delete(&self) -> bool {
        matches!(self, ForgeAction::Delete { .. })
    }

    /// Check if this action would update a repo
    pub fn is_update(&self) -> bool {
        matches!(self, ForgeAction::Update { .. })
    }
}

/// The complete diff for a repository across all forges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoDiff {
    /// Identity of the repository
    pub identity: RepoIdentity,
    /// Actions needed for each forge
    pub forge_actions: Vec<ForgeAction>,
    /// Whether this repo is tracked in configuration
    pub is_tracked: bool,
    /// Whether this repo is marked for deletion
    pub marked_for_deletion: bool,
}

impl RepoDiff {
    /// Create a diff from desired and observed states
    pub fn compute(desired: Option<&DesiredRepo>, observed: &ObservedRepo) -> Self {
        let identity = observed.identity.clone();
        let mut forge_actions = Vec::new();

        match desired {
            Some(desired) => {
                // Repo is tracked - compute per-forge actions
                for forge in &desired.forges {
                    let action = Self::compute_forge_action(
                        forge,
                        desired,
                        observed.get_forge_state(forge),
                    );
                    forge_actions.push(action);
                }

                // Check for repos on forges not in desired state (need deletion)
                for forge_state in &observed.forge_states {
                    if forge_state.exists && !desired.forges.contains(&forge_state.forge) {
                        forge_actions.push(ForgeAction::Delete {
                            forge: forge_state.forge.clone(),
                            url: forge_state.url.clone(),
                        });
                    }
                }

                Self {
                    identity,
                    forge_actions,
                    is_tracked: true,
                    marked_for_deletion: desired.marked_for_deletion,
                }
            }
            None => {
                // Repo is untracked - just report existence
                Self {
                    identity,
                    forge_actions: vec![],
                    is_tracked: false,
                    marked_for_deletion: false,
                }
            }
        }
    }

    /// Compute the action needed for a specific forge
    fn compute_forge_action(
        forge: &Forge,
        desired: &DesiredRepo,
        observed: Option<&super::ForgeRepoState>,
    ) -> ForgeAction {
        // If marked for deletion, delete if exists
        if desired.marked_for_deletion {
            return match observed {
                Some(state) if state.exists => ForgeAction::Delete {
                    forge: forge.clone(),
                    url: state.url.clone(),
                },
                _ => ForgeAction::NoOp { forge: forge.clone() },
            };
        }

        match observed {
            Some(state) if state.exists => {
                // Repo exists - check if update needed
                let mut changes = PropertyChanges::default();

                // Check visibility
                if let Some(obs_visibility) = &state.visibility {
                    if obs_visibility != &desired.visibility {
                        changes.visibility = Some((obs_visibility.clone(), desired.visibility.clone()));
                    }
                }

                // Check description
                if state.description != desired.description {
                    changes.description = Some((state.description.clone(), desired.description.clone()));
                }

                if changes.is_empty() {
                    ForgeAction::NoOp { forge: forge.clone() }
                } else {
                    ForgeAction::Update {
                        forge: forge.clone(),
                        changes,
                    }
                }
            }
            _ => {
                // Repo doesn't exist - needs creation
                ForgeAction::Create {
                    forge: forge.clone(),
                    visibility: desired.visibility.clone(),
                    description: desired.description.clone(),
                }
            }
        }
    }

    /// Check if any action is needed
    pub fn needs_action(&self) -> bool {
        self.forge_actions.iter().any(|a| !a.is_noop())
    }

    /// Get count of creates
    pub fn create_count(&self) -> usize {
        self.forge_actions.iter().filter(|a| a.is_create()).count()
    }

    /// Get count of updates
    pub fn update_count(&self) -> usize {
        self.forge_actions.iter().filter(|a| a.is_update()).count()
    }

    /// Get count of deletes
    pub fn delete_count(&self) -> usize {
        self.forge_actions.iter().filter(|a| a.is_delete()).count()
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
    use super::super::{ForgeRepoState, DesiredRepo};
    use std::collections::HashSet;

    fn test_identity() -> RepoIdentity {
        RepoIdentity::new("hypermemetic", "hyperforge")
    }

    fn github_codeberg_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges
    }

    #[test]
    fn test_compute_diff_create_everywhere() {
        let desired = DesiredRepo::new(
            test_identity(),
            Visibility::Public,
            github_codeberg_forges(),
        );
        let observed = ObservedRepo::new(test_identity());

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.is_tracked);
        assert!(!diff.marked_for_deletion);
        assert_eq!(diff.create_count(), 2);
        assert_eq!(diff.update_count(), 0);
        assert_eq!(diff.delete_count(), 0);
        assert!(diff.needs_action());
    }

    #[test]
    fn test_compute_diff_in_sync() {
        let desired = DesiredRepo::new(
            test_identity(),
            Visibility::Public,
            github_codeberg_forges(),
        );
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ))
            .with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                "https://codeberg.org/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(!diff.needs_action());
        assert_eq!(diff.create_count(), 0);
        assert_eq!(diff.update_count(), 0);
        assert_eq!(diff.delete_count(), 0);
    }

    #[test]
    fn test_compute_diff_visibility_update() {
        let desired = DesiredRepo::new(
            test_identity(),
            Visibility::Private, // Want private
            github_codeberg_forges(),
        );
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public, // Currently public
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.needs_action());
        assert_eq!(diff.create_count(), 1); // Codeberg
        assert_eq!(diff.update_count(), 1); // GitHub
        assert_eq!(diff.delete_count(), 0);

        // Check the update action has correct changes
        let update_action = diff.forge_actions.iter()
            .find(|a| a.is_update())
            .unwrap();
        if let ForgeAction::Update { changes, .. } = update_action {
            assert_eq!(changes.visibility, Some((Visibility::Public, Visibility::Private)));
        }
    }

    #[test]
    fn test_compute_diff_delete_extra_forge() {
        // Only want GitHub, but exists on both
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);

        let desired = DesiredRepo::new(test_identity(), Visibility::Public, forges);
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ))
            .with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                "https://codeberg.org/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.needs_action());
        assert_eq!(diff.create_count(), 0);
        assert_eq!(diff.update_count(), 0);
        assert_eq!(diff.delete_count(), 1); // Delete from Codeberg
    }

    #[test]
    fn test_compute_diff_marked_for_deletion() {
        let desired = DesiredRepo::new(
            test_identity(),
            Visibility::Public,
            github_codeberg_forges(),
        ).with_deletion_mark(true);

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.marked_for_deletion);
        assert_eq!(diff.delete_count(), 1); // Delete from GitHub
        // Codeberg is NoOp (nothing to delete)
    }

    #[test]
    fn test_compute_diff_untracked() {
        // No desired state - repo is untracked
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let diff = RepoDiff::compute(None, &observed);

        assert!(!diff.is_tracked);
        assert!(diff.forge_actions.is_empty());
    }

    #[test]
    fn test_property_changes_is_empty() {
        let empty = PropertyChanges::default();
        assert!(empty.is_empty());

        let with_visibility = PropertyChanges {
            visibility: Some((Visibility::Public, Visibility::Private)),
            ..Default::default()
        };
        assert!(!with_visibility.is_empty());
    }

    #[test]
    fn test_forge_action_accessors() {
        let create = ForgeAction::Create {
            forge: Forge::GitHub,
            visibility: Visibility::Public,
            description: None,
        };
        assert!(create.is_create());
        assert!(!create.is_delete());
        assert!(!create.is_update());
        assert!(!create.is_noop());
        assert_eq!(create.forge(), &Forge::GitHub);

        let noop = ForgeAction::NoOp { forge: Forge::Codeberg };
        assert!(noop.is_noop());
        assert!(!noop.is_create());
    }

    #[test]
    fn test_diff_serialization() {
        let desired = DesiredRepo::new(
            test_identity(),
            Visibility::Public,
            github_codeberg_forges(),
        );
        let observed = ObservedRepo::new(test_identity());
        let diff = RepoDiff::compute(Some(&desired), &observed);

        let json = serde_json::to_string(&diff).unwrap();
        let deserialized: RepoDiff = serde_json::from_str(&json).unwrap();

        assert_eq!(diff, deserialized);
    }
}
