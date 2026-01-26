# LFORGE-7: Simplify Tests

**blocked_by:** [LFORGE-4, LFORGE-6]
**unlocks:** [LFORGE-8]

## Scope

Refactor all service and integration tests to use two `LocalForge` instances instead of mock implementations. Remove the `MockForgePort` and other test-specific forge mocks. This dramatically simplifies testing because LocalForge is a real, working implementation.

## Deliverables

1. Update all `services/` tests to use LocalForge pairs
2. Update all `activations/` tests to use LocalForge
3. Remove `MockForgePort` from `services/sync.rs`
4. Remove `mock_forge_client.rs` adapter
5. Add test utilities module for common LocalForge test patterns
6. Maintain or increase test coverage

## Verification Steps

```bash
# Run all tests
cd ~/dev/controlflow/hypermemetic/hyperforge
cargo test

# Verify no mock implementations remain
grep -r "MockForge" src/
# Should return nothing

# Check test coverage
cargo tarpaulin --out Html
# Open tarpaulin-report.html
```

## Implementation Notes

### Test Utilities Module

Create `src/test_utils.rs`:

```rust
//! Test utilities for LocalForge-based testing.
//!
//! These helpers make it easy to set up test scenarios with
//! pre-populated LocalForge instances.

#![cfg(test)]

use crate::adapters::LocalForge;
use crate::domain::{DesiredRepo, RepoIdentity};
use crate::types::{Forge, Visibility};
use std::collections::HashSet;

/// Builder for creating test repos
pub struct TestRepoBuilder {
    org: String,
    name: String,
    visibility: Visibility,
    description: Option<String>,
}

impl TestRepoBuilder {
    pub fn new(org: &str, name: &str) -> Self {
        Self {
            org: org.to_string(),
            name: name.to_string(),
            visibility: Visibility::Public,
            description: None,
        }
    }

    pub fn private(mut self) -> Self {
        self.visibility = Visibility::Private;
        self
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    pub fn build(self) -> DesiredRepo {
        let mut repo = DesiredRepo::new(
            RepoIdentity::new(&self.org, &self.name),
            self.visibility,
            HashSet::new(),
        );
        if let Some(desc) = self.description {
            repo = repo.with_description(desc);
        }
        repo
    }
}

/// Shorthand for creating a public test repo
pub fn repo(org: &str, name: &str) -> DesiredRepo {
    TestRepoBuilder::new(org, name).build()
}

/// Shorthand for creating a private test repo
pub fn private_repo(org: &str, name: &str) -> DesiredRepo {
    TestRepoBuilder::new(org, name).private().build()
}

/// Create a LocalForge with the given repos
pub fn forge_with(repos: Vec<DesiredRepo>) -> LocalForge {
    LocalForge::with_repos(repos)
}

/// Create two LocalForges: source with repos, target empty
pub fn source_target_pair(source_repos: Vec<DesiredRepo>) -> (LocalForge, LocalForge) {
    (LocalForge::with_repos(source_repos), LocalForge::new())
}

/// Assert a forge contains exactly these repo names for an org
pub async fn assert_repos(forge: &LocalForge, org: &str, expected_names: &[&str]) {
    let repos = forge.list_repos(org).await.unwrap();
    let actual_names: HashSet<_> = repos.iter().map(|r| r.identity.name.as_str()).collect();
    let expected: HashSet<_> = expected_names.iter().copied().collect();
    assert_eq!(actual_names, expected, "Repos mismatch for org {}", org);
}

/// Assert a forge has no repos for an org
pub async fn assert_empty(forge: &LocalForge, org: &str) {
    let repos = forge.list_repos(org).await.unwrap();
    assert!(repos.is_empty(), "Expected no repos for org {}, found {:?}", org, repos);
}
```

### Updated Sync Service Tests

Rewrite `src/services/sync.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::LocalForge;
    use crate::test_utils::*;

    // =========================================================================
    // SymmetricSyncService tests using LocalForge pairs
    // =========================================================================

    #[tokio::test]
    async fn test_sync_creates_missing_repos() {
        let (source, target) = source_target_pair(vec![
            repo("org", "repo1"),
            repo("org", "repo2"),
        ]);

        let report = SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new()
        ).await.unwrap();

        assert_eq!(report.created_count(), 2);
        assert_repos(&target, "org", &["repo1", "repo2"]).await;
    }

    #[tokio::test]
    async fn test_sync_updates_changed_visibility() {
        let source = forge_with(vec![private_repo("org", "repo1")]);
        let target = forge_with(vec![repo("org", "repo1")]); // Public

        let report = SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new()
        ).await.unwrap();

        assert_eq!(report.updated_count(), 1);
    }

    #[tokio::test]
    async fn test_sync_deletes_when_flag_set() {
        let source = LocalForge::new();
        let target = forge_with(vec![repo("org", "orphan")]);

        let report = SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new().delete_missing()
        ).await.unwrap();

        assert_eq!(report.deleted_count(), 1);
        assert_empty(&target, "org").await;
    }

    #[tokio::test]
    async fn test_sync_preserves_extra_repos_without_delete_flag() {
        let source = LocalForge::new();
        let target = forge_with(vec![repo("org", "keep-me")]);

        SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new()
        ).await.unwrap();

        // Target should still have the repo
        assert_repos(&target, "org", &["keep-me"]).await;
    }

    #[tokio::test]
    async fn test_sync_dry_run_makes_no_changes() {
        let (source, target) = source_target_pair(vec![repo("org", "new-repo")]);

        let report = SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new().dry_run()
        ).await.unwrap();

        assert!(report.dry_run);
        assert_eq!(report.results.len(), 1);
        assert!(matches!(report.results[0].outcome, SyncOutcome::Skipped));
        assert_empty(&target, "org").await;
    }

    #[tokio::test]
    async fn test_sync_with_filter() {
        let source = forge_with(vec![
            repo("org", "included"),
            repo("org", "excluded"),
        ]);
        let target = LocalForge::new();

        let mut filter = HashSet::new();
        filter.insert("included".to_string());

        SymmetricSyncService::sync(
            &source, &target, "org",
            SyncOptions::new().filter_repos(filter)
        ).await.unwrap();

        assert_repos(&target, "org", &["included"]).await;
    }

    #[tokio::test]
    async fn test_sync_in_sync_repos_noop() {
        let repos = vec![repo("org", "same")];
        let source = forge_with(repos.clone());
        let target = forge_with(repos);

        let report = SymmetricSyncService::sync(
            &source, &target, "org", SyncOptions::new()
        ).await.unwrap();

        assert_eq!(report.created_count(), 0);
        assert_eq!(report.updated_count(), 0);
        assert!(report.results.iter().all(|r| r.action.is_in_sync()));
    }

    #[tokio::test]
    async fn test_bidirectional_sync_scenario() {
        // Simulate real workflow: import from "github", modify locally, sync to "codeberg"
        let github = forge_with(vec![repo("org", "from-github")]);
        let local = LocalForge::new();
        let codeberg = LocalForge::new();

        // Import: github -> local
        SymmetricSyncService::sync(&github, &local, "org", SyncOptions::new()).await.unwrap();

        // Create local repo
        local.create_repo(&repo("org", "local-only")).await.unwrap();

        // Sync: local -> codeberg
        SymmetricSyncService::sync(&local, &codeberg, "org", SyncOptions::new()).await.unwrap();

        // Codeberg should have both
        assert_repos(&codeberg, "org", &["from-github", "local-only"]).await;
    }

    #[tokio::test]
    async fn test_multiple_orgs_independent() {
        let source = forge_with(vec![
            repo("org1", "repo1"),
            repo("org2", "repo2"),
        ]);
        let target = LocalForge::new();

        // Sync only org1
        SymmetricSyncService::sync(&source, &target, "org1", SyncOptions::new()).await.unwrap();

        assert_repos(&target, "org1", &["repo1"]).await;
        assert_empty(&target, "org2").await;
    }
}
```

### Updated Activation Tests

```rust
// In src/activations/org/activation.rs tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;

    #[tokio::test]
    async fn test_import_from_remote() {
        let remote = forge_with(vec![
            repo("myorg", "remote-repo"),
        ]);
        let local = LocalForge::new();

        let activation = OrgActivation::new();
        let report = activation.import(
            &remote, &local, "myorg", ImportOptions::default()
        ).await.unwrap();

        assert_eq!(report.created_count(), 1);
        assert_repos(&local, "myorg", &["remote-repo"]).await;
    }

    #[tokio::test]
    async fn test_import_dry_run() {
        let remote = forge_with(vec![repo("myorg", "repo")]);
        let local = LocalForge::new();

        let options = ImportOptions { dry_run: true, ..Default::default() };
        let report = OrgActivation::new()
            .import(&remote, &local, "myorg", options)
            .await.unwrap();

        assert_empty(&local, "myorg").await; // No changes
    }
}

// In src/activations/repos/activation.rs tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;

    #[tokio::test]
    async fn test_sync_to_remote() {
        let local = forge_with(vec![repo("myorg", "local-repo")]);
        let remote = LocalForge::new();

        let activation = ReposActivation::new();
        let report = activation.sync(
            &local, &remote, "myorg", SyncRepoOptions::default()
        ).await.unwrap();

        assert_eq!(report.created_count(), 1);
        assert_repos(&remote, "myorg", &["local-repo"]).await;
    }
}
```

### Files to Remove

After migrating all tests:

```bash
# Remove mock implementations
rm src/bridge/mock_forge_client.rs

# Update src/bridge/mod.rs to remove:
# mod mock_forge_client;
# pub use mock_forge_client::MockForgeClient;
```

### Key Design Decisions

1. **LocalForge IS the test infrastructure**: No mocks needed
2. **Test utilities module**: Consistent helpers across all tests
3. **Async test assertions**: Helper functions for common checks
4. **No behavioral differences**: LocalForge behaves exactly like real forges
5. **Simpler test setup**: Create, populate, and assert in a few lines
6. **Maintainability**: One implementation to maintain, not N mocks
