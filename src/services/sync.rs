//! Sync service - executes sync plans against forges.
//!
//! This service orchestrates the execution of a `SyncPlan`, applying
//! creates, updates, and deletes to the appropriate forges via `ForgePort`
//! and recording results via `StoragePort`.

use std::sync::Arc;
use thiserror::Error;

use crate::domain::{DesiredRepo, ForgeAction, ObservedRepo, RepoDiff, SyncPlan};
use crate::ports::{ForgeError, ForgePort, StorageError, StoragePort};
use crate::types::Forge;

/// Errors that can occur during sync operations.
#[derive(Debug, Error)]
pub enum SyncError {
    /// A forge operation failed
    #[error("Forge operation failed: {0}")]
    ForgeError(#[from] ForgeError),

    /// A storage operation failed
    #[error("Storage operation failed: {0}")]
    StorageError(#[from] StorageError),

    /// No forge adapter available for the requested forge
    #[error("No adapter available for forge: {forge}")]
    NoAdapterForForge { forge: Forge },

    /// Sync was aborted due to errors
    #[error("Sync aborted: {message}")]
    Aborted { message: String },

    /// Dry run completed (not an actual error)
    #[error("Dry run completed - no changes applied")]
    DryRun,
}

/// Result of applying a single action.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// The repository this result is for
    pub repo_name: String,
    /// The forge the action was applied to
    pub forge: Forge,
    /// The outcome of the action
    pub outcome: SyncOutcome,
}

/// Outcome of a sync action.
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Repository was created successfully
    Created,
    /// Repository was updated successfully
    Updated,
    /// Repository was deleted successfully
    Deleted,
    /// No action was needed
    NoOp,
    /// Action was skipped (dry run)
    Skipped,
    /// Action failed with an error
    Failed { error: String },
}

impl SyncOutcome {
    /// Check if this outcome represents success
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            SyncOutcome::Created
                | SyncOutcome::Updated
                | SyncOutcome::Deleted
                | SyncOutcome::NoOp
                | SyncOutcome::Skipped
        )
    }

    /// Check if this outcome represents a failure
    pub fn is_failure(&self) -> bool {
        matches!(self, SyncOutcome::Failed { .. })
    }
}

/// Service for executing sync plans.
///
/// Coordinates between forge adapters and storage to apply changes
/// and record results.
pub struct SyncService {
    /// Forge adapters keyed by forge type
    forges: Vec<Arc<dyn ForgePort>>,
    /// Storage adapter for persisting state
    storage: Arc<dyn StoragePort>,
}

impl SyncService {
    /// Create a new sync service with the given ports.
    ///
    /// # Arguments
    ///
    /// * `forges` - List of forge adapters to use
    /// * `storage` - Storage adapter for persisting state
    pub fn new(forges: Vec<Arc<dyn ForgePort>>, storage: Arc<dyn StoragePort>) -> Self {
        Self { forges, storage }
    }

    /// Get the forge adapter for a specific forge type.
    fn get_forge_adapter(&self, forge: &Forge) -> Option<&Arc<dyn ForgePort>> {
        self.forges.iter().find(|f| &f.forge_type() == forge)
    }

    /// Execute a sync plan.
    ///
    /// Applies all actions in the plan to the appropriate forges.
    /// If `dry_run` is true, no changes are actually made.
    ///
    /// # Arguments
    ///
    /// * `plan` - The sync plan to execute
    /// * `dry_run` - If true, don't actually apply changes
    ///
    /// # Returns
    ///
    /// A list of results for each action in the plan.
    pub async fn execute_plan(
        &self,
        plan: &SyncPlan,
        dry_run: bool,
    ) -> Result<Vec<SyncResult>, SyncError> {
        let mut results = Vec::new();

        for diff in &plan.repo_diffs {
            let diff_results = self.execute_diff(diff, dry_run).await;
            results.extend(diff_results);
        }

        // If not a dry run and we had successful changes, update synced state
        if !dry_run {
            let successful_repos = self.collect_successful_observed(&results, plan).await?;
            if !successful_repos.is_empty() {
                self.storage
                    .save_synced(&plan.org, &successful_repos)
                    .await?;
            }
        }

        Ok(results)
    }

    /// Execute actions for a single repository diff.
    async fn execute_diff(&self, diff: &RepoDiff, dry_run: bool) -> Vec<SyncResult> {
        let mut results = Vec::new();

        for action in &diff.forge_actions {
            let result = self.execute_action(&diff.identity.name, action, dry_run).await;
            results.push(result);
        }

        results
    }

    /// Execute a single forge action.
    async fn execute_action(
        &self,
        repo_name: &str,
        action: &ForgeAction,
        dry_run: bool,
    ) -> SyncResult {
        let forge = action.forge().clone();

        if dry_run {
            return SyncResult {
                repo_name: repo_name.to_string(),
                forge,
                outcome: SyncOutcome::Skipped,
            };
        }

        if action.is_noop() {
            return SyncResult {
                repo_name: repo_name.to_string(),
                forge,
                outcome: SyncOutcome::NoOp,
            };
        }

        let adapter = match self.get_forge_adapter(&forge) {
            Some(adapter) => adapter,
            None => {
                return SyncResult {
                    repo_name: repo_name.to_string(),
                    forge: forge.clone(),
                    outcome: SyncOutcome::Failed {
                        error: format!("No adapter available for forge: {}", forge),
                    },
                };
            }
        };

        let outcome = match action {
            ForgeAction::Create {
                forge: _,
                visibility,
                description,
            } => {
                // We need to construct a DesiredRepo to pass to create_repo
                // For now, we'll use a minimal approach - in practice, we'd get this from the diff
                self.execute_create(adapter.as_ref(), repo_name, visibility, description)
                    .await
            }
            ForgeAction::Update { forge: _, changes } => {
                self.execute_update(adapter.as_ref(), repo_name, changes)
                    .await
            }
            ForgeAction::Delete { forge: _, url } => {
                self.execute_delete(adapter.as_ref(), repo_name, url.as_deref())
                    .await
            }
            ForgeAction::NoOp { .. } => SyncOutcome::NoOp,
        };

        SyncResult {
            repo_name: repo_name.to_string(),
            forge,
            outcome,
        }
    }

    /// Execute a create action.
    async fn execute_create(
        &self,
        _adapter: &dyn ForgePort,
        _repo_name: &str,
        _visibility: &crate::types::Visibility,
        _description: &Option<String>,
    ) -> SyncOutcome {
        // Note: Full implementation would construct a DesiredRepo and call adapter.create_repo()
        // For now, we return a placeholder - the actual implementation depends on
        // having access to the full DesiredRepo from the plan context
        SyncOutcome::Failed {
            error: "Create not yet implemented - needs DesiredRepo context".to_string(),
        }
    }

    /// Execute an update action.
    async fn execute_update(
        &self,
        _adapter: &dyn ForgePort,
        _repo_name: &str,
        _changes: &crate::domain::PropertyChanges,
    ) -> SyncOutcome {
        // Note: Full implementation would call adapter.update_repo()
        SyncOutcome::Failed {
            error: "Update not yet implemented - needs DesiredRepo context".to_string(),
        }
    }

    /// Execute a delete action.
    async fn execute_delete(
        &self,
        _adapter: &dyn ForgePort,
        _repo_name: &str,
        _url: Option<&str>,
    ) -> SyncOutcome {
        // Note: Full implementation would call adapter.delete_repo()
        SyncOutcome::Failed {
            error: "Delete not yet implemented - needs confirmation flow".to_string(),
        }
    }

    /// Collect observed state for successfully synced repos.
    async fn collect_successful_observed(
        &self,
        results: &[SyncResult],
        plan: &SyncPlan,
    ) -> Result<Vec<ObservedRepo>, SyncError> {
        // Get the previously synced state as a starting point
        let synced = self
            .storage
            .load_synced(&plan.org)
            .await
            .unwrap_or_default();

        // For each successful result, we'd query the forge for current state
        // For now, we just return the existing synced state
        // Full implementation would update based on successful operations

        for result in results {
            if result.outcome.is_success() && !matches!(result.outcome, SyncOutcome::Skipped) {
                // In full implementation: query forge for observed state and update synced
                let _ = result; // Placeholder
            }
        }

        Ok(synced)
    }

    /// Execute a sync plan with full context.
    ///
    /// This variant takes both the plan and the desired repos, allowing
    /// access to full repo configuration during create/update operations.
    pub async fn execute_plan_with_context(
        &self,
        plan: &SyncPlan,
        desired: &[DesiredRepo],
        dry_run: bool,
    ) -> Result<Vec<SyncResult>, SyncError> {
        let mut results = Vec::new();

        // Build a lookup map for desired repos
        let desired_map: std::collections::HashMap<_, _> = desired
            .iter()
            .map(|d| (&d.identity, d))
            .collect();

        for diff in &plan.repo_diffs {
            let desired_repo = desired_map.get(&diff.identity);
            let diff_results = self
                .execute_diff_with_context(diff, desired_repo.copied(), dry_run)
                .await;
            results.extend(diff_results);
        }

        // Update synced state if not a dry run
        if !dry_run {
            self.update_synced_state(&plan.org, &results, desired).await?;
        }

        Ok(results)
    }

    /// Execute diff with access to desired repo context.
    async fn execute_diff_with_context(
        &self,
        diff: &RepoDiff,
        desired: Option<&DesiredRepo>,
        dry_run: bool,
    ) -> Vec<SyncResult> {
        let mut results = Vec::new();

        for action in &diff.forge_actions {
            let result = self
                .execute_action_with_context(&diff.identity.name, action, desired, dry_run)
                .await;
            results.push(result);
        }

        results
    }

    /// Execute a single action with desired repo context.
    async fn execute_action_with_context(
        &self,
        repo_name: &str,
        action: &ForgeAction,
        desired: Option<&DesiredRepo>,
        dry_run: bool,
    ) -> SyncResult {
        let forge = action.forge().clone();

        if dry_run {
            return SyncResult {
                repo_name: repo_name.to_string(),
                forge,
                outcome: SyncOutcome::Skipped,
            };
        }

        if action.is_noop() {
            return SyncResult {
                repo_name: repo_name.to_string(),
                forge,
                outcome: SyncOutcome::NoOp,
            };
        }

        let adapter = match self.get_forge_adapter(&forge) {
            Some(adapter) => adapter,
            None => {
                return SyncResult {
                    repo_name: repo_name.to_string(),
                    forge: forge.clone(),
                    outcome: SyncOutcome::Failed {
                        error: format!("No adapter available for forge: {}", forge),
                    },
                };
            }
        };

        let outcome = match action {
            ForgeAction::Create { .. } => {
                if let Some(desired) = desired {
                    match adapter.create_repo(desired).await {
                        Ok(_) => SyncOutcome::Created,
                        Err(e) => SyncOutcome::Failed {
                            error: e.to_string(),
                        },
                    }
                } else {
                    SyncOutcome::Failed {
                        error: "No desired repo context for create".to_string(),
                    }
                }
            }
            ForgeAction::Update { .. } => {
                if let Some(desired) = desired {
                    match adapter.update_repo(desired).await {
                        Ok(_) => SyncOutcome::Updated,
                        Err(e) => SyncOutcome::Failed {
                            error: e.to_string(),
                        },
                    }
                } else {
                    SyncOutcome::Failed {
                        error: "No desired repo context for update".to_string(),
                    }
                }
            }
            ForgeAction::Delete { .. } => {
                // For delete, we use the identity from diff context
                let identity = crate::domain::RepoIdentity::new(
                    desired.map(|d| d.org()).unwrap_or("unknown"),
                    repo_name,
                );
                match adapter.delete_repo(&identity).await {
                    Ok(_) => SyncOutcome::Deleted,
                    Err(e) => SyncOutcome::Failed {
                        error: e.to_string(),
                    },
                }
            }
            ForgeAction::NoOp { .. } => SyncOutcome::NoOp,
        };

        SyncResult {
            repo_name: repo_name.to_string(),
            forge,
            outcome,
        }
    }

    /// Update synced state after successful operations.
    async fn update_synced_state(
        &self,
        org: &str,
        results: &[SyncResult],
        desired: &[DesiredRepo],
    ) -> Result<(), SyncError> {
        // Build observed state from successful operations
        let mut observed_repos: Vec<ObservedRepo> = Vec::new();

        // Group results by repo name
        let mut repo_results: std::collections::HashMap<&str, Vec<&SyncResult>> =
            std::collections::HashMap::new();
        for result in results {
            repo_results
                .entry(&result.repo_name)
                .or_default()
                .push(result);
        }

        // For each repo that had successful operations, create observed state
        for (repo_name, results) in repo_results {
            // Find the desired repo to get the identity
            let desired_repo = desired.iter().find(|d| d.name() == repo_name);
            if let Some(desired) = desired_repo {
                let mut observed = ObservedRepo::new(desired.identity.clone());

                for result in results {
                    if result.outcome.is_success() && !matches!(result.outcome, SyncOutcome::Skipped) {
                        // In a full implementation, we'd query the forge for actual state
                        // For now, we assume the desired state was applied
                        let forge_state = crate::domain::ForgeRepoState::found(
                            result.forge.clone(),
                            format!("https://{}/{}/{}", result.forge.ssh_host(), org, repo_name),
                            desired.visibility.clone(),
                            None,
                            desired.description.clone(),
                        );
                        observed = observed.with_forge_state(forge_state);
                    }
                }

                if !observed.forge_states.is_empty() {
                    observed_repos.push(observed);
                }
            }
        }

        if !observed_repos.is_empty() {
            self.storage.save_synced(org, &observed_repos).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::InMemoryStorageAdapter;
    use crate::domain::{ForgeRepoState, RepoIdentity};
    use crate::types::Visibility;
    use async_trait::async_trait;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    // =========================================================================
    // MockForgePort - records calls and returns configured responses
    // =========================================================================

    struct MockForgePort {
        forge_type: Forge,
        /// Recorded create_repo calls
        create_calls: Mutex<Vec<DesiredRepo>>,
        /// Recorded update_repo calls
        update_calls: Mutex<Vec<DesiredRepo>>,
        /// Recorded delete_repo calls
        delete_calls: Mutex<Vec<RepoIdentity>>,
        /// Error to return from create_repo (if set)
        create_error: Mutex<Option<ForgeError>>,
        /// Error to return from update_repo (if set)
        update_error: Mutex<Option<ForgeError>>,
        /// Error to return from delete_repo (if set)
        delete_error: Mutex<Option<ForgeError>>,
        /// Count of operations
        operation_count: AtomicUsize,
    }

    impl MockForgePort {
        fn new(forge: Forge) -> Self {
            Self {
                forge_type: forge,
                create_calls: Mutex::new(Vec::new()),
                update_calls: Mutex::new(Vec::new()),
                delete_calls: Mutex::new(Vec::new()),
                create_error: Mutex::new(None),
                update_error: Mutex::new(None),
                delete_error: Mutex::new(None),
                operation_count: AtomicUsize::new(0),
            }
        }

        async fn set_create_error(&self, error: ForgeError) {
            let mut err = self.create_error.lock().await;
            *err = Some(error);
        }

        async fn set_update_error(&self, error: ForgeError) {
            let mut err = self.update_error.lock().await;
            *err = Some(error);
        }

        async fn set_delete_error(&self, error: ForgeError) {
            let mut err = self.delete_error.lock().await;
            *err = Some(error);
        }

        async fn create_call_count(&self) -> usize {
            self.create_calls.lock().await.len()
        }

        async fn update_call_count(&self) -> usize {
            self.update_calls.lock().await.len()
        }

        async fn delete_call_count(&self) -> usize {
            self.delete_calls.lock().await.len()
        }

        async fn get_created_repos(&self) -> Vec<DesiredRepo> {
            self.create_calls.lock().await.clone()
        }

        async fn get_updated_repos(&self) -> Vec<DesiredRepo> {
            self.update_calls.lock().await.clone()
        }

        async fn get_deleted_repos(&self) -> Vec<RepoIdentity> {
            self.delete_calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl ForgePort for MockForgePort {
        fn forge_type(&self) -> Forge {
            self.forge_type.clone()
        }

        async fn list_repos(&self, _org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
            Ok(vec![])
        }

        async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);

            // Check for configured error
            let err = self.create_error.lock().await;
            if let Some(e) = &*err {
                return Err(ForgeError::api_error(self.forge_type.clone(), e.to_string()));
            }
            drop(err);

            // Record the call
            let mut calls = self.create_calls.lock().await;
            calls.push(repo.clone());

            // Return success
            Ok(ObservedRepo::new(repo.identity.clone()).with_forge_state(ForgeRepoState::found(
                self.forge_type.clone(),
                format!("https://{}/{}/{}", self.forge_type.ssh_host(), repo.org(), repo.name()),
                repo.visibility.clone(),
                Some("new-id".to_string()),
                repo.description.clone(),
            )))
        }

        async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);

            // Check for configured error
            let err = self.update_error.lock().await;
            if let Some(e) = &*err {
                return Err(ForgeError::api_error(self.forge_type.clone(), e.to_string()));
            }
            drop(err);

            // Record the call
            let mut calls = self.update_calls.lock().await;
            calls.push(repo.clone());

            // Return success
            Ok(ObservedRepo::new(repo.identity.clone()).with_forge_state(ForgeRepoState::found(
                self.forge_type.clone(),
                format!("https://{}/{}/{}", self.forge_type.ssh_host(), repo.org(), repo.name()),
                repo.visibility.clone(),
                Some("updated-id".to_string()),
                repo.description.clone(),
            )))
        }

        async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError> {
            self.operation_count.fetch_add(1, Ordering::SeqCst);

            // Check for configured error
            let err = self.delete_error.lock().await;
            if let Some(e) = &*err {
                return Err(ForgeError::api_error(self.forge_type.clone(), e.to_string()));
            }
            drop(err);

            // Record the call
            let mut calls = self.delete_calls.lock().await;
            calls.push(identity.clone());

            Ok(())
        }

        async fn repo_exists(&self, _identity: &RepoIdentity) -> Result<bool, ForgeError> {
            Ok(false)
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
    // SyncService orchestration tests
    // =========================================================================

    #[tokio::test]
    async fn test_execute_plan_calls_create_for_create_actions() {
        // Given: plan has create action for repo
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "new-repo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan with context
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: create_repo was called
        assert_eq!(mock_forge.create_call_count().await, 1);
        let created = mock_forge.get_created_repos().await;
        assert_eq!(created[0].name(), "new-repo");

        // And: result shows Created
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, SyncOutcome::Created));
    }

    #[tokio::test]
    async fn test_execute_plan_calls_update_for_update_actions() {
        // Given: plan has update action (visibility change)
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        // Desired: Public, Observed: Private (need update)
        let desired = vec![DesiredRepo::new(
            RepoIdentity::new("testorg", "repo"),
            Visibility::Public,
            github_forges(),
        )];
        let observed = vec![ObservedRepo::new(RepoIdentity::new("testorg", "repo")).with_forge_state(
            ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/testorg/repo".to_string(),
                Visibility::Private, // Different from desired
                None,
                None,
            ),
        )];
        let plan = SyncPlan::from_diff("testorg", &desired, &observed);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: update_repo was called
        assert_eq!(mock_forge.update_call_count().await, 1);
        let updated = mock_forge.get_updated_repos().await;
        assert_eq!(updated[0].name(), "repo");

        // And: result shows Updated
        assert!(results.iter().any(|r| matches!(r.outcome, SyncOutcome::Updated)));
    }

    #[tokio::test]
    async fn test_execute_plan_calls_delete_for_delete_actions() {
        // Given: plan has delete action (repo marked for deletion)
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        // Desired: marked for deletion, Observed: exists
        let desired = vec![make_desired("testorg", "to-delete").with_deletion_mark(true)];
        let observed = vec![make_observed("testorg", "to-delete", Forge::GitHub)];
        let plan = SyncPlan::from_diff("testorg", &desired, &observed);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: delete_repo was called
        assert_eq!(mock_forge.delete_call_count().await, 1);
        let deleted = mock_forge.get_deleted_repos().await;
        assert_eq!(deleted[0].name, "to-delete");

        // And: result shows Deleted
        assert!(results.iter().any(|r| matches!(r.outcome, SyncOutcome::Deleted)));
    }

    #[tokio::test]
    async fn test_execute_plan_noop_does_not_call_forge() {
        // Given: plan is in sync (no ops needed)
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "repo")];
        let observed = vec![make_observed("testorg", "repo", Forge::GitHub)];
        let plan = SyncPlan::from_diff("testorg", &desired, &observed);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: no forge operations called
        assert_eq!(mock_forge.create_call_count().await, 0);
        assert_eq!(mock_forge.update_call_count().await, 0);
        assert_eq!(mock_forge.delete_call_count().await, 0);

        // And: result shows NoOp
        assert!(results.iter().all(|r| matches!(r.outcome, SyncOutcome::NoOp)));
    }

    #[tokio::test]
    async fn test_execute_plan_dry_run_skips_all_operations() {
        // Given: plan has create action
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "new-repo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute with dry_run=true
        let results = service
            .execute_plan_with_context(&plan, &desired, true)
            .await
            .unwrap();

        // Then: no forge operations called
        assert_eq!(mock_forge.create_call_count().await, 0);

        // And: all results are Skipped
        for result in results {
            assert!(matches!(result.outcome, SyncOutcome::Skipped));
        }
    }

    #[tokio::test]
    async fn test_execute_plan_partial_failure_reports_correctly() {
        // Given: plan has two creates, second one will fail
        let storage = Arc::new(InMemoryStorageAdapter::new());

        // Create two forge adapters - one succeeds, one fails
        let github_success = Arc::new(MockForgePort::new(Forge::GitHub));
        let codeberg_fail = Arc::new(MockForgePort::new(Forge::Codeberg));
        codeberg_fail
            .set_create_error(ForgeError::api_error(Forge::Codeberg, "rate limited"))
            .await;

        let service = SyncService::new(vec![github_success.clone(), codeberg_fail.clone()], storage);

        // Desired on both forges
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);
        let desired = vec![DesiredRepo::new(
            RepoIdentity::new("testorg", "new-repo"),
            Visibility::Public,
            forges,
        )];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: GitHub succeeded, Codeberg failed
        let github_result = results.iter().find(|r| r.forge == Forge::GitHub).unwrap();
        let codeberg_result = results.iter().find(|r| r.forge == Forge::Codeberg).unwrap();

        assert!(matches!(github_result.outcome, SyncOutcome::Created));
        assert!(matches!(codeberg_result.outcome, SyncOutcome::Failed { .. }));

        // And: forge was still called for both
        assert_eq!(github_success.create_call_count().await, 1);
        // Codeberg also attempted (but failed)
        assert_eq!(codeberg_fail.create_call_count().await, 0); // Error before call recorded
    }

    #[tokio::test]
    async fn test_execute_plan_no_adapter_returns_failed() {
        // Given: plan needs GitHub but no GitHub adapter configured
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let codeberg_only = Arc::new(MockForgePort::new(Forge::Codeberg));

        let service = SyncService::new(vec![codeberg_only], storage);

        // Desired on GitHub (no adapter for this)
        let desired = vec![make_desired("testorg", "repo")]; // GitHub only
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: result shows Failed with "No adapter" message
        assert_eq!(results.len(), 1);
        match &results[0].outcome {
            SyncOutcome::Failed { error } => {
                assert!(error.contains("No adapter"));
            }
            _ => panic!("Expected Failed outcome"),
        }
    }

    #[tokio::test]
    async fn test_execute_plan_create_failure_reports_error() {
        // Given: forge will fail on create
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_create_error(ForgeError::api_error(Forge::GitHub, "quota exceeded"))
            .await;

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "repo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: result shows Failed
        assert!(results[0].outcome.is_failure());
        match &results[0].outcome {
            SyncOutcome::Failed { error } => {
                assert!(error.contains("quota exceeded"));
            }
            _ => panic!("Expected Failed outcome"),
        }
    }

    #[tokio::test]
    async fn test_execute_plan_update_failure_reports_error() {
        // Given: forge will fail on update
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_update_error(ForgeError::api_error(Forge::GitHub, "permission denied"))
            .await;

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        // Visibility change = update
        let desired = vec![DesiredRepo::new(
            RepoIdentity::new("testorg", "repo"),
            Visibility::Public,
            github_forges(),
        )];
        let observed = vec![ObservedRepo::new(RepoIdentity::new("testorg", "repo")).with_forge_state(
            ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/testorg/repo".to_string(),
                Visibility::Private,
                None,
                None,
            ),
        )];
        let plan = SyncPlan::from_diff("testorg", &desired, &observed);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: shows failure
        let update_result = results.iter().find(|r| r.forge == Forge::GitHub).unwrap();
        assert!(update_result.outcome.is_failure());
    }

    #[tokio::test]
    async fn test_execute_plan_delete_failure_reports_error() {
        // Given: forge will fail on delete
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_delete_error(ForgeError::api_error(Forge::GitHub, "delete not allowed"))
            .await;

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "repo").with_deletion_mark(true)];
        let observed = vec![make_observed("testorg", "repo", Forge::GitHub)];
        let plan = SyncPlan::from_diff("testorg", &desired, &observed);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: shows failure
        let delete_result = results.iter().find(|r| r.forge == Forge::GitHub).unwrap();
        assert!(delete_result.outcome.is_failure());
    }

    #[tokio::test]
    async fn test_sync_outcome_success_and_failure_helpers() {
        // Success cases
        assert!(SyncOutcome::Created.is_success());
        assert!(SyncOutcome::Updated.is_success());
        assert!(SyncOutcome::Deleted.is_success());
        assert!(SyncOutcome::NoOp.is_success());
        assert!(SyncOutcome::Skipped.is_success());

        // Failure cases
        assert!(!SyncOutcome::Created.is_failure());
        assert!(!SyncOutcome::Updated.is_failure());
        assert!(!SyncOutcome::NoOp.is_failure());
        assert!(!SyncOutcome::Skipped.is_failure());

        let failed = SyncOutcome::Failed {
            error: "test error".to_string(),
        };
        assert!(!failed.is_success());
        assert!(failed.is_failure());
    }

    #[tokio::test]
    async fn test_execute_plan_multiple_repos() {
        // Given: plan with multiple repos to create
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![
            make_desired("testorg", "repo-1"),
            make_desired("testorg", "repo-2"),
            make_desired("testorg", "repo-3"),
        ];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: all repos created
        assert_eq!(mock_forge.create_call_count().await, 3);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| matches!(r.outcome, SyncOutcome::Created)));
    }

    #[tokio::test]
    async fn test_execute_plan_result_contains_repo_and_forge_info() {
        // Given: plan with action
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));

        let service = SyncService::new(vec![mock_forge.clone()], storage);

        let desired = vec![make_desired("testorg", "my-repo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        // When: execute plan
        let results = service
            .execute_plan_with_context(&plan, &desired, false)
            .await
            .unwrap();

        // Then: result contains correct metadata
        assert_eq!(results[0].repo_name, "my-repo");
        assert_eq!(results[0].forge, Forge::GitHub);
    }
}
