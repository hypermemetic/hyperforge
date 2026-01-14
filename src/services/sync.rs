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
    use crate::domain::{DesiredRepo, RepoIdentity};
    use crate::types::Visibility;
    use std::collections::HashSet;

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

    #[tokio::test]
    async fn test_sync_service_creation() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = SyncService::new(vec![], storage);

        // Service should be created successfully
        assert!(service.forges.is_empty());
    }

    #[tokio::test]
    async fn test_dry_run_returns_skipped() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = SyncService::new(vec![], storage);

        let desired = vec![test_desired_repo("testorg", "testrepo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        let results = service.execute_plan(&plan, true).await.unwrap();

        // All results should be skipped in dry run
        for result in results {
            assert!(matches!(result.outcome, SyncOutcome::Skipped));
        }
    }

    #[tokio::test]
    async fn test_no_adapter_returns_failed() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        // Empty forges - no adapters
        let service = SyncService::new(vec![], storage);

        let desired = vec![test_desired_repo("testorg", "testrepo")];
        let plan = SyncPlan::from_diff("testorg", &desired, &[]);

        let results = service.execute_plan(&plan, false).await.unwrap();

        // All results should be failed due to no adapter
        for result in results {
            assert!(result.outcome.is_failure());
        }
    }

    #[tokio::test]
    async fn test_sync_outcome_is_success() {
        assert!(SyncOutcome::Created.is_success());
        assert!(SyncOutcome::Updated.is_success());
        assert!(SyncOutcome::Deleted.is_success());
        assert!(SyncOutcome::NoOp.is_success());
        assert!(SyncOutcome::Skipped.is_success());
        assert!(!SyncOutcome::Failed {
            error: "test".to_string()
        }
        .is_success());
    }

    #[tokio::test]
    async fn test_sync_outcome_is_failure() {
        assert!(!SyncOutcome::Created.is_failure());
        assert!(!SyncOutcome::NoOp.is_failure());
        assert!(SyncOutcome::Failed {
            error: "test".to_string()
        }
        .is_failure());
    }
}
