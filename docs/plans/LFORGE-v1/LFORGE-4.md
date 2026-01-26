# LFORGE-4: Symmetric SyncService

**blocked_by:** [LFORGE-2]
**unlocks:** [LFORGE-6, LFORGE-7]

## Scope

Create a new `SyncService` that performs symmetric sync between any two `ForgePort` implementations. This replaces the current asymmetric sync that only pushes from config to remotes. With symmetric sync: `sync(github, local)` imports, `sync(local, github)` pushes, `sync(github, codeberg)` mirrors.

## Deliverables

1. New `src/services/symmetric_sync.rs` with `SymmetricSyncService`
2. `sync()` method signature: `sync(source: &dyn ForgePort, target: &dyn ForgePort, org, options)`
3. `SyncOptions` struct with `dry_run`, `delete_missing`, `repo_filter`
4. `SyncReport` struct with results per repo
5. Integration with diff computation from LFORGE-3
6. Unit tests using two `LocalForge` instances

## Verification Steps

```bash
# Run symmetric sync tests
cd ~/dev/controlflow/hypermemetic/hyperforge
cargo test symmetric_sync

# Run integration test with two LocalForges
cargo test test_sync_between_local_forges
```

## Implementation Notes

### SyncOptions and SyncReport

```rust
//! Symmetric sync service types.

use std::collections::HashSet;

use crate::domain::{RepoIdentity, SyncAction};

/// Options for sync operation
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// If true, compute actions but don't apply them
    pub dry_run: bool,
    /// If true, delete repos in target that don't exist in source
    pub delete_missing: bool,
    /// If set, only sync repos matching these names
    pub repo_filter: Option<HashSet<String>>,
}

impl SyncOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    pub fn delete_missing(mut self) -> Self {
        self.delete_missing = true;
        self
    }

    pub fn filter_repos(mut self, repos: HashSet<String>) -> Self {
        self.repo_filter = Some(repos);
        self
    }
}

/// Result of a single repo sync
#[derive(Debug, Clone)]
pub struct RepoSyncResult {
    pub identity: RepoIdentity,
    pub action: SyncAction,
    pub outcome: SyncOutcome,
}

/// Outcome of applying a sync action
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Action was applied successfully
    Applied,
    /// Action was skipped (dry run)
    Skipped,
    /// Action failed with error
    Failed { error: String },
    /// No action was needed (in sync)
    NoOp,
}

impl SyncOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, SyncOutcome::Applied | SyncOutcome::Skipped | SyncOutcome::NoOp)
    }
}

/// Summary report of sync operation
#[derive(Debug, Clone)]
pub struct SyncReport {
    pub org: String,
    pub source_forge: String,
    pub target_forge: String,
    pub results: Vec<RepoSyncResult>,
    pub dry_run: bool,
}

impl SyncReport {
    pub fn created_count(&self) -> usize {
        self.results.iter()
            .filter(|r| r.action.is_create() && matches!(r.outcome, SyncOutcome::Applied))
            .count()
    }

    pub fn updated_count(&self) -> usize {
        self.results.iter()
            .filter(|r| r.action.is_update() && matches!(r.outcome, SyncOutcome::Applied))
            .count()
    }

    pub fn deleted_count(&self) -> usize {
        self.results.iter()
            .filter(|r| r.action.is_delete() && matches!(r.outcome, SyncOutcome::Applied))
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.results.iter()
            .filter(|r| matches!(r.outcome, SyncOutcome::Failed { .. }))
            .count()
    }

    pub fn has_failures(&self) -> bool {
        self.failed_count() > 0
    }
}
```

### SymmetricSyncService

Create `src/services/symmetric_sync.rs`:

```rust
//! Symmetric sync service - sync repos between any two forges.

use crate::domain::{Repo, SyncAction, compute_sync_actions};
use crate::ports::{ForgePort, ForgeError};

use super::{SyncOptions, SyncReport, RepoSyncResult, SyncOutcome};

/// Service for syncing repositories between any two forges.
///
/// This enables symmetric operations:
/// - `sync(github, local)` = import from GitHub
/// - `sync(local, github)` = push to GitHub
/// - `sync(github, codeberg)` = mirror GitHub to Codeberg
pub struct SymmetricSyncService;

impl SymmetricSyncService {
    /// Sync repositories from source forge to target forge.
    ///
    /// Computes the diff between source and target, then applies
    /// create/update/delete operations to make target match source.
    ///
    /// # Arguments
    /// * `source` - The source of truth forge
    /// * `target` - The forge to update
    /// * `org` - Organization name to sync
    /// * `options` - Sync options (dry_run, delete_missing, etc.)
    pub async fn sync(
        source: &dyn ForgePort,
        target: &dyn ForgePort,
        org: &str,
        options: SyncOptions,
    ) -> Result<SyncReport, SyncError> {
        // 1. List repos from both forges
        let source_observed = source.list_repos(org).await?;
        let target_observed = target.list_repos(org).await?;

        // 2. Convert to unified Repo type
        let source_repos: Vec<Repo> = source_observed.into_iter().map(Repo::from).collect();
        let target_repos: Vec<Repo> = target_observed.into_iter().map(Repo::from).collect();

        // 3. Apply filter if specified
        let source_repos = if let Some(filter) = &options.repo_filter {
            source_repos.into_iter()
                .filter(|r| filter.contains(&r.identity.name))
                .collect()
        } else {
            source_repos
        };

        // 4. Compute sync actions
        let actions = compute_sync_actions(&source_repos, &target_repos, options.delete_missing);

        // 5. Apply actions (or skip if dry run)
        let mut results = Vec::new();
        for action in actions {
            let outcome = if options.dry_run {
                if action.needs_action() {
                    SyncOutcome::Skipped
                } else {
                    SyncOutcome::NoOp
                }
            } else {
                Self::apply_action(target, &action).await
            };

            results.push(RepoSyncResult {
                identity: action.identity().clone(),
                action,
                outcome,
            });
        }

        Ok(SyncReport {
            org: org.to_string(),
            source_forge: format!("{:?}", source.forge_type()),
            target_forge: format!("{:?}", target.forge_type()),
            results,
            dry_run: options.dry_run,
        })
    }

    /// Apply a single sync action to the target forge
    async fn apply_action(target: &dyn ForgePort, action: &SyncAction) -> SyncOutcome {
        match action {
            SyncAction::Create(repo) => {
                // Convert Repo to DesiredRepo for ForgePort API
                // TODO: Update ForgePort to accept Repo directly in LFORGE-6
                let desired = Self::repo_to_desired(repo);
                match target.create_repo(&desired).await {
                    Ok(_) => SyncOutcome::Applied,
                    Err(e) => SyncOutcome::Failed { error: e.to_string() },
                }
            }
            SyncAction::Update { repo, .. } => {
                let desired = Self::repo_to_desired(repo);
                match target.update_repo(&desired).await {
                    Ok(_) => SyncOutcome::Applied,
                    Err(e) => SyncOutcome::Failed { error: e.to_string() },
                }
            }
            SyncAction::Delete(identity) => {
                match target.delete_repo(identity).await {
                    Ok(_) => SyncOutcome::Applied,
                    Err(e) => SyncOutcome::Failed { error: e.to_string() },
                }
            }
            SyncAction::InSync(_) => SyncOutcome::NoOp,
        }
    }

    /// Convert unified Repo to legacy DesiredRepo for ForgePort compatibility
    fn repo_to_desired(repo: &Repo) -> crate::domain::DesiredRepo {
        use std::collections::HashSet;
        crate::domain::DesiredRepo {
            identity: repo.identity.clone(),
            description: repo.description.clone(),
            visibility: repo.visibility.clone(),
            forges: HashSet::new(), // Not used by ForgePort methods
            protected: false,
            marked_for_deletion: false,
        }
    }
}

/// Errors from sync operations
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Source forge error: {0}")]
    SourceError(#[from] ForgeError),

    #[error("Target forge error: {0}")]
    TargetError(String),
}
```

### Test with Two LocalForges

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::LocalForge;
    use crate::domain::RepoIdentity;
    use crate::types::Visibility;
    use std::collections::HashSet;

    fn make_desired(org: &str, name: &str) -> crate::domain::DesiredRepo {
        let mut forges = HashSet::new();
        forges.insert(crate::types::Forge::GitHub);
        crate::domain::DesiredRepo::new(
            RepoIdentity::new(org, name),
            Visibility::Public,
            forges,
        )
    }

    #[tokio::test]
    async fn test_sync_creates_missing_repos() {
        // Source has repos, target is empty
        let source = LocalForge::with_repos(vec![
            make_desired("org", "repo1"),
            make_desired("org", "repo2"),
        ]);
        let target = LocalForge::new();

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new(),
        ).await.unwrap();

        assert_eq!(report.created_count(), 2);
        assert_eq!(report.failed_count(), 0);

        // Verify target now has the repos
        let target_repos = target.list_repos("org").await.unwrap();
        assert_eq!(target_repos.len(), 2);
    }

    #[tokio::test]
    async fn test_sync_updates_changed_repos() {
        // Source has private repo, target has same repo as public
        let source = LocalForge::with_repos(vec![
            crate::domain::DesiredRepo::new(
                RepoIdentity::new("org", "repo1"),
                Visibility::Private,
                HashSet::new(),
            ),
        ]);
        let target = LocalForge::with_repos(vec![
            make_desired("org", "repo1"), // Public
        ]);

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new(),
        ).await.unwrap();

        assert_eq!(report.updated_count(), 1);
    }

    #[tokio::test]
    async fn test_sync_deletes_with_flag() {
        // Source is empty, target has repos
        let source = LocalForge::new();
        let target = LocalForge::with_repos(vec![
            make_desired("org", "orphan"),
        ]);

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new().delete_missing(),
        ).await.unwrap();

        assert_eq!(report.deleted_count(), 1);

        // Verify target is now empty
        let target_repos = target.list_repos("org").await.unwrap();
        assert!(target_repos.is_empty());
    }

    #[tokio::test]
    async fn test_sync_no_delete_without_flag() {
        let source = LocalForge::new();
        let target = LocalForge::with_repos(vec![
            make_desired("org", "orphan"),
        ]);

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new(), // No delete_missing
        ).await.unwrap();

        assert_eq!(report.deleted_count(), 0);

        // Target still has the repo
        let target_repos = target.list_repos("org").await.unwrap();
        assert_eq!(target_repos.len(), 1);
    }

    #[tokio::test]
    async fn test_sync_dry_run_no_changes() {
        let source = LocalForge::with_repos(vec![
            make_desired("org", "repo1"),
        ]);
        let target = LocalForge::new();

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new().dry_run(),
        ).await.unwrap();

        assert!(report.dry_run);
        // Check action was computed but skipped
        assert_eq!(report.results.len(), 1);
        assert!(matches!(report.results[0].outcome, SyncOutcome::Skipped));

        // Target should still be empty
        let target_repos = target.list_repos("org").await.unwrap();
        assert!(target_repos.is_empty());
    }

    #[tokio::test]
    async fn test_sync_filter_repos() {
        let source = LocalForge::with_repos(vec![
            make_desired("org", "repo1"),
            make_desired("org", "repo2"),
            make_desired("org", "repo3"),
        ]);
        let target = LocalForge::new();

        let mut filter = HashSet::new();
        filter.insert("repo1".to_string());
        filter.insert("repo3".to_string());

        let report = SymmetricSyncService::sync(
            &source,
            &target,
            "org",
            SyncOptions::new().filter_repos(filter),
        ).await.unwrap();

        assert_eq!(report.created_count(), 2);

        // Only repo1 and repo3 should be in target
        let target_repos = target.list_repos("org").await.unwrap();
        assert_eq!(target_repos.len(), 2);
    }

    #[tokio::test]
    async fn test_bidirectional_sync() {
        // Test import then export pattern
        let github = LocalForge::with_repos(vec![
            make_desired("org", "from-github"),
        ]);
        let local = LocalForge::new();

        // Import: github -> local
        SymmetricSyncService::sync(&github, &local, "org", SyncOptions::new())
            .await.unwrap();

        // Create new repo locally
        local.create_repo(&make_desired("org", "new-local")).await.unwrap();

        // Export: local -> github
        SymmetricSyncService::sync(&local, &github, "org", SyncOptions::new())
            .await.unwrap();

        // Both should have both repos
        assert_eq!(github.list_repos("org").await.unwrap().len(), 2);
        assert_eq!(local.list_repos("org").await.unwrap().len(), 2);
    }
}
```

### Key Design Decisions

1. **Pure function approach**: `sync()` is stateless, takes forges as parameters
2. **No special-casing**: Source and target are just ForgePort, doesn't matter which is local
3. **Explicit delete control**: Must opt-in to destructive deletes
4. **Report-based feedback**: Full visibility into what was done
5. **Filter support**: Can sync subset of repos
6. **Legacy compatibility**: Converts Repo to DesiredRepo for current ForgePort API
