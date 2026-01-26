# LFORGE-8: Human Verification Gate

**blocked_by:** [LFORGE-6, LFORGE-7]
**unlocks:** [LFORGE-9]

## Scope

This is a manual verification checkpoint. Before removing old code (LFORGE-9), a human must verify that the new LocalForge-based architecture works correctly end-to-end. This ticket provides a comprehensive checklist of verifications to perform.

## Deliverables

1. All checklist items verified and checked off
2. Any issues found documented and fixed
3. Sign-off from reviewer

## Verification Checklist

### Unit Test Verification

```bash
cd ~/dev/controlflow/hypermemetic/hyperforge

# All tests pass
cargo test

# No warnings
cargo test 2>&1 | grep -i warning
# Should be empty or only expected warnings

# Coverage check (should be >= current coverage)
cargo tarpaulin --out Html
open tarpaulin-report.html
```

- [ ] All unit tests pass
- [ ] No unexpected warnings
- [ ] Test coverage maintained or improved

### LocalForge Verification

```bash
# In Rust, run these tests interactively or add as integration tests:
```

```rust
#[tokio::test]
async fn verify_localforge_crud() {
    use crate::adapters::LocalForge;
    use crate::domain::{DesiredRepo, RepoIdentity};
    use crate::ports::ForgePort;
    use crate::types::Visibility;
    use std::collections::HashSet;

    let forge = LocalForge::new();

    // Create
    let repo = DesiredRepo::new(
        RepoIdentity::new("testorg", "testrepo"),
        Visibility::Public,
        HashSet::new(),
    );
    forge.create_repo(&repo).await.unwrap();

    // Read (list)
    let repos = forge.list_repos("testorg").await.unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].identity.name, "testrepo");

    // Update
    let updated = DesiredRepo::new(
        RepoIdentity::new("testorg", "testrepo"),
        Visibility::Private,
        HashSet::new(),
    );
    forge.update_repo(&updated).await.unwrap();

    // Delete
    forge.delete_repo(&RepoIdentity::new("testorg", "testrepo")).await.unwrap();
    assert!(forge.list_repos("testorg").await.unwrap().is_empty());

    println!("LocalForge CRUD: PASS");
}
```

- [ ] LocalForge create works
- [ ] LocalForge list works
- [ ] LocalForge update works
- [ ] LocalForge delete works
- [ ] LocalForge repo_exists works

### Persistence Verification

```rust
#[tokio::test]
async fn verify_localforge_persistence() {
    use crate::adapters::LocalForge;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repos.yaml");

    // Create and save
    let forge1 = LocalForge::new();
    forge1.create_repo(&make_repo("org", "repo1")).await.unwrap();
    forge1.save(&path, "org").unwrap();

    // Load into new instance
    let forge2 = LocalForge::load(&path).unwrap();
    let repos = forge2.list_repos("org").await.unwrap();
    assert_eq!(repos.len(), 1);

    println!("LocalForge persistence: PASS");
}
```

- [ ] save() creates valid YAML file
- [ ] load() reads existing YAML file
- [ ] Roundtrip preserves all data
- [ ] Auto-save works on mutations

### Symmetric Sync Verification

```rust
#[tokio::test]
async fn verify_symmetric_sync() {
    use crate::adapters::LocalForge;
    use crate::services::{SymmetricSyncService, SyncOptions};

    // Test: sync(A, B) then sync(B, A) results in identical forges

    let forge_a = LocalForge::with_repos(vec![
        make_repo("org", "repo1"),
        make_repo("org", "repo2"),
    ]);
    let forge_b = LocalForge::new();

    // A -> B
    SymmetricSyncService::sync(&forge_a, &forge_b, "org", SyncOptions::new())
        .await.unwrap();

    // Add repo to B
    forge_b.create_repo(&make_repo("org", "repo3")).await.unwrap();

    // B -> A
    SymmetricSyncService::sync(&forge_b, &forge_a, "org", SyncOptions::new())
        .await.unwrap();

    // Both should have all 3 repos
    assert_eq!(forge_a.list_repos("org").await.unwrap().len(), 3);
    assert_eq!(forge_b.list_repos("org").await.unwrap().len(), 3);

    println!("Symmetric sync: PASS");
}
```

- [ ] sync(source, target) creates missing repos
- [ ] sync(source, target) updates changed repos
- [ ] sync with delete_missing removes extra repos
- [ ] sync without delete_missing preserves extra repos
- [ ] dry_run computes but doesn't apply changes
- [ ] Bidirectional sync works correctly

### Import/Export Flow Verification

```rust
#[tokio::test]
async fn verify_import_export_flow() {
    use crate::adapters::LocalForge;
    use crate::services::{SymmetricSyncService, SyncOptions};

    // Simulate: GitHub -> Local -> Codeberg

    let github = LocalForge::with_repos(vec![
        make_repo("myorg", "project-a"),
        make_repo("myorg", "project-b"),
    ]);
    let local = LocalForge::new();
    let codeberg = LocalForge::new();

    // Import from GitHub
    let import_report = SymmetricSyncService::sync(
        &github, &local, "myorg", SyncOptions::new()
    ).await.unwrap();
    assert_eq!(import_report.created_count(), 2);

    // User creates new local repo
    local.create_repo(&make_repo("myorg", "local-project")).await.unwrap();

    // Sync to Codeberg
    let sync_report = SymmetricSyncService::sync(
        &local, &codeberg, "myorg", SyncOptions::new()
    ).await.unwrap();
    assert_eq!(sync_report.created_count(), 3);

    // Verify all repos present
    assert_eq!(codeberg.list_repos("myorg").await.unwrap().len(), 3);

    println!("Import/Export flow: PASS");
}
```

- [ ] Import (remote -> local) works
- [ ] Export (local -> remote) works
- [ ] Full import -> modify -> export flow works

### YAML Format Compatibility

```bash
# Check existing repos.yaml can be loaded
cat ~/.config/hyperforge/orgs/hypermemetic/repos.yaml
```

```rust
#[test]
fn verify_yaml_compatibility() {
    // This YAML is the existing format
    let yaml = r#"
owner: hypermemetic
repos:
  hyperforge:
    description: "Multi-forge repo manager"
    visibility: public
    forges:
      - github
      - codeberg
    protected: true
  .dotfiles:
    visibility: private
    forges:
      - github
"#;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), yaml).unwrap();

    let forge = LocalForge::load(tmp.path()).unwrap();
    let repos = forge.get_org_repos("hypermemetic");

    assert_eq!(repos.len(), 2);
    println!("YAML compatibility: PASS");
}
```

- [ ] Existing repos.yaml files load correctly
- [ ] Saved YAML is readable by other tools
- [ ] Description, visibility, protected flags preserved

### CLI Command Verification (if applicable)

```bash
# If CLI is updated, test these commands:

# Import should work
synapse plexus hyperforge org import --org_name test-org --dry_run true

# Diff should show changes
synapse plexus hyperforge org test-org repos diff

# Sync should apply changes (with --yes)
synapse plexus hyperforge org test-org repos sync --yes true --dry_run true
```

- [ ] Import command works (if updated)
- [ ] Diff command shows correct changes
- [ ] Sync command applies changes correctly

### Edge Cases

```rust
#[tokio::test]
async fn verify_edge_cases() {
    let forge = LocalForge::new();

    // Empty org returns empty vec, not error
    let repos = forge.list_repos("nonexistent").await.unwrap();
    assert!(repos.is_empty());

    // Create duplicate fails
    forge.create_repo(&make_repo("org", "repo")).await.unwrap();
    let result = forge.create_repo(&make_repo("org", "repo")).await;
    assert!(result.is_err());

    // Update nonexistent fails
    let result = forge.update_repo(&make_repo("org", "nope")).await;
    assert!(result.is_err());

    // Delete nonexistent fails
    let result = forge.delete_repo(&RepoIdentity::new("org", "nope")).await;
    assert!(result.is_err());

    println!("Edge cases: PASS");
}
```

- [ ] Empty org returns empty list (not error)
- [ ] Duplicate create fails with RepoAlreadyExists
- [ ] Update nonexistent fails with RepoNotFound
- [ ] Delete nonexistent fails with RepoNotFound

### Documentation Check

- [ ] Code has appropriate doc comments
- [ ] Complex functions have examples
- [ ] Public API is documented

## Sign-Off

Once all items are verified:

```
Verified by: _______________
Date: _______________
Notes: _______________
```

## Issues Found

Document any issues discovered during verification:

| Issue | Severity | Fix Required Before LFORGE-9? |
|-------|----------|------------------------------|
| (none yet) | | |

## Notes

- If any critical issues are found, create follow-up tickets
- Non-critical issues can be addressed in LFORGE-9 or later
- This gate ensures we don't remove working code prematurely
