//! Sync plan types - a plan of actions to reconcile state.
//!
//! This module defines the SyncPlan type which aggregates all RepoDiffs
//! and provides summary statistics and filtering capabilities.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::Forge;
use super::{RepoDiff, ForgeAction, DesiredRepo, ObservedRepo};

/// Summary statistics for a sync plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSummary {
    /// Number of repositories to create (across all forges)
    pub creates: usize,
    /// Number of repositories to update (across all forges)
    pub updates: usize,
    /// Number of repositories to delete (across all forges)
    pub deletes: usize,
    /// Number of repositories in sync (no action needed)
    pub in_sync: usize,
    /// Number of untracked repositories discovered
    pub untracked: usize,
}

impl PlanSummary {
    /// Check if any changes are needed
    pub fn has_changes(&self) -> bool {
        self.creates > 0 || self.updates > 0 || self.deletes > 0
    }

    /// Total number of actions
    pub fn total_actions(&self) -> usize {
        self.creates + self.updates + self.deletes
    }
}

/// A plan of actions to synchronize repositories.
///
/// This is a pure data structure with no I/O. It can be computed
/// from desired and observed states, filtered, and serialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncPlan {
    /// The organization this plan is for
    pub org: String,
    /// Diffs for each repository
    pub repo_diffs: Vec<RepoDiff>,
    /// Summary statistics
    pub summary: PlanSummary,
}

impl SyncPlan {
    /// Create a new empty sync plan for an organization
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            repo_diffs: Vec::new(),
            summary: PlanSummary::default(),
        }
    }

    /// Create a sync plan by diffing desired repos against observed state.
    ///
    /// This is a pure function - no I/O, just computation.
    pub fn from_diff(
        org: impl Into<String>,
        desired: &[DesiredRepo],
        observed: &[ObservedRepo],
    ) -> Self {
        let org = org.into();
        let mut repo_diffs = Vec::new();
        let mut summary = PlanSummary::default();

        // Build a map of observed repos by identity for quick lookup
        let observed_map: std::collections::HashMap<_, _> = observed
            .iter()
            .map(|o| (&o.identity, o))
            .collect();

        // Process each desired repo
        for desired_repo in desired {
            let observed_repo = observed_map
                .get(&desired_repo.identity)
                .map(|o| (*o).clone())
                .unwrap_or_else(|| ObservedRepo::new(desired_repo.identity.clone()));

            let diff = RepoDiff::compute(Some(desired_repo), &observed_repo);

            // Update summary
            summary.creates += diff.create_count();
            summary.updates += diff.update_count();
            summary.deletes += diff.delete_count();

            if !diff.needs_action() {
                summary.in_sync += 1;
            }

            repo_diffs.push(diff);
        }

        // Find untracked repos (observed but not desired)
        let desired_identities: std::collections::HashSet<_> = desired
            .iter()
            .map(|d| &d.identity)
            .collect();

        for observed_repo in observed {
            if !desired_identities.contains(&observed_repo.identity) {
                let diff = RepoDiff::compute(None, observed_repo);
                summary.untracked += 1;
                repo_diffs.push(diff);
            }
        }

        Self {
            org,
            repo_diffs,
            summary,
        }
    }

    /// Filter to only repos that need action
    pub fn actionable(&self) -> impl Iterator<Item = &RepoDiff> {
        self.repo_diffs.iter().filter(|d| d.needs_action())
    }

    /// Filter to only repos that need creation on any forge
    pub fn to_create(&self) -> impl Iterator<Item = &RepoDiff> {
        self.repo_diffs.iter().filter(|d| d.create_count() > 0)
    }

    /// Filter to only repos that need updates on any forge
    pub fn to_update(&self) -> impl Iterator<Item = &RepoDiff> {
        self.repo_diffs.iter().filter(|d| d.update_count() > 0)
    }

    /// Filter to only repos that need deletion on any forge
    pub fn to_delete(&self) -> impl Iterator<Item = &RepoDiff> {
        self.repo_diffs.iter().filter(|d| d.delete_count() > 0)
    }

    /// Filter to only untracked repos
    pub fn untracked(&self) -> impl Iterator<Item = &RepoDiff> {
        self.repo_diffs.iter().filter(|d| !d.is_tracked)
    }

    /// Get all forge actions for a specific forge
    pub fn actions_for_forge(&self, forge: &Forge) -> Vec<(&RepoDiff, &ForgeAction)> {
        self.repo_diffs
            .iter()
            .flat_map(|diff| {
                diff.forge_actions
                    .iter()
                    .filter(|a| a.forge() == forge)
                    .map(move |action| (diff, action))
            })
            .collect()
    }

    /// Check if the plan has any changes
    pub fn has_changes(&self) -> bool {
        self.summary.has_changes()
    }

    /// Get the diff for a specific repository
    pub fn get_repo(&self, name: &str) -> Option<&RepoDiff> {
        self.repo_diffs.iter().find(|d| d.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ForgeRepoState, DesiredRepo, RepoIdentity};
    use crate::types::Visibility;
    use std::collections::HashSet;

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

    fn make_desired(name: &str) -> DesiredRepo {
        DesiredRepo::new(
            RepoIdentity::new("hypermemetic", name),
            Visibility::Public,
            github_codeberg(),
        )
    }

    fn make_observed(name: &str, github: bool, codeberg: bool) -> ObservedRepo {
        let mut repo = ObservedRepo::new(RepoIdentity::new("hypermemetic", name));

        if github {
            repo = repo.with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                format!("https://github.com/hypermemetic/{}", name),
                Visibility::Public,
                None,
                None,
            ));
        }

        if codeberg {
            repo = repo.with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                format!("https://codeberg.org/hypermemetic/{}", name),
                Visibility::Public,
                None,
                None,
            ));
        }

        repo
    }

    // ==========================================================================
    // SyncPlan::from_diff() - Core plan generation (PURE FUNCTION)
    // ==========================================================================

    /// Empty desired + empty observed = empty plan with zero counts
    #[test]
    fn test_from_diff_empty_inputs() {
        let plan = SyncPlan::from_diff("hypermemetic", &[], &[]);

        assert_eq!(plan.org, "hypermemetic");
        assert!(plan.repo_diffs.is_empty());
        assert_eq!(plan.summary.creates, 0);
        assert_eq!(plan.summary.updates, 0);
        assert_eq!(plan.summary.deletes, 0);
        assert_eq!(plan.summary.in_sync, 0);
        assert_eq!(plan.summary.untracked, 0);
        assert!(!plan.has_changes());
    }

    /// Desired repos with no observed = all creates
    #[test]
    fn test_from_diff_all_creates() {
        let desired = vec![
            make_desired("repo1"),
            make_desired("repo2"),
            make_desired("repo3"),
        ];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        // 3 repos * 2 forges = 6 creates
        assert_eq!(plan.summary.creates, 6);
        assert_eq!(plan.summary.updates, 0);
        assert_eq!(plan.summary.deletes, 0);
        assert_eq!(plan.summary.in_sync, 0);
        assert_eq!(plan.summary.untracked, 0);
        assert!(plan.has_changes());
    }

    /// All repos in sync = zero changes, correct in_sync count
    #[test]
    fn test_from_diff_all_in_sync() {
        let desired = vec![
            make_desired("repo1"),
            make_desired("repo2"),
        ];
        let observed = vec![
            make_observed("repo1", true, true),
            make_observed("repo2", true, true),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.creates, 0);
        assert_eq!(plan.summary.updates, 0);
        assert_eq!(plan.summary.deletes, 0);
        assert_eq!(plan.summary.in_sync, 2);
        assert_eq!(plan.summary.untracked, 0);
        assert!(!plan.has_changes());
    }

    /// Observed repos not in desired = untracked
    #[test]
    fn test_from_diff_untracked_detection() {
        let desired = vec![make_desired("tracked-repo")];
        let observed = vec![
            make_observed("tracked-repo", true, true),
            make_observed("untracked1", true, false),
            make_observed("untracked2", false, true),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.in_sync, 1);
        assert_eq!(plan.summary.untracked, 2);

        // Verify untracked repos are marked as such
        let untracked_names: Vec<_> = plan.untracked().map(|d| d.name()).collect();
        assert!(untracked_names.contains(&"untracked1"));
        assert!(untracked_names.contains(&"untracked2"));
    }

    /// Mixed scenario: creates, updates, deletes, in-sync, untracked
    #[test]
    fn test_from_diff_mixed_scenario() {
        let desired = vec![
            // Repo 1: in sync on both forges
            make_desired("in-sync"),
            // Repo 2: needs creation on both forges
            make_desired("needs-create"),
            // Repo 3: needs update (visibility change)
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "needs-update"),
                Visibility::Private,
                github_codeberg(),
            ),
            // Repo 4: exists on Codeberg but shouldn't (delete)
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "needs-delete"),
                Visibility::Public,
                github_only(), // Only GitHub
            ),
        ];

        let observed = vec![
            make_observed("in-sync", true, true),
            // needs-create not observed
            make_observed("needs-update", true, true), // Both forges need visibility update
            make_observed("needs-delete", true, true), // Codeberg should be deleted
            make_observed("untracked", true, false),   // Not in desired
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.creates, 2);   // needs-create on 2 forges
        assert_eq!(plan.summary.updates, 2);   // needs-update on 2 forges
        assert_eq!(plan.summary.deletes, 1);   // needs-delete from Codeberg
        assert_eq!(plan.summary.in_sync, 1);   // in-sync
        assert_eq!(plan.summary.untracked, 1); // untracked

        // Total repos in plan: 4 tracked + 1 untracked = 5
        assert_eq!(plan.repo_diffs.len(), 5);
    }

    // ==========================================================================
    // Filter methods: actionable(), to_create(), to_update(), to_delete()
    // ==========================================================================

    /// actionable() filters out repos with no pending actions
    #[test]
    fn test_actionable_filters_correctly() {
        let desired = vec![
            make_desired("in-sync"),      // No action needed
            make_desired("needs-create"), // Needs action
        ];
        let observed = vec![
            make_observed("in-sync", true, true),
            // needs-create not observed
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let actionable: Vec<_> = plan.actionable().collect();
        assert_eq!(actionable.len(), 1);
        assert_eq!(actionable[0].name(), "needs-create");
    }

    /// to_create() returns only repos that have create actions
    #[test]
    fn test_to_create_filter() {
        let desired = vec![
            make_desired("existing"),
            make_desired("new-repo"),
        ];
        let observed = vec![
            make_observed("existing", true, true),
            // new-repo not observed
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let to_create: Vec<_> = plan.to_create().collect();
        assert_eq!(to_create.len(), 1);
        assert_eq!(to_create[0].name(), "new-repo");
    }

    /// to_update() returns only repos that have update actions
    #[test]
    fn test_to_update_filter() {
        let desired = vec![
            make_desired("in-sync"),
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "needs-update"),
                Visibility::Private,
                github_codeberg(),
            ),
        ];
        let observed = vec![
            make_observed("in-sync", true, true),
            make_observed("needs-update", true, true),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let to_update: Vec<_> = plan.to_update().collect();
        assert_eq!(to_update.len(), 1);
        assert_eq!(to_update[0].name(), "needs-update");
    }

    /// to_delete() returns only repos that have delete actions
    #[test]
    fn test_to_delete_filter() {
        let desired = vec![
            // Only want on GitHub, not Codeberg
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "reduce-forges"),
                Visibility::Public,
                github_only(),
            ),
            make_desired("keep-both"),
        ];
        let observed = vec![
            make_observed("reduce-forges", true, true), // Codeberg needs deletion
            make_observed("keep-both", true, true),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let to_delete: Vec<_> = plan.to_delete().collect();
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0].name(), "reduce-forges");
    }

    /// untracked() returns only untracked repos
    #[test]
    fn test_untracked_filter() {
        let desired = vec![make_desired("tracked")];
        let observed = vec![
            make_observed("tracked", true, true),
            make_observed("orphan1", true, false),
            make_observed("orphan2", false, true),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let untracked: Vec<_> = plan.untracked().collect();
        assert_eq!(untracked.len(), 2);

        let names: Vec<_> = untracked.iter().map(|d| d.name()).collect();
        assert!(names.contains(&"orphan1"));
        assert!(names.contains(&"orphan2"));
    }

    // ==========================================================================
    // actions_for_forge() - Filter actions by specific forge
    // ==========================================================================

    #[test]
    fn test_actions_for_forge_filters_correctly() {
        let desired = vec![
            make_desired("repo1"), // Create on both
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "repo2"),
                Visibility::Public,
                github_only(), // Only GitHub
            ),
        ];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        // GitHub should have 2 create actions (repo1 and repo2)
        let github_actions = plan.actions_for_forge(&Forge::GitHub);
        assert_eq!(github_actions.len(), 2);
        assert!(github_actions.iter().all(|(_, a)| a.is_create()));

        // Codeberg should have 1 create action (only repo1)
        let codeberg_actions = plan.actions_for_forge(&Forge::Codeberg);
        assert_eq!(codeberg_actions.len(), 1);
        assert_eq!(codeberg_actions[0].0.name(), "repo1");

        // GitLab should have no actions
        let gitlab_actions = plan.actions_for_forge(&Forge::GitLab);
        assert!(gitlab_actions.is_empty());
    }

    // ==========================================================================
    // get_repo() - Lookup repo by name
    // ==========================================================================

    #[test]
    fn test_get_repo_lookup() {
        let desired = vec![
            make_desired("repo-a"),
            make_desired("repo-b"),
            make_desired("repo-c"),
        ];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert!(plan.get_repo("repo-a").is_some());
        assert!(plan.get_repo("repo-b").is_some());
        assert!(plan.get_repo("repo-c").is_some());
        assert!(plan.get_repo("nonexistent").is_none());
        assert!(plan.get_repo("").is_none());
    }

    // ==========================================================================
    // PlanSummary calculations
    // ==========================================================================

    #[test]
    fn test_plan_summary_has_changes() {
        assert!(!PlanSummary::default().has_changes());

        assert!(PlanSummary { creates: 1, ..Default::default() }.has_changes());
        assert!(PlanSummary { updates: 1, ..Default::default() }.has_changes());
        assert!(PlanSummary { deletes: 1, ..Default::default() }.has_changes());

        // in_sync and untracked don't count as changes
        assert!(!PlanSummary { in_sync: 100, untracked: 50, ..Default::default() }.has_changes());
    }

    #[test]
    fn test_plan_summary_total_actions() {
        let summary = PlanSummary {
            creates: 5,
            updates: 3,
            deletes: 2,
            in_sync: 10,
            untracked: 7,
        };

        // Total = creates + updates + deletes (not in_sync or untracked)
        assert_eq!(summary.total_actions(), 10);
    }

    // ==========================================================================
    // Summary counts match actual diffs
    // ==========================================================================

    /// Verify summary counts are accurate by counting diffs directly
    #[test]
    fn test_summary_counts_match_actual_diffs() {
        let desired = vec![
            make_desired("create-me"),
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "update-me"),
                Visibility::Private,
                github_codeberg(),
            ),
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "delete-from-codeberg"),
                Visibility::Public,
                github_only(),
            ),
            make_desired("in-sync"),
        ];

        let observed = vec![
            // create-me not observed
            make_observed("update-me", true, true),
            make_observed("delete-from-codeberg", true, true),
            make_observed("in-sync", true, true),
            make_observed("untracked", true, false),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        // Manually count from diffs
        let actual_creates: usize = plan.repo_diffs.iter().map(|d| d.create_count()).sum();
        let actual_updates: usize = plan.repo_diffs.iter().map(|d| d.update_count()).sum();
        let actual_deletes: usize = plan.repo_diffs.iter().map(|d| d.delete_count()).sum();
        let actual_untracked = plan.repo_diffs.iter().filter(|d| !d.is_tracked).count();

        assert_eq!(plan.summary.creates, actual_creates);
        assert_eq!(plan.summary.updates, actual_updates);
        assert_eq!(plan.summary.deletes, actual_deletes);
        assert_eq!(plan.summary.untracked, actual_untracked);
    }

    // ==========================================================================
    // JSON serialization
    // ==========================================================================

    #[test]
    fn test_sync_plan_json_roundtrip() {
        let desired = vec![
            make_desired("repo1"),
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "repo2"),
                Visibility::Private,
                github_codeberg(),
            ).with_description("Test repo"),
        ];
        let observed = vec![
            make_observed("repo1", true, false),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);
        let json = serde_json::to_string(&plan).unwrap();
        let restored: SyncPlan = serde_json::from_str(&json).unwrap();

        assert_eq!(plan, restored);
        assert_eq!(plan.summary, restored.summary);
    }
}
