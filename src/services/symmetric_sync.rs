//! SymmetricSyncService - Bidirectional forge synchronization
//!
//! This service implements symmetric sync between any two ForgePort implementations:
//! - sync(local, github): Push local state to GitHub
//! - sync(github, local): Import GitHub repos to local state
//! - sync(local, codeberg): Mirror repos to Codeberg
//!
//! Origin-based logic:
//! - Each repo has one origin forge (source of truth)
//! - Repos are synced to origin first, then mirrored to other forges

use std::sync::Arc;

use crate::adapters::{ForgePort, ForgeResult};
use crate::types::Repo;

/// Sync operation type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Create repo on target (doesn't exist)
    Create,
    /// Update repo on target (exists but differs)
    Update,
    /// Delete repo on target (exists but marked for deletion)
    Delete,
    /// No action needed (in sync)
    InSync,
}

/// Repo with its sync operation
#[derive(Debug, Clone)]
pub struct RepoOp {
    pub repo: Repo,
    pub op: SyncOp,
}

/// Diff between source and target forges
#[derive(Debug, Clone)]
pub struct SyncDiff {
    /// Organization being synced
    pub org: String,
    /// Operations to perform
    pub ops: Vec<RepoOp>,
}

impl SyncDiff {
    /// Get repos that need to be created
    pub fn to_create(&self) -> Vec<&Repo> {
        self.ops
            .iter()
            .filter(|op| op.op == SyncOp::Create)
            .map(|op| &op.repo)
            .collect()
    }

    /// Get repos that need to be updated
    pub fn to_update(&self) -> Vec<&Repo> {
        self.ops
            .iter()
            .filter(|op| op.op == SyncOp::Update)
            .map(|op| &op.repo)
            .collect()
    }

    /// Get repos that need to be deleted
    pub fn to_delete(&self) -> Vec<&Repo> {
        self.ops
            .iter()
            .filter(|op| op.op == SyncOp::Delete)
            .map(|op| &op.repo)
            .collect()
    }

    /// Get repos that are already in sync
    pub fn in_sync(&self) -> Vec<&Repo> {
        self.ops
            .iter()
            .filter(|op| op.op == SyncOp::InSync)
            .map(|op| &op.repo)
            .collect()
    }

    /// Check if any changes are needed
    pub fn has_changes(&self) -> bool {
        self.ops.iter().any(|op| op.op != SyncOp::InSync)
    }
}

/// Service for symmetric forge synchronization
pub struct SymmetricSyncService;

impl SymmetricSyncService {
    /// Create a new sync service
    pub fn new() -> Self {
        Self
    }

    /// Compute diff between source and target forges
    ///
    /// # Arguments
    /// * `source` - Source forge to read from
    /// * `target` - Target forge to compare against
    /// * `org` - Organization name
    ///
    /// # Returns
    /// SyncDiff containing operations needed to make target match source
    pub async fn diff(
        &self,
        source: Arc<dyn ForgePort>,
        target: Arc<dyn ForgePort>,
        org: &str,
    ) -> ForgeResult<SyncDiff> {
        // Get repos from both forges
        let source_repos = source.list_repos(org).await?;
        let target_repos = target.list_repos(org).await?;

        // Build map for quick lookup
        let mut target_map: std::collections::HashMap<String, Repo> = target_repos
            .into_iter()
            .map(|r| (r.name.clone(), r))
            .collect();

        let mut ops = Vec::new();

        // Check each source repo
        for source_repo in source_repos {
            // Staged for deletion: delete from target if present, otherwise skip
            if source_repo.staged_for_deletion {
                if target_map.remove(&source_repo.name).is_some() {
                    ops.push(RepoOp {
                        repo: source_repo,
                        op: SyncOp::Delete,
                    });
                }
                continue;
            }

            if let Some(target_repo) = target_map.remove(&source_repo.name) {
                // Repo exists on both - check if update needed
                if repos_differ(&source_repo, &target_repo) {
                    ops.push(RepoOp {
                        repo: source_repo,
                        op: SyncOp::Update,
                    });
                } else {
                    ops.push(RepoOp {
                        repo: source_repo,
                        op: SyncOp::InSync,
                    });
                }
            } else {
                // Repo only in source - needs creation on target
                ops.push(RepoOp {
                    repo: source_repo,
                    op: SyncOp::Create,
                });
            }
        }

        // Remaining target repos not in source - mark for deletion
        for (_, target_repo) in target_map {
            ops.push(RepoOp {
                repo: target_repo,
                op: SyncOp::Delete,
            });
        }

        Ok(SyncDiff {
            org: org.to_string(),
            ops,
        })
    }

    /// Execute sync operations to make target match source
    ///
    /// # Arguments
    /// * `source` - Source forge to read from
    /// * `target` - Target forge to write to
    /// * `org` - Organization name
    /// * `dry_run` - If true, don't actually execute operations
    ///
    /// # Returns
    /// SyncDiff showing what was/would be done
    pub async fn sync(
        &self,
        source: Arc<dyn ForgePort>,
        target: Arc<dyn ForgePort>,
        org: &str,
        dry_run: bool,
    ) -> ForgeResult<SyncDiff> {
        let diff = self.diff(source.clone(), target.clone(), org).await?;

        if dry_run {
            return Ok(diff);
        }

        // Execute operations
        for op in &diff.ops {
            match op.op {
                SyncOp::Create => {
                    target.create_repo(org, &op.repo).await?;
                }
                SyncOp::Update => {
                    target.update_repo(org, &op.repo).await?;
                }
                SyncOp::Delete => {
                    target.delete_repo(org, &op.repo.name).await?;
                }
                SyncOp::InSync => {
                    // No action needed
                }
            }
        }

        Ok(diff)
    }

    /// Sync repos with origin-first logic
    ///
    /// For each forge, syncs only the repos that belong on that forge:
    /// - Repos where origin == forge
    /// - Repos where mirrors includes forge
    ///
    /// This respects the origin/mirror configuration and prevents syncing
    /// all repos to all forges.
    pub async fn sync_with_origins(
        &self,
        source: Arc<dyn ForgePort>,
        forges: std::collections::HashMap<String, Arc<dyn ForgePort>>,
        org: &str,
        dry_run: bool,
    ) -> ForgeResult<Vec<SyncDiff>> {
        use crate::adapters::LocalForge;
        use crate::types::Forge;

        let all_repos = source.list_repos(org).await?;
        let mut diffs = Vec::new();

        // For each forge, filter repos that belong on it
        for (forge_name, forge_adapter) in forges {
            let forge_type = match forge_name.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => continue, // Skip unknown forges
            };

            // Filter repos that should be on this forge (exclude staged for deletion)
            let repos_for_forge: Vec<_> = all_repos
                .iter()
                .filter(|r| {
                    !r.staged_for_deletion
                        && (r.origin == forge_type || r.mirrors.contains(&forge_type))
                })
                .cloned()
                .collect();

            if repos_for_forge.is_empty() {
                continue; // No repos for this forge
            }

            // Create temporary LocalForge with only these repos
            let filtered_local = Arc::new(LocalForge::new(org));
            for repo in repos_for_forge {
                if let Err(e) = filtered_local.create_repo(org, &repo).await {
                    // Log error but continue with other repos
                    eprintln!("Failed to add repo {} to filtered forge: {}", repo.name, e);
                }
            }

            // Sync filtered repos to this forge
            let diff = self
                .sync(filtered_local, forge_adapter, org, dry_run)
                .await?;
            diffs.push(diff);
        }

        Ok(diffs)
    }
}

impl Default for SymmetricSyncService {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if two repos differ in meaningful ways
fn repos_differ(a: &Repo, b: &Repo) -> bool {
    a.description != b.description || a.visibility != b.visibility
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::LocalForge;
    use crate::types::{Forge, Visibility};

    #[tokio::test]
    async fn test_diff_empty_forges() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.ops.len(), 0);
        assert!(!diff.has_changes());
    }

    #[tokio::test]
    async fn test_diff_create_needed() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add repo to source
        let repo = Repo::new("new-repo", Forge::GitHub);
        source.create_repo("testorg", &repo).await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.to_create().len(), 1);
        assert_eq!(diff.to_create()[0].name, "new-repo");
        assert!(diff.has_changes());
    }

    #[tokio::test]
    async fn test_diff_update_needed() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add repo to both with different descriptions
        let repo_source = Repo::new("test-repo", Forge::GitHub)
            .with_description("New description");
        let repo_target = Repo::new("test-repo", Forge::GitHub)
            .with_description("Old description");

        source.create_repo("testorg", &repo_source).await.unwrap();
        target.create_repo("testorg", &repo_target).await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.to_update().len(), 1);
        assert_eq!(diff.to_update()[0].name, "test-repo");
        assert!(diff.has_changes());
    }

    #[tokio::test]
    async fn test_diff_delete_needed() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add repo only to target
        let repo = Repo::new("old-repo", Forge::GitHub);
        target.create_repo("testorg", &repo).await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.to_delete().len(), 1);
        assert_eq!(diff.to_delete()[0].name, "old-repo");
        assert!(diff.has_changes());
    }

    #[tokio::test]
    async fn test_diff_in_sync() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add identical repo to both
        let repo = Repo::new("synced-repo", Forge::GitHub)
            .with_description("Same description");

        source.create_repo("testorg", &repo).await.unwrap();
        target.create_repo("testorg", &repo).await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.in_sync().len(), 1);
        assert_eq!(diff.in_sync()[0].name, "synced-repo");
        assert!(!diff.has_changes());
    }

    #[tokio::test]
    async fn test_sync_creates_repos() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add repo to source
        let repo = Repo::new("new-repo", Forge::GitHub);
        source.create_repo("testorg", &repo).await.unwrap();

        // Sync (not dry run)
        let diff = service
            .sync(source.clone(), target.clone(), "testorg", false)
            .await
            .unwrap();

        assert_eq!(diff.to_create().len(), 1);

        // Verify repo was created on target
        let target_repo = target.get_repo("testorg", "new-repo").await.unwrap();
        assert_eq!(target_repo.name, "new-repo");
    }

    #[tokio::test]
    async fn test_sync_dry_run() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Add repo to source
        let repo = Repo::new("new-repo", Forge::GitHub);
        source.create_repo("testorg", &repo).await.unwrap();

        // Sync with dry_run=true
        let diff = service
            .sync(source.clone(), target.clone(), "testorg", true)
            .await
            .unwrap();

        assert_eq!(diff.to_create().len(), 1);

        // Verify repo was NOT created on target
        let result = target.get_repo("testorg", "new-repo").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_repos_differ_description() {
        let repo1 = Repo::new("test", Forge::GitHub).with_description("Desc 1");
        let repo2 = Repo::new("test", Forge::GitHub).with_description("Desc 2");
        assert!(repos_differ(&repo1, &repo2));
    }

    #[tokio::test]
    async fn test_repos_differ_visibility() {
        let repo1 = Repo::new("test", Forge::GitHub).with_visibility(Visibility::Public);
        let repo2 = Repo::new("test", Forge::GitHub).with_visibility(Visibility::Private);
        assert!(repos_differ(&repo1, &repo2));
    }

    #[tokio::test]
    async fn test_repos_same() {
        let repo1 = Repo::new("test", Forge::GitHub).with_description("Same");
        let repo2 = Repo::new("test", Forge::GitHub).with_description("Same");
        assert!(!repos_differ(&repo1, &repo2));
    }

    #[tokio::test]
    async fn test_diff_staged_for_deletion_on_target() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Create then soft-delete on source (sets dismissed → staged_for_deletion)
        let repo = Repo::new("dying-repo", Forge::GitHub);
        source.create_repo("testorg", &repo).await.unwrap();
        source.delete_repo("testorg", "dying-repo").await.unwrap();

        // Also exists on target
        let target_repo = Repo::new("dying-repo", Forge::GitHub);
        target.create_repo("testorg", &target_repo).await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        assert_eq!(diff.to_delete().len(), 1);
        assert_eq!(diff.to_delete()[0].name, "dying-repo");
        assert_eq!(diff.to_create().len(), 0);
    }

    #[tokio::test]
    async fn test_diff_staged_for_deletion_not_on_target() {
        let service = SymmetricSyncService::new();
        let source = Arc::new(LocalForge::new("testorg"));
        let target = Arc::new(LocalForge::new("testorg"));

        // Create then soft-delete on source, not on target
        let repo = Repo::new("already-gone", Forge::GitHub);
        source.create_repo("testorg", &repo).await.unwrap();
        source.delete_repo("testorg", "already-gone").await.unwrap();

        let diff = service.diff(source, target, "testorg").await.unwrap();
        // Should be completely absent from diff — nothing to do
        assert_eq!(diff.ops.len(), 0);
    }

    #[tokio::test]
    async fn test_sync_with_origins_filters_by_forge() {
        let service = SymmetricSyncService::new();

        // Create source with 3 repos:
        // - repo1: origin=github, no mirrors
        // - repo2: origin=codeberg, no mirrors
        // - repo3: origin=github, mirrors=[codeberg]
        let source = Arc::new(LocalForge::new("testorg"));

        let repo1 = Repo::new("github-only", Forge::GitHub);
        let repo2 = Repo::new("codeberg-only", Forge::Codeberg);
        let repo3 = Repo::new("github-mirrored", Forge::GitHub)
            .with_mirror(Forge::Codeberg);

        source.create_repo("testorg", &repo1).await.unwrap();
        source.create_repo("testorg", &repo2).await.unwrap();
        source.create_repo("testorg", &repo3).await.unwrap();

        // Create target forges
        let github_target = Arc::new(LocalForge::new("testorg"));
        let codeberg_target = Arc::new(LocalForge::new("testorg"));

        let mut forges: std::collections::HashMap<String, Arc<dyn ForgePort>> = std::collections::HashMap::new();
        forges.insert("github".to_string(), github_target.clone());
        forges.insert("codeberg".to_string(), codeberg_target.clone());

        // Sync with origins
        let diffs = service
            .sync_with_origins(source, forges, "testorg", false)
            .await
            .unwrap();

        // Should have 2 diffs (one per forge)
        assert_eq!(diffs.len(), 2);

        // GitHub should have repo1 and repo3
        let github_repos = github_target.list_repos("testorg").await.unwrap();
        assert_eq!(github_repos.len(), 2);
        let github_names: Vec<_> = github_repos.iter().map(|r| r.name.as_str()).collect();
        assert!(github_names.contains(&"github-only"));
        assert!(github_names.contains(&"github-mirrored"));

        // Codeberg should have repo2 and repo3
        let codeberg_repos = codeberg_target.list_repos("testorg").await.unwrap();
        assert_eq!(codeberg_repos.len(), 2);
        let codeberg_names: Vec<_> = codeberg_repos.iter().map(|r| r.name.as_str()).collect();
        assert!(codeberg_names.contains(&"codeberg-only"));
        assert!(codeberg_names.contains(&"github-mirrored"));
    }

    #[tokio::test]
    async fn test_sync_with_origins_respects_mirrors() {
        let service = SymmetricSyncService::new();

        // Repo with origin=github, mirrors=[codeberg, gitlab]
        let source = Arc::new(LocalForge::new("testorg"));
        let repo = Repo::new("multi-mirror", Forge::GitHub)
            .with_mirrors(vec![Forge::Codeberg, Forge::GitLab]);
        source.create_repo("testorg", &repo).await.unwrap();

        let github_target = Arc::new(LocalForge::new("testorg"));
        let codeberg_target = Arc::new(LocalForge::new("testorg"));
        let gitlab_target = Arc::new(LocalForge::new("testorg"));

        let mut forges: std::collections::HashMap<String, Arc<dyn ForgePort>> = std::collections::HashMap::new();
        forges.insert("github".to_string(), github_target.clone());
        forges.insert("codeberg".to_string(), codeberg_target.clone());
        forges.insert("gitlab".to_string(), gitlab_target.clone());

        service
            .sync_with_origins(source, forges, "testorg", false)
            .await
            .unwrap();

        // Repo should exist on all three forges
        assert_eq!(github_target.list_repos("testorg").await.unwrap().len(), 1);
        assert_eq!(codeberg_target.list_repos("testorg").await.unwrap().len(), 1);
        assert_eq!(gitlab_target.list_repos("testorg").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_sync_with_origins_no_cross_contamination() {
        let service = SymmetricSyncService::new();

        // Two repos with different origins, no mirroring
        let source = Arc::new(LocalForge::new("testorg"));
        let repo1 = Repo::new("github-repo", Forge::GitHub);
        let repo2 = Repo::new("codeberg-repo", Forge::Codeberg);

        source.create_repo("testorg", &repo1).await.unwrap();
        source.create_repo("testorg", &repo2).await.unwrap();

        let github_target = Arc::new(LocalForge::new("testorg"));
        let codeberg_target = Arc::new(LocalForge::new("testorg"));

        let mut forges: std::collections::HashMap<String, Arc<dyn ForgePort>> = std::collections::HashMap::new();
        forges.insert("github".to_string(), github_target.clone());
        forges.insert("codeberg".to_string(), codeberg_target.clone());

        service
            .sync_with_origins(source, forges, "testorg", false)
            .await
            .unwrap();

        // GitHub should ONLY have github-repo
        let github_repos = github_target.list_repos("testorg").await.unwrap();
        assert_eq!(github_repos.len(), 1);
        assert_eq!(github_repos[0].name, "github-repo");

        // Codeberg should ONLY have codeberg-repo
        let codeberg_repos = codeberg_target.list_repos("testorg").await.unwrap();
        assert_eq!(codeberg_repos.len(), 1);
        assert_eq!(codeberg_repos[0].name, "codeberg-repo");
    }
}
