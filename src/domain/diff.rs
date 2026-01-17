//! Diff types - the difference between desired and observed state.
//!
//! This module defines types that represent the actions needed to
//! reconcile desired state with observed state.
//!
//! ## New Symmetric Diff API
//!
//! The [`compute_sync_actions`] function computes differences between
//! repos from any two forges, enabling symmetric sync operations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{Forge, Visibility};
use super::{RepoIdentity, DesiredRepo, ObservedRepo, Repo, SyncAction, PropertyDiff};

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

// =============================================================================
// Symmetric Sync Functions
// =============================================================================

/// Compute sync actions needed to make target match source.
///
/// This is the core of symmetric sync - the same function works for:
/// - `sync(github, local)` = import from GitHub
/// - `sync(local, github)` = push to GitHub
/// - `sync(github, codeberg)` = mirror GitHub to Codeberg
///
/// # Arguments
/// * `source_repos` - Repos from source forge (the "truth")
/// * `target_repos` - Repos from target forge (to be updated)
/// * `delete_missing` - If true, delete repos in target not in source
///
/// # Returns
/// List of actions to apply to target forge
pub fn compute_sync_actions(
    source_repos: &[Repo],
    target_repos: &[Repo],
    delete_missing: bool,
) -> Vec<SyncAction> {
    let mut actions = Vec::new();

    // Index target repos by identity for fast lookup
    let target_map: HashMap<&RepoIdentity, &Repo> = target_repos
        .iter()
        .map(|r| (&r.identity, r))
        .collect();

    // Index source repos by identity
    let source_map: HashMap<&RepoIdentity, &Repo> = source_repos
        .iter()
        .map(|r| (&r.identity, r))
        .collect();

    // Check each source repo
    for source in source_repos {
        match target_map.get(&source.identity) {
            Some(target) => {
                // Exists in both - check for differences
                let diffs = compute_property_diffs(source, target);
                if diffs.is_empty() {
                    actions.push(SyncAction::InSync(source.identity.clone()));
                } else {
                    actions.push(SyncAction::Update {
                        repo: source.clone(),
                        diffs,
                    });
                }
            }
            None => {
                // Only in source - needs create in target
                actions.push(SyncAction::Create(source.clone()));
            }
        }
    }

    // Check for repos only in target (potential deletes)
    if delete_missing {
        for target in target_repos {
            if !source_map.contains_key(&target.identity) {
                actions.push(SyncAction::Delete(target.identity.clone()));
            }
        }
    }

    actions
}

/// Compute property differences between two repos
fn compute_property_diffs(source: &Repo, target: &Repo) -> Vec<PropertyDiff> {
    let mut diffs = Vec::new();

    if source.visibility != target.visibility {
        diffs.push(PropertyDiff::Visibility {
            source: source.visibility.clone(),
            target: target.visibility.clone(),
        });
    }

    if source.description != target.description {
        diffs.push(PropertyDiff::Description {
            source: source.description.clone(),
            target: target.description.clone(),
        });
    }

    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ForgeRepoState, DesiredRepo};
    use std::collections::HashSet;

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

    // ==========================================================================
    // RepoDiff::compute() - Core diffing logic (PURE FUNCTION)
    // ==========================================================================

    /// Desired repo + no observed state = Create actions for all target forges
    #[test]
    fn test_compute_create_on_all_target_forges() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg());
        let observed = ObservedRepo::new(test_identity()); // No forge states

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.is_tracked);
        assert!(!diff.marked_for_deletion);
        assert_eq!(diff.create_count(), 2);
        assert_eq!(diff.update_count(), 0);
        assert_eq!(diff.delete_count(), 0);
        assert!(diff.needs_action());

        // Verify both forges have Create actions
        for action in &diff.forge_actions {
            assert!(action.is_create(), "Expected Create for {:?}", action.forge());
        }
    }

    /// Desired matches observed exactly = NoOp for all forges
    #[test]
    fn test_compute_in_sync_produces_noops() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg())
            .with_description("My repo");

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                Some("My repo".to_string()),
            ))
            .with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                "https://codeberg.org/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                Some("My repo".to_string()),
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(!diff.needs_action());
        assert_eq!(diff.create_count(), 0);
        assert_eq!(diff.update_count(), 0);
        assert_eq!(diff.delete_count(), 0);

        for action in &diff.forge_actions {
            assert!(action.is_noop());
        }
    }

    /// Visibility mismatch = Update action with visibility change recorded
    #[test]
    fn test_compute_visibility_change_produces_update() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Private, github_only());
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public, // Mismatch: observed Public, want Private
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.update_count(), 1);
        let update = diff.forge_actions.iter().find(|a| a.is_update()).unwrap();

        if let ForgeAction::Update { forge, changes } = update {
            assert_eq!(forge, &Forge::GitHub);
            assert_eq!(changes.visibility, Some((Visibility::Public, Visibility::Private)));
            assert!(changes.description.is_none());
        } else {
            panic!("Expected Update action");
        }
    }

    /// Description mismatch = Update action with description change recorded
    #[test]
    fn test_compute_description_change_produces_update() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_only())
            .with_description("New description");

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                Some("Old description".to_string()),
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.update_count(), 1);
        let update = diff.forge_actions.iter().find(|a| a.is_update()).unwrap();

        if let ForgeAction::Update { changes, .. } = update {
            assert!(changes.visibility.is_none());
            assert_eq!(
                changes.description,
                Some((Some("Old description".to_string()), Some("New description".to_string())))
            );
        } else {
            panic!("Expected Update action");
        }
    }

    /// Multiple property changes at once
    #[test]
    fn test_compute_multiple_property_changes() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Private, github_only())
            .with_description("New desc");

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                Some("Old desc".to_string()),
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.update_count(), 1);
        let update = diff.forge_actions.iter().find(|a| a.is_update()).unwrap();

        if let ForgeAction::Update { changes, .. } = update {
            // Both visibility AND description changed
            assert!(changes.visibility.is_some());
            assert!(changes.description.is_some());
            assert!(!changes.is_empty());
        } else {
            panic!("Expected Update action");
        }
    }

    /// Description: None -> Some (adding description)
    #[test]
    fn test_compute_adding_description() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_only())
            .with_description("Added description");

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None, // No description currently
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.update_count(), 1);
        let update = diff.forge_actions.iter().find(|a| a.is_update()).unwrap();

        if let ForgeAction::Update { changes, .. } = update {
            assert_eq!(
                changes.description,
                Some((None, Some("Added description".to_string())))
            );
        }
    }

    /// Description: Some -> None (removing description)
    #[test]
    fn test_compute_removing_description() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_only());
        // No description set (None)

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                Some("Has description".to_string()),
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.update_count(), 1);
        let update = diff.forge_actions.iter().find(|a| a.is_update()).unwrap();

        if let ForgeAction::Update { changes, .. } = update {
            assert_eq!(
                changes.description,
                Some((Some("Has description".to_string()), None))
            );
        }
    }

    /// Repo exists on forge not in desired forges set = Delete action
    #[test]
    fn test_compute_delete_from_unwanted_forge() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_only());

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

        assert_eq!(diff.delete_count(), 1);
        let delete = diff.forge_actions.iter().find(|a| a.is_delete()).unwrap();

        if let ForgeAction::Delete { forge, url } = delete {
            assert_eq!(forge, &Forge::Codeberg);
            assert_eq!(url, &Some("https://codeberg.org/hypermemetic/hyperforge".to_string()));
        }
    }

    /// marked_for_deletion = Delete existing, NoOp non-existing
    #[test]
    fn test_compute_marked_for_deletion() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Public, github_codeberg())
            .with_deletion_mark(true);

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));
        // Codeberg not observed (doesn't exist)

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert!(diff.marked_for_deletion);
        assert_eq!(diff.delete_count(), 1); // Delete from GitHub where it exists
        // Codeberg should be NoOp since nothing to delete

        let github_action = diff.forge_actions.iter().find(|a| a.forge() == &Forge::GitHub).unwrap();
        assert!(github_action.is_delete());

        let codeberg_action = diff.forge_actions.iter().find(|a| a.forge() == &Forge::Codeberg).unwrap();
        assert!(codeberg_action.is_noop());
    }

    /// No desired state = untracked repo with empty actions
    #[test]
    fn test_compute_untracked_repo() {
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
        assert!(!diff.marked_for_deletion);
        assert!(diff.forge_actions.is_empty());
        assert!(!diff.needs_action()); // Untracked repos don't need action
    }

    /// Mixed scenario: create + update + noop on different forges
    #[test]
    fn test_compute_mixed_actions() {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges.insert(Forge::GitLab);

        let desired = DesiredRepo::new(test_identity(), Visibility::Private, forges);

        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Private, // In sync
                None,
                None,
            ))
            .with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                "https://codeberg.org/hypermemetic/hyperforge".to_string(),
                Visibility::Public, // Needs update
                None,
                None,
            ));
        // GitLab not observed = needs create

        let diff = RepoDiff::compute(Some(&desired), &observed);

        assert_eq!(diff.create_count(), 1);  // GitLab
        assert_eq!(diff.update_count(), 1);  // Codeberg
        assert!(diff.needs_action());

        // Verify specific actions
        let gitlab_action = diff.forge_actions.iter().find(|a| a.forge() == &Forge::GitLab).unwrap();
        assert!(gitlab_action.is_create());

        let codeberg_action = diff.forge_actions.iter().find(|a| a.forge() == &Forge::Codeberg).unwrap();
        assert!(codeberg_action.is_update());

        let github_action = diff.forge_actions.iter().find(|a| a.forge() == &Forge::GitHub).unwrap();
        assert!(github_action.is_noop());
    }

    // ==========================================================================
    // PropertyChanges helper methods
    // ==========================================================================

    #[test]
    fn test_property_changes_is_empty() {
        let empty = PropertyChanges::default();
        assert!(empty.is_empty());

        let with_visibility = PropertyChanges {
            visibility: Some((Visibility::Public, Visibility::Private)),
            description: None,
        };
        assert!(!with_visibility.is_empty());

        let with_description = PropertyChanges {
            visibility: None,
            description: Some((None, Some("desc".to_string()))),
        };
        assert!(!with_description.is_empty());

        let with_both = PropertyChanges {
            visibility: Some((Visibility::Public, Visibility::Private)),
            description: Some((None, Some("desc".to_string()))),
        };
        assert!(!with_both.is_empty());
    }

    // ==========================================================================
    // ForgeAction helper methods
    // ==========================================================================

    #[test]
    fn test_forge_action_type_predicates() {
        let cases = [
            (
                ForgeAction::Create { forge: Forge::GitHub, visibility: Visibility::Public, description: None },
                true, false, false, false,
            ),
            (
                ForgeAction::Update { forge: Forge::GitHub, changes: PropertyChanges::default() },
                false, true, false, false,
            ),
            (
                ForgeAction::Delete { forge: Forge::GitHub, url: None },
                false, false, true, false,
            ),
            (
                ForgeAction::NoOp { forge: Forge::GitHub },
                false, false, false, true,
            ),
        ];

        for (action, is_create, is_update, is_delete, is_noop) in cases {
            assert_eq!(action.is_create(), is_create, "is_create for {:?}", action);
            assert_eq!(action.is_update(), is_update, "is_update for {:?}", action);
            assert_eq!(action.is_delete(), is_delete, "is_delete for {:?}", action);
            assert_eq!(action.is_noop(), is_noop, "is_noop for {:?}", action);
        }
    }

    #[test]
    fn test_forge_action_forge_accessor() {
        let actions = [
            (ForgeAction::Create { forge: Forge::GitHub, visibility: Visibility::Public, description: None }, Forge::GitHub),
            (ForgeAction::Update { forge: Forge::Codeberg, changes: PropertyChanges::default() }, Forge::Codeberg),
            (ForgeAction::Delete { forge: Forge::GitLab, url: None }, Forge::GitLab),
            (ForgeAction::NoOp { forge: Forge::GitHub }, Forge::GitHub),
        ];

        for (action, expected_forge) in actions {
            assert_eq!(action.forge(), &expected_forge);
        }
    }

    // ==========================================================================
    // JSON serialization
    // ==========================================================================

    #[test]
    fn test_diff_json_roundtrip() {
        let desired = DesiredRepo::new(test_identity(), Visibility::Private, github_codeberg())
            .with_description("Test");
        let observed = ObservedRepo::new(test_identity())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/hypermemetic/hyperforge".to_string(),
                Visibility::Public,
                None,
                None,
            ));

        let diff = RepoDiff::compute(Some(&desired), &observed);
        let json = serde_json::to_string(&diff).unwrap();
        let restored: RepoDiff = serde_json::from_str(&json).unwrap();

        assert_eq!(diff, restored);
    }

    // ==========================================================================
    // Symmetric Sync Tests (compute_sync_actions)
    // ==========================================================================

    fn make_repo(org: &str, name: &str) -> Repo {
        Repo::new(RepoIdentity::new(org, name), Visibility::Public)
    }

    #[test]
    fn test_sync_empty_source_and_target() {
        let actions = compute_sync_actions(&[], &[], false);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_sync_create_when_only_in_source() {
        let source = vec![make_repo("org", "repo1")];
        let target = vec![];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_create());
    }

    #[test]
    fn test_sync_in_sync_when_identical() {
        let source = vec![make_repo("org", "repo1")];
        let target = vec![make_repo("org", "repo1")];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_in_sync());
    }

    #[test]
    fn test_sync_update_when_visibility_differs() {
        let source = vec![Repo::new(RepoIdentity::new("org", "repo1"), Visibility::Private)];
        let target = vec![Repo::new(RepoIdentity::new("org", "repo1"), Visibility::Public)];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_update());
    }

    #[test]
    fn test_sync_update_when_description_differs() {
        let source = vec![make_repo("org", "repo1").with_description("new")];
        let target = vec![make_repo("org", "repo1").with_description("old")];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_update());
        if let SyncAction::Update { diffs, .. } = &actions[0] {
            assert_eq!(diffs.len(), 1);
            assert!(matches!(diffs[0], PropertyDiff::Description { .. }));
        }
    }

    #[test]
    fn test_sync_delete_when_only_in_target_and_flag_set() {
        let source = vec![];
        let target = vec![make_repo("org", "orphan")];

        let actions = compute_sync_actions(&source, &target, true);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_delete());
    }

    #[test]
    fn test_sync_no_delete_when_flag_not_set() {
        let source = vec![];
        let target = vec![make_repo("org", "orphan")];

        let actions = compute_sync_actions(&source, &target, false);

        // No actions because we're not deleting missing repos
        assert!(actions.is_empty());
    }

    #[test]
    fn test_sync_mixed_actions() {
        let source = vec![
            make_repo("org", "existing"),  // Same in both
            make_repo("org", "new"),       // Only in source
            Repo::new(RepoIdentity::new("org", "changed"), Visibility::Private),  // Different vis
        ];
        let target = vec![
            make_repo("org", "existing"),  // Same
            make_repo("org", "orphan"),    // Only in target
            Repo::new(RepoIdentity::new("org", "changed"), Visibility::Public),   // Different vis
        ];

        let actions = compute_sync_actions(&source, &target, true);

        // Should have: 1 in_sync, 1 create, 1 update, 1 delete
        assert_eq!(actions.len(), 4);

        let in_sync_count = actions.iter().filter(|a| a.is_in_sync()).count();
        let create_count = actions.iter().filter(|a| a.is_create()).count();
        let update_count = actions.iter().filter(|a| a.is_update()).count();
        let delete_count = actions.iter().filter(|a| a.is_delete()).count();

        assert_eq!(in_sync_count, 1);
        assert_eq!(create_count, 1);
        assert_eq!(update_count, 1);
        assert_eq!(delete_count, 1);
    }

    #[test]
    fn test_sync_multiple_repos_create() {
        let source = vec![
            make_repo("org", "repo1"),
            make_repo("org", "repo2"),
            make_repo("org", "repo3"),
        ];
        let target = vec![];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 3);
        assert!(actions.iter().all(|a| a.is_create()));
    }
}
