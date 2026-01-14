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

    fn github_codeberg_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges
    }

    fn make_desired(name: &str) -> DesiredRepo {
        DesiredRepo::new(
            RepoIdentity::new("hypermemetic", name),
            Visibility::Public,
            github_codeberg_forges(),
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

    #[test]
    fn test_sync_plan_new() {
        let plan = SyncPlan::new("hypermemetic");

        assert_eq!(plan.org, "hypermemetic");
        assert!(plan.repo_diffs.is_empty());
        assert!(!plan.has_changes());
    }

    #[test]
    fn test_sync_plan_from_diff_empty() {
        let plan = SyncPlan::from_diff("hypermemetic", &[], &[]);

        assert_eq!(plan.org, "hypermemetic");
        assert!(plan.repo_diffs.is_empty());
        assert_eq!(plan.summary.creates, 0);
        assert_eq!(plan.summary.updates, 0);
        assert_eq!(plan.summary.deletes, 0);
        assert_eq!(plan.summary.in_sync, 0);
        assert_eq!(plan.summary.untracked, 0);
    }

    #[test]
    fn test_sync_plan_from_diff_create_all() {
        let desired = vec![
            make_desired("repo1"),
            make_desired("repo2"),
        ];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.creates, 4); // 2 repos * 2 forges
        assert_eq!(plan.summary.in_sync, 0);
        assert!(plan.has_changes());

        let to_create: Vec<_> = plan.to_create().collect();
        assert_eq!(to_create.len(), 2);
    }

    #[test]
    fn test_sync_plan_from_diff_all_in_sync() {
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
        assert!(!plan.has_changes());
    }

    #[test]
    fn test_sync_plan_from_diff_untracked() {
        let desired = vec![make_desired("repo1")];
        let observed = vec![
            make_observed("repo1", true, true),
            make_observed("untracked-repo", true, false), // Not in desired
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.in_sync, 1);
        assert_eq!(plan.summary.untracked, 1);

        let untracked: Vec<_> = plan.untracked().collect();
        assert_eq!(untracked.len(), 1);
        assert_eq!(untracked[0].name(), "untracked-repo");
    }

    #[test]
    fn test_sync_plan_from_diff_mixed() {
        let desired = vec![
            make_desired("in-sync"),
            make_desired("needs-create"),
            DesiredRepo::new(
                RepoIdentity::new("hypermemetic", "needs-update"),
                Visibility::Private, // Different visibility
                github_codeberg_forges(),
            ),
        ];
        let observed = vec![
            make_observed("in-sync", true, true),
            // needs-create not observed
            make_observed("needs-update", true, true),
            make_observed("untracked", true, false),
        ];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert_eq!(plan.summary.creates, 2); // needs-create on 2 forges
        assert_eq!(plan.summary.updates, 2); // needs-update on 2 forges
        assert_eq!(plan.summary.deletes, 0);
        assert_eq!(plan.summary.in_sync, 1); // in-sync
        assert_eq!(plan.summary.untracked, 1); // untracked
        assert!(plan.has_changes());
    }

    #[test]
    fn test_sync_plan_actions_for_forge() {
        let desired = vec![make_desired("repo1")];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let github_actions = plan.actions_for_forge(&Forge::GitHub);
        assert_eq!(github_actions.len(), 1);
        assert!(github_actions[0].1.is_create());

        let codeberg_actions = plan.actions_for_forge(&Forge::Codeberg);
        assert_eq!(codeberg_actions.len(), 1);
        assert!(codeberg_actions[0].1.is_create());

        let gitlab_actions = plan.actions_for_forge(&Forge::GitLab);
        assert!(gitlab_actions.is_empty());
    }

    #[test]
    fn test_sync_plan_get_repo() {
        let desired = vec![
            make_desired("repo1"),
            make_desired("repo2"),
        ];
        let observed = vec![];

        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        assert!(plan.get_repo("repo1").is_some());
        assert!(plan.get_repo("repo2").is_some());
        assert!(plan.get_repo("nonexistent").is_none());
    }

    #[test]
    fn test_plan_summary_has_changes() {
        let empty = PlanSummary::default();
        assert!(!empty.has_changes());

        let with_creates = PlanSummary { creates: 1, ..Default::default() };
        assert!(with_creates.has_changes());

        let with_deletes = PlanSummary { deletes: 1, ..Default::default() };
        assert!(with_deletes.has_changes());
    }

    #[test]
    fn test_plan_summary_total_actions() {
        let summary = PlanSummary {
            creates: 2,
            updates: 3,
            deletes: 1,
            in_sync: 5,
            untracked: 2,
        };

        assert_eq!(summary.total_actions(), 6); // 2 + 3 + 1
    }

    #[test]
    fn test_sync_plan_serialization() {
        let desired = vec![make_desired("repo1")];
        let observed = vec![];
        let plan = SyncPlan::from_diff("hypermemetic", &desired, &observed);

        let json = serde_json::to_string(&plan).unwrap();
        let deserialized: SyncPlan = serde_json::from_str(&json).unwrap();

        assert_eq!(plan, deserialized);
    }
}
