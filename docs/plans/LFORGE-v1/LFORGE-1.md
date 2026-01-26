# LFORGE-1: Local Forge - Symmetric Sync Architecture

## Goal

Treat "local" as an in-memory forge, making all sync operations symmetric:
- Import = `sync(github, local)`
- Push = `sync(local, github)`
- Mirror = `sync(github, codeberg)`

## Why

Current architecture has asymmetric concepts:
- `DesiredState` (what user wants) vs `ObservedState` (what exists remotely)
- `SyncedState` (last known remote state)
- `StoragePort` for config, separate from `ForgePort` for remotes
- Different code paths for import vs sync

With LocalForge:
- One concept: repos exist in forges
- One operation: sync between any two forges
- Testing is trivial: two LocalForge instances

## Dependency DAG

```
              LFORGE-2 (LocalForge impl)
                      │
          ┌───────────┼───────────┐
          ▼           ▼           ▼
     LFORGE-3    LFORGE-4    LFORGE-5
     (simplify   (symmetric  (persistence)
      domain)     sync svc)
          │           │           │
          └─────┬─────┴───────────┘
                ▼
           LFORGE-6 (update activations)
                │
                ▼
           LFORGE-7 (simplify tests)
                │
                ▼
           LFORGE-8 (HUMAN GATE)
                │
                ▼
           LFORGE-9 (cleanup old code)
```

## What Changes

| Component | Current | After |
|-----------|---------|-------|
| `domain/desired.rs` | DesiredRepo | **Remove** - use Repo |
| `domain/observed.rs` | ObservedRepo, ForgeRepoState | **Simplify** - Repo + which forges |
| `domain/diff.rs` | RepoDiff between desired/observed | Diff between two forge states |
| `services/import.rs` | Special import logic | **Remove** - use sync |
| `services/sync.rs` | Push to remotes | `sync(source, target)` |
| `services/diff.rs` | Compare desired vs synced | Compare any two forges |
| `StoragePort` | Load/save YAML config | **Remove** - LocalForge handles persistence |
| `adapters/yaml_storage.rs` | YAML adapter | Merge into LocalForge |
| `adapters/memory_storage.rs` | Test adapter | **Remove** - use LocalForge |

## New Core Types

```rust
/// A repository as it exists in any forge
pub struct Repo {
    pub identity: RepoIdentity,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub homepage: Option<String>,
}

/// What to do to sync source → target
pub enum SyncAction {
    Create(Repo),           // Exists in source, not in target
    Update(Repo, Vec<Diff>), // Exists in both, different properties
    Delete(RepoIdentity),   // Exists in target, not in source (optional)
    InSync(RepoIdentity),   // Same in both
}

/// Symmetric sync between any two forges
pub struct SyncService;

impl SyncService {
    pub async fn sync(
        source: &dyn ForgePort,
        target: &dyn ForgePort,
        org: &str,
        options: SyncOptions,
    ) -> Result<SyncReport, SyncError>;
}
```

## Tickets

| Ticket | Description | Blocked By |
|--------|-------------|------------|
| LFORGE-2 | Create LocalForge implementing ForgePort | - |
| LFORGE-3 | Simplify domain types (Repo, SyncAction) | LFORGE-2 |
| LFORGE-4 | Symmetric SyncService | LFORGE-2, LFORGE-3 |
| LFORGE-5 | LocalForge persistence (YAML backend) | LFORGE-2 |
| LFORGE-6 | Update activations to use symmetric sync | LFORGE-4 |
| LFORGE-7 | Simplify tests with LocalForge | LFORGE-4 |
| LFORGE-8 | Human verification gate | LFORGE-6, LFORGE-7 |
| LFORGE-9 | Remove old code (StoragePort, ImportService) | LFORGE-8 |

## Success Criteria

1. `LocalForge` passes all ForgePort tests
2. `sync(local, github)` and `sync(github, local)` both work
3. Integration test: create in local → sync to mock → verify
4. Removed: DesiredState, ObservedState, SyncedState, StoragePort, ImportService
5. Test count maintained or increased
