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
    use crate::domain::{ForgeRepoState, RepoIdentity};
    use crate::types::Visibility;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    // =========================================================================
    // MockForgePort - records calls and returns configured responses
    // =========================================================================

    struct MockForgePort {
        forge_type: Forge,
        /// Repos to return from list_repos, keyed by org
        repos_by_org: Mutex<std::collections::HashMap<String, Vec<ObservedRepo>>>,
        /// Count of list_repos calls
        list_repos_calls: AtomicUsize,
    }

    impl MockForgePort {
        fn new(forge: Forge) -> Self {
            Self {
                forge_type: forge,
                repos_by_org: Mutex::new(std::collections::HashMap::new()),
                list_repos_calls: AtomicUsize::new(0),
            }
        }

        async fn set_repos(&self, org: &str, repos: Vec<ObservedRepo>) {
            let mut map = self.repos_by_org.lock().await;
            map.insert(org.to_string(), repos);
        }

        fn call_count(&self) -> usize {
            self.list_repos_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ForgePort for MockForgePort {
        fn forge_type(&self) -> Forge {
            self.forge_type.clone()
        }

        async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
            self.list_repos_calls.fetch_add(1, Ordering::SeqCst);

            let map = self.repos_by_org.lock().await;
            match map.get(org) {
                Some(repos) => Ok(repos.clone()),
                None => Err(ForgeError::OrgNotFound {
                    forge: self.forge_type.clone(),
                    org: org.to_string(),
                }),
            }
        }

        async fn create_repo(&self, _repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            unimplemented!("not needed for diff tests")
        }

        async fn update_repo(&self, _repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            unimplemented!("not needed for diff tests")
        }

        async fn delete_repo(&self, _identity: &crate::domain::RepoIdentity) -> Result<(), ForgeError> {
            unimplemented!("not needed for diff tests")
        }

        async fn repo_exists(&self, _identity: &crate::domain::RepoIdentity) -> Result<bool, ForgeError> {
            unimplemented!("not needed for diff tests")
        }
    }

    // =========================================================================
    // Test helpers
    // =========================================================================

    fn github_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges
    }

    fn github_codeberg_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        forges
    }

    fn make_desired(org: &str, name: &str) -> DesiredRepo {
        DesiredRepo::new(RepoIdentity::new(org, name), Visibility::Public, github_forges())
    }

    fn make_observed(org: &str, name: &str, forge: Forge) -> ObservedRepo {
        ObservedRepo::new(RepoIdentity::new(org, name)).with_forge_state(ForgeRepoState::found(
            forge.clone(),
            format!("https://{}/{}/{}", forge.ssh_host(), org, name),
            Visibility::Public,
            None,
            None,
        ))
    }

    // =========================================================================
    // Orchestration tests for DiffService
    // =========================================================================

    #[tokio::test]
    async fn test_compute_plan_with_two_creates_when_observed_is_empty() {
        // Given: desired=[A,B], observed=[]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage
            .save_desired("testorg", &[make_desired("testorg", "repo-a"), make_desired("testorg", "repo-b")])
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute plan using cached (empty observed)
        let plan = service.compute_plan("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: plan has 2 creates (one per repo on GitHub)
        assert_eq!(plan.summary.creates, 2);
        assert_eq!(plan.summary.in_sync, 0);
        assert_eq!(plan.repo_diffs.len(), 2);
        assert!(plan.has_changes());
    }

    #[tokio::test]
    async fn test_compute_plan_empty_when_fully_in_sync() {
        // Given: desired=[A], observed=[A] (both on GitHub)
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let desired = make_desired("testorg", "repo-a");
        let observed = make_observed("testorg", "repo-a", Forge::GitHub);

        storage.save_desired("testorg", &[desired]).await.unwrap();
        storage.save_synced("testorg", &[observed]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute plan
        let plan = service.compute_plan("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: plan has no changes (in sync)
        assert!(!plan.has_changes());
        assert_eq!(plan.summary.in_sync, 1);
        assert_eq!(plan.summary.creates, 0);
        assert_eq!(plan.summary.updates, 0);
    }

    #[tokio::test]
    async fn test_compute_plan_returns_org_not_configured_error() {
        // Given: storage has no orgs
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = DiffService::new(vec![], storage);

        // When: compute plan for nonexistent org
        let result = service.compute_plan("nonexistent", &DiffOptions::cached()).await;

        // Then: returns OrgNotConfigured error
        assert!(matches!(result, Err(DiffError::OrgNotConfigured { org }) if org == "nonexistent"));
    }

    #[tokio::test]
    async fn test_compute_plan_queries_forge_when_not_cached() {
        // Given: desired=[A] on GitHub, forge returns [A]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage
            .save_desired("testorg", &[make_desired("testorg", "repo-a")])
            .await
            .unwrap();

        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos("testorg", vec![make_observed("testorg", "repo-a", Forge::GitHub)])
            .await;

        let service = DiffService::new(vec![mock_forge.clone()], storage);

        // When: compute plan with fresh (queries forge)
        let plan = service.compute_plan("testorg", &DiffOptions::fresh()).await.unwrap();

        // Then: forge was queried and plan shows in sync
        assert_eq!(mock_forge.call_count(), 1);
        assert!(!plan.has_changes());
        assert_eq!(plan.summary.in_sync, 1);
    }

    #[tokio::test]
    async fn test_compute_plan_shows_creates_when_forge_returns_empty() {
        // Given: desired=[A,B] on GitHub, forge returns []
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage
            .save_desired("testorg", &[make_desired("testorg", "repo-a"), make_desired("testorg", "repo-b")])
            .await
            .unwrap();

        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge.set_repos("testorg", vec![]).await;

        let service = DiffService::new(vec![mock_forge.clone()], storage);

        // When: compute plan with fresh options
        let plan = service.compute_plan("testorg", &DiffOptions::fresh()).await.unwrap();

        // Then: plan shows 2 creates needed
        assert_eq!(plan.summary.creates, 2);
        assert!(plan.has_changes());
    }

    #[tokio::test]
    async fn test_compute_plan_handles_forge_org_not_found_gracefully() {
        // Given: desired=[A], forge returns OrgNotFound
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("testorg", &[make_desired("testorg", "repo-a")]).await.unwrap();

        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        // Don't set any repos - will return OrgNotFound

        let service = DiffService::new(vec![mock_forge.clone()], storage);

        // When: compute plan - forge says org not found
        let plan = service.compute_plan("testorg", &DiffOptions::fresh()).await.unwrap();

        // Then: plan still works, shows create needed
        assert_eq!(plan.summary.creates, 1);
        assert!(plan.has_changes());
    }

    #[tokio::test]
    async fn test_compute_all_plans_aggregates_across_orgs() {
        // Given: two orgs with desired repos
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("org1", &[make_desired("org1", "repo-a")]).await.unwrap();
        storage.save_desired("org2", &[make_desired("org2", "repo-b"), make_desired("org2", "repo-c")]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute all plans
        let plans = service.compute_all_plans(&DiffOptions::cached()).await.unwrap();

        // Then: returns plans for both orgs
        assert_eq!(plans.len(), 2);

        let org1_plan = plans.iter().find(|p| p.org == "org1").unwrap();
        let org2_plan = plans.iter().find(|p| p.org == "org2").unwrap();

        assert_eq!(org1_plan.summary.creates, 1);
        assert_eq!(org2_plan.summary.creates, 2);
    }

    #[tokio::test]
    async fn test_compute_plan_detects_untracked_repos() {
        // Given: desired=[A], synced=[A, B] (B is untracked)
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("testorg", &[make_desired("testorg", "repo-a")]).await.unwrap();
        storage
            .save_synced(
                "testorg",
                &[
                    make_observed("testorg", "repo-a", Forge::GitHub),
                    make_observed("testorg", "repo-b", Forge::GitHub),
                ],
            )
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute plan with include_untracked=true (default)
        let plan = service.compute_plan("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: untracked count is 1
        assert_eq!(plan.summary.untracked, 1);
        assert_eq!(plan.summary.in_sync, 1);

        // When: compute plan with tracked_only
        let plan_tracked = service
            .compute_plan("testorg", &DiffOptions::cached().tracked_only())
            .await
            .unwrap();

        // Then: untracked is filtered out
        assert_eq!(plan_tracked.summary.untracked, 0);
        assert_eq!(plan_tracked.repo_diffs.len(), 1);
    }

    #[tokio::test]
    async fn test_compute_plan_detects_visibility_change() {
        // Given: desired=[A with public], synced=[A with private]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let desired = DesiredRepo::new(
            RepoIdentity::new("testorg", "repo-a"),
            Visibility::Public, // Want public
            github_forges(),
        );

        let observed = ObservedRepo::new(RepoIdentity::new("testorg", "repo-a")).with_forge_state(
            ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/testorg/repo-a".to_string(),
                Visibility::Private, // Currently private
                None,
                None,
            ),
        );

        storage.save_desired("testorg", &[desired]).await.unwrap();
        storage.save_synced("testorg", &[observed]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute plan
        let plan = service.compute_plan("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: shows update needed
        assert_eq!(plan.summary.updates, 1);
        assert!(plan.has_changes());
    }

    #[tokio::test]
    async fn test_diff_repo_returns_specific_repo_diff() {
        // Given: multiple repos
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage
            .save_desired(
                "testorg",
                &[make_desired("testorg", "repo-a"), make_desired("testorg", "repo-b")],
            )
            .await
            .unwrap();

        let service = DiffService::new(vec![], storage);

        // When: diff specific repo
        let diff = service.diff_repo("testorg", "repo-a", &DiffOptions::cached()).await.unwrap();

        // Then: returns only that repo's diff
        assert!(diff.is_some());
        assert_eq!(diff.unwrap().name(), "repo-a");

        // And: nonexistent repo returns None
        let no_diff = service.diff_repo("testorg", "nonexistent", &DiffOptions::cached()).await.unwrap();
        assert!(no_diff.is_none());
    }

    #[tokio::test]
    async fn test_has_changes_returns_true_when_creates_needed() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("testorg", &[make_desired("testorg", "repo-a")]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: check has_changes (no synced state = creates needed)
        let has_changes = service.has_changes("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: true
        assert!(has_changes);
    }

    #[tokio::test]
    async fn test_has_changes_returns_false_when_in_sync() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let desired = make_desired("testorg", "repo-a");
        let observed = make_observed("testorg", "repo-a", Forge::GitHub);

        storage.save_desired("testorg", &[desired]).await.unwrap();
        storage.save_synced("testorg", &[observed]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: check has_changes
        let has_changes = service.has_changes("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: false
        assert!(!has_changes);
    }

    #[tokio::test]
    async fn test_list_orgs_returns_all_configured_orgs() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("alpha", &[make_desired("alpha", "repo")]).await.unwrap();
        storage.save_desired("beta", &[make_desired("beta", "repo")]).await.unwrap();
        storage.save_desired("gamma", &[make_desired("gamma", "repo")]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        let orgs = service.list_orgs().await.unwrap();

        assert_eq!(orgs.len(), 3);
        assert!(orgs.contains(&"alpha".to_string()));
        assert!(orgs.contains(&"beta".to_string()));
        assert!(orgs.contains(&"gamma".to_string()));
    }

    #[tokio::test]
    async fn test_compute_plan_with_multiple_forges() {
        // Given: desired repo on both GitHub and Codeberg
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let desired = DesiredRepo::new(
            RepoIdentity::new("testorg", "repo-a"),
            Visibility::Public,
            github_codeberg_forges(),
        );
        storage.save_desired("testorg", &[desired]).await.unwrap();

        // Synced only on GitHub
        let observed = make_observed("testorg", "repo-a", Forge::GitHub);
        storage.save_synced("testorg", &[observed]).await.unwrap();

        let service = DiffService::new(vec![], storage);

        // When: compute plan
        let plan = service.compute_plan("testorg", &DiffOptions::cached()).await.unwrap();

        // Then: shows create on Codeberg, in sync on GitHub
        assert_eq!(plan.summary.creates, 1);
        assert_eq!(plan.summary.in_sync, 0); // The repo overall needs action
        assert!(plan.has_changes());
    }

    #[tokio::test]
    async fn test_compute_plan_no_forges_configured_error() {
        // Given: org exists with desired repos
        let storage = Arc::new(InMemoryStorageAdapter::new());
        storage.save_desired("testorg", &[make_desired("testorg", "repo-a")]).await.unwrap();

        // No forge adapters configured
        let service = DiffService::new(vec![], storage);

        // When: compute plan with fresh (requires forge query)
        let result = service.compute_plan("testorg", &DiffOptions::fresh()).await;

        // Then: returns NoForgesConfigured error
        assert!(matches!(result, Err(DiffError::NoForgesConfigured)));
    }
}
