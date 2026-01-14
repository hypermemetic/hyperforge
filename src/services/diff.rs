//! Diff service - computes differences between desired and observed state.
//!
//! This service orchestrates the comparison between what the user wants
//! (desired state in storage) and what actually exists (queried from forges).
//! It produces `SyncPlan` objects that describe what changes are needed.

use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;

use crate::domain::{DesiredRepo, ObservedRepo, SyncPlan};
use crate::ports::{ForgeError, ForgePort, StorageError, StoragePort};
use crate::types::Forge;

/// Errors that can occur during diff operations.
#[derive(Debug, Error)]
pub enum DiffError {
    /// A forge operation failed
    #[error("Forge operation failed: {0}")]
    ForgeError(#[from] ForgeError),

    /// A storage operation failed
    #[error("Storage operation failed: {0}")]
    StorageError(#[from] StorageError),

    /// Organization not found in local storage
    #[error("Organization '{org}' not found in local configuration")]
    OrgNotConfigured { org: String },

    /// No forge adapters configured
    #[error("No forge adapters configured")]
    NoForgesConfigured,
}

/// Options for diff operation.
#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Use cached/synced state instead of querying forges
    pub use_cached: bool,
    /// Only diff against specific forges (empty = all)
    pub forges: HashSet<Forge>,
    /// Include repos that exist on forges but not in config
    pub include_untracked: bool,
}

impl DiffOptions {
    /// Create options for a fresh diff (query forges).
    pub fn fresh() -> Self {
        Self {
            use_cached: false,
            forges: HashSet::new(),
            include_untracked: true,
        }
    }

    /// Create options using cached state.
    pub fn cached() -> Self {
        Self {
            use_cached: true,
            forges: HashSet::new(),
            include_untracked: true,
        }
    }

    /// Builder: limit to specific forges
    pub fn for_forge(mut self, forge: Forge) -> Self {
        self.forges.insert(forge);
        self
    }

    /// Builder: exclude untracked repos
    pub fn tracked_only(mut self) -> Self {
        self.include_untracked = false;
        self
    }
}

/// Service for computing state differences.
///
/// Compares desired state (from storage) with observed state (from forges
/// or cached) and produces sync plans.
pub struct DiffService {
    /// Forge adapters for querying current state
    forges: Vec<Arc<dyn ForgePort>>,
    /// Storage adapter for loading desired/synced state
    storage: Arc<dyn StoragePort>,
}

impl DiffService {
    /// Create a new diff service with the given ports.
    ///
    /// # Arguments
    ///
    /// * `forges` - List of forge adapters for querying state
    /// * `storage` - Storage adapter for loading configuration
    pub fn new(forges: Vec<Arc<dyn ForgePort>>, storage: Arc<dyn StoragePort>) -> Self {
        Self { forges, storage }
    }

    /// Compute a sync plan for an organization.
    ///
    /// Loads desired state from storage, queries observed state from forges
    /// (or uses cached synced state), and computes what actions are needed.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization to compute diff for
    /// * `options` - Diff options
    ///
    /// # Returns
    ///
    /// A `SyncPlan` describing what changes are needed.
    pub async fn compute_plan(
        &self,
        org: &str,
        options: &DiffOptions,
    ) -> Result<SyncPlan, DiffError> {
        // Load desired state from storage
        let desired = match self.storage.load_desired(org).await {
            Ok(repos) => repos,
            Err(StorageError::OrgNotFound(_)) => {
                return Err(DiffError::OrgNotConfigured { org: org.to_string() });
            }
            Err(e) => return Err(DiffError::StorageError(e)),
        };

        // Get observed state (from cache or forges)
        let observed = if options.use_cached {
            self.load_cached_state(org).await?
        } else {
            self.query_forge_state(org, &desired, options).await?
        };

        // Compute the plan using domain logic
        let mut plan = SyncPlan::from_diff(org, &desired, &observed);

        // Filter out untracked if requested
        if !options.include_untracked {
            plan.repo_diffs.retain(|d| d.is_tracked);
            // Recalculate summary
            plan.summary.untracked = 0;
        }

        Ok(plan)
    }

    /// Load cached/synced state from storage.
    async fn load_cached_state(&self, org: &str) -> Result<Vec<ObservedRepo>, DiffError> {
        match self.storage.load_synced(org).await {
            Ok(repos) => Ok(repos),
            Err(StorageError::OrgNotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(DiffError::StorageError(e)),
        }
    }

    /// Query forges for current state.
    async fn query_forge_state(
        &self,
        org: &str,
        desired: &[DesiredRepo],
        options: &DiffOptions,
    ) -> Result<Vec<ObservedRepo>, DiffError> {
        if self.forges.is_empty() {
            return Err(DiffError::NoForgesConfigured);
        }

        // Determine which forges to query
        let forges_to_query: Vec<_> = if options.forges.is_empty() {
            // Query all configured forges
            self.forges.iter().collect()
        } else {
            self.forges
                .iter()
                .filter(|f| options.forges.contains(&f.forge_type()))
                .collect()
        };

        // Also include forges mentioned in desired repos
        let desired_forges: HashSet<_> = desired
            .iter()
            .flat_map(|d| d.forges.iter())
            .cloned()
            .collect();

        let all_forges_to_query: HashSet<_> = forges_to_query
            .iter()
            .map(|f| f.forge_type())
            .chain(desired_forges)
            .collect();

        // Query each forge
        let mut all_observed: Vec<ObservedRepo> = Vec::new();

        for forge_adapter in &self.forges {
            if !all_forges_to_query.contains(&forge_adapter.forge_type()) {
                continue;
            }

            match forge_adapter.list_repos(org).await {
                Ok(repos) => {
                    for observed in repos {
                        self.merge_observed(&mut all_observed, observed);
                    }
                }
                Err(ForgeError::OrgNotFound { .. }) => {
                    // Org doesn't exist on this forge - not an error
                    continue;
                }
                Err(e) => {
                    // Log error but continue with other forges
                    // In production, we might want to track these errors
                    let _ = e;
                }
            }
        }

        // Ensure we have entries for all repos in desired state
        for desired_repo in desired {
            if !all_observed
                .iter()
                .any(|o| o.identity == desired_repo.identity)
            {
                // Repo not found on any forge - add empty observed
                all_observed.push(ObservedRepo::new(desired_repo.identity.clone()));
            }
        }

        Ok(all_observed)
    }

    /// Merge observed repo into existing list.
    fn merge_observed(&self, existing: &mut Vec<ObservedRepo>, new: ObservedRepo) {
        if let Some(existing_repo) = existing
            .iter_mut()
            .find(|o| o.identity == new.identity)
        {
            // Merge forge states
            for state in new.forge_states {
                *existing_repo = existing_repo.clone().with_forge_state(state);
            }
        } else {
            existing.push(new);
        }
    }

    /// Compute diff for a single repository.
    ///
    /// Useful when you only need to check one repo without computing
    /// the full org plan.
    pub async fn diff_repo(
        &self,
        org: &str,
        repo_name: &str,
        options: &DiffOptions,
    ) -> Result<Option<crate::domain::RepoDiff>, DiffError> {
        let plan = self.compute_plan(org, options).await?;
        Ok(plan.get_repo(repo_name).cloned())
    }

    /// Check if an org has any pending changes.
    ///
    /// Quick check without computing full plan details.
    pub async fn has_changes(&self, org: &str, options: &DiffOptions) -> Result<bool, DiffError> {
        let plan = self.compute_plan(org, options).await?;
        Ok(plan.has_changes())
    }

    /// List all configured organizations.
    pub async fn list_orgs(&self) -> Result<Vec<String>, DiffError> {
        self.storage
            .list_orgs()
            .await
            .map_err(DiffError::StorageError)
    }

    /// Compute plans for all configured organizations.
    pub async fn compute_all_plans(
        &self,
        options: &DiffOptions,
    ) -> Result<Vec<SyncPlan>, DiffError> {
        let orgs = self.list_orgs().await?;
        let mut plans = Vec::new();

        for org in orgs {
            match self.compute_plan(&org, options).await {
                Ok(plan) => plans.push(plan),
                Err(e) => {
                    // In production, might want to collect errors
                    let _ = e;
                }
            }
        }

        Ok(plans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::InMemoryStorageAdapter;
    use crate::domain::RepoIdentity;
    use crate::types::Visibility;

    fn test_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges
    }

    fn test_desired_repo(org: &str, name: &str) -> DesiredRepo {
        DesiredRepo::new(
            RepoIdentity::new(org, name),
            Visibility::Public,
            test_forges(),
        )
    }

    fn test_observed_repo(org: &str, name: &str) -> ObservedRepo {
        ObservedRepo::new(RepoIdentity::new(org, name)).with_forge_state(
            crate::domain::ForgeRepoState::found(
                Forge::GitHub,
                format!("https://github.com/{}/{}", org, name),
                Visibility::Public,
                None,
                None,
            ),
        )
    }

    #[tokio::test]
    async fn test_diff_service_creation() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = DiffService::new(vec![], storage);

        assert!(service.forges.is_empty());
    }

    #[tokio::test]
    async fn test_diff_options_builder() {
        let opts = DiffOptions::fresh().for_forge(Forge::GitHub).tracked_only();

        assert!(!opts.use_cached);
        assert!(opts.forges.contains(&Forge::GitHub));
        assert!(!opts.include_untracked);
    }

    #[tokio::test]
    async fn test_diff_options_cached() {
        let opts = DiffOptions::cached();

        assert!(opts.use_cached);
        assert!(opts.include_untracked);
    }

    #[tokio::test]
    async fn test_org_not_configured_error() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = DiffService::new(vec![], storage);

        let result = service
            .compute_plan("nonexistent", &DiffOptions::cached())
            .await;

        assert!(matches!(result, Err(DiffError::OrgNotConfigured { .. })));
    }

    #[tokio::test]
    async fn test_compute_plan_cached_empty() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        // Set up org with desired state
        storage
            .save_desired("testorg", &[test_desired_repo("testorg", "repo1")])
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        let plan = service
            .compute_plan("testorg", &DiffOptions::cached())
            .await
            .unwrap();

        assert_eq!(plan.org, "testorg");
        assert_eq!(plan.repo_diffs.len(), 1);
        // Should show repo needs to be created (no synced state)
        assert!(plan.summary.creates > 0);
    }

    #[tokio::test]
    async fn test_compute_plan_in_sync() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        let desired = test_desired_repo("testorg", "repo1");
        let observed = test_observed_repo("testorg", "repo1");

        storage.save_desired("testorg", &[desired]).await.unwrap();
        storage.save_synced("testorg", &[observed]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        let plan = service
            .compute_plan("testorg", &DiffOptions::cached())
            .await
            .unwrap();

        assert_eq!(plan.org, "testorg");
        assert!(!plan.has_changes());
        assert_eq!(plan.summary.in_sync, 1);
    }

    #[tokio::test]
    async fn test_has_changes() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        // New repo with no synced state = has changes
        storage
            .save_desired("testorg", &[test_desired_repo("testorg", "repo1")])
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        let has_changes = service
            .has_changes("testorg", &DiffOptions::cached())
            .await
            .unwrap();

        assert!(has_changes);
    }

    #[tokio::test]
    async fn test_list_orgs() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        storage
            .save_desired("org1", &[test_desired_repo("org1", "repo")])
            .await
            .unwrap();
        storage
            .save_desired("org2", &[test_desired_repo("org2", "repo")])
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        let orgs = service.list_orgs().await.unwrap();

        assert_eq!(orgs.len(), 2);
        assert!(orgs.contains(&"org1".to_string()));
        assert!(orgs.contains(&"org2".to_string()));
    }

    #[tokio::test]
    async fn test_diff_repo() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        storage
            .save_desired(
                "testorg",
                &[
                    test_desired_repo("testorg", "repo1"),
                    test_desired_repo("testorg", "repo2"),
                ],
            )
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        let diff = service
            .diff_repo("testorg", "repo1", &DiffOptions::cached())
            .await
            .unwrap();

        assert!(diff.is_some());
        assert_eq!(diff.unwrap().name(), "repo1");

        let diff_none = service
            .diff_repo("testorg", "nonexistent", &DiffOptions::cached())
            .await
            .unwrap();

        assert!(diff_none.is_none());
    }

    #[tokio::test]
    async fn test_tracked_only_filter() {
        let storage = Arc::new(InMemoryStorageAdapter::new());

        // Desired has repo1
        storage
            .save_desired("testorg", &[test_desired_repo("testorg", "repo1")])
            .await
            .unwrap();

        // Synced has repo1 and untracked repo2
        storage
            .save_synced(
                "testorg",
                &[
                    test_observed_repo("testorg", "repo1"),
                    test_observed_repo("testorg", "untracked"),
                ],
            )
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        // With untracked
        let plan_with = service
            .compute_plan("testorg", &DiffOptions::cached())
            .await
            .unwrap();
        assert_eq!(plan_with.summary.untracked, 1);

        // Without untracked
        let plan_without = service
            .compute_plan("testorg", &DiffOptions::cached().tracked_only())
            .await
            .unwrap();
        assert_eq!(plan_without.summary.untracked, 0);
    }
}
