# Synced State Unification

**Status**: Accepted
**Date**: 2026-01-16
**Issue**: Dual storage locations for synced state causing diff/sync failures

## Context

The hyperforge sync system tracks which repositories have been synced to which forges. This "synced state" is used by the diff operation to compare desired state against what's actually on the forges.

Currently, there are **two incompatible storage locations** for synced state:

1. **Inline `_synced` field** in `repos.yaml` - where new syncs write
2. **Separate `synced.yaml` file** - where historical data lives

This causes the diff operation to show false positives ("to_create" for repos that exist).

## Current Architecture

### File Locations

```
~/.config/hyperforge/orgs/{org}/
├── repos.yaml          # Desired state + inline _synced (new format)
├── synced.yaml         # Legacy synced state (old format)
└── staged-repos.yaml   # Staged changes
```

### repos.yaml Schema (with inline _synced)

```yaml
owner: hypermemetic
repos:
  my-repo:
    description: "My repository"
    visibility: public
    forges: [github, codeberg]
    protected: false
    _delete: false
    _synced:                          # Inline synced state
      github:
        url: "https://github.com/hypermemetic/my-repo"
        id: "12345"
        synced_at: "2026-01-15T10:30:00Z"
      codeberg:
        url: "https://codeberg.org/hypermemetic/my-repo"
        id: null
        synced_at: "2026-01-15T10:30:00Z"
```

### synced.yaml Schema (legacy)

```yaml
owner: hypermemetic
repos:
  my-repo:
    forge_states:
      - forge: github
        exists: true
        url: "https://github.com/hypermemetic/my-repo.git"
        forge_id: null
        visibility: public
        description: "My repository"
      - forge: codeberg
        exists: true
        url: "https://codeberg.org/hypermemetic/my-repo.git"
        forge_id: null
        visibility: public
        description: "My repository"
```

### Current Data Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│ WRITE PATH (workspace sync)                                         │
│                                                                     │
│ SyncOutcome::Applied                                                │
│     │                                                               │
│     └──► storage.update_synced(repo, forge, url, id)                │
│              │                                                      │
│              └──► repos.yaml._synced[repo][forge] = { url, id, ts } │
│                                                                     │
│ Problem: Only writes to _synced, ignores synced.yaml                │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ READ PATH (workspace repos diff)                                    │
│                                                                     │
│ OrgStorageAdapter.load_synced()                                     │
│     │                                                               │
│     ├──► Check repos.yaml for _synced fields                        │
│     │        │                                                      │
│     │        └──► If ANY repo has _synced: return those only        │
│     │                                                               │
│     └──► Fallback: If ALL _synced empty, read synced.yaml           │
│                                                                     │
│ Problem: After first sync via new path, loses synced.yaml data      │
└─────────────────────────────────────────────────────────────────────┘
```

### The Bug

1. Historical synced data exists in `synced.yaml` (e.g., 13 repos)
2. User runs `workspace sync` - updates 2 repos
3. `update_synced()` writes to `_synced` in `repos.yaml` for those 2 repos
4. Next `diff` operation:
   - Sees non-empty `_synced` in repos.yaml (2 repos)
   - Skips `synced.yaml` fallback
   - Returns only 2 repos as "synced"
   - Other 11 repos show as "to_create" (false positive)

## Proposed Solution: Unified Storage with Migration

### Design Principles

1. **Single source of truth** - One location for synced state
2. **Inline with desired state** - Keep `_synced` in `repos.yaml` alongside config
3. **Migration path** - Automatic migration from legacy format
4. **No data loss** - Preserve all historical synced state

### Target Architecture

```
~/.config/hyperforge/orgs/{org}/
├── repos.yaml          # Desired state + synced state (unified)
├── synced.yaml.bak     # Backup of migrated data (optional)
└── staged-repos.yaml   # Staged changes
```

### Implementation

#### Phase 1: Merge on Read (Immediate Fix)

Update `OrgStorageAdapter.load_synced()` to merge both sources:

```rust
async fn load_synced(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError> {
    let mut repos_by_name: HashMap<String, ObservedRepo> = HashMap::new();

    // 1. Load from synced.yaml first (baseline)
    if let Ok(legacy_repos) = self.load_synced_file(org).await {
        for repo in legacy_repos {
            repos_by_name.insert(repo.identity.name.clone(), repo);
        }
    }

    // 2. Overlay _synced from repos.yaml (takes precedence)
    let repos_config = self.storage.load_repos().await?;
    for (name, config) in &repos_config.repos {
        if let Some(synced) = &config.synced {
            let identity = RepoIdentity::new(org, name);
            let mut observed = ObservedRepo::new(identity);

            for (forge, state) in &synced.forges {
                observed = observed.with_forge_state(ForgeRepoState::found(
                    forge.clone(),
                    state.url.clone(),
                    config.visibility.clone().unwrap_or(Visibility::Public),
                    state.id.clone(),
                    config.description.clone(),
                ));
            }

            // Override any legacy data for this repo
            repos_by_name.insert(name.clone(), observed);
        }
    }

    Ok(repos_by_name.into_values().collect())
}
```

#### Phase 2: Migration Command

Add `hyperforge migrate-synced-state` command:

```rust
pub async fn migrate_synced_state(&self, org: &str) -> Result<MigrationResult> {
    // 1. Load legacy synced.yaml
    let legacy_path = self.paths.org_dir(org).join("synced.yaml");
    if !legacy_path.exists() {
        return Ok(MigrationResult::NothingToMigrate);
    }

    let legacy_synced = self.load_synced_file(org).await?;

    // 2. Load current repos.yaml
    let mut repos_config = self.storage.load_repos().await?;

    // 3. Merge legacy data into _synced fields
    let mut migrated_count = 0;
    for observed in legacy_synced {
        let repo_name = &observed.identity.name;

        if let Some(config) = repos_config.repos.get_mut(repo_name) {
            let synced = config.synced.get_or_insert_with(SyncedState::default);

            for forge_state in &observed.forge_states {
                if forge_state.exists {
                    synced.forges.entry(forge_state.forge.clone())
                        .or_insert_with(|| ForgeSyncedState {
                            url: forge_state.url.clone().unwrap_or_default(),
                            id: forge_state.forge_id.clone(),
                            synced_at: chrono::Utc::now(),
                        });
                }
            }
            migrated_count += 1;
        }
    }

    // 4. Save updated repos.yaml
    self.storage.save_repos(&repos_config).await?;

    // 5. Backup and remove legacy file
    let backup_path = legacy_path.with_extension("yaml.bak");
    tokio::fs::rename(&legacy_path, &backup_path).await?;

    Ok(MigrationResult::Migrated {
        repos: migrated_count,
        backup_path,
    })
}
```

#### Phase 3: Remove Legacy Support

After migration is deployed and users have migrated:
1. Remove `synced.yaml` fallback from `load_synced()`
2. Remove migration command
3. Simplify to single-source reads

### File Changes

| File | Change |
|------|--------|
| `src/adapters/org_storage.rs` | Merge both sources in `load_synced()` |
| `src/storage/org_storage.rs` | Add `migrate_synced_state()` method |
| `src/activations/org/activation.rs` | Add `migrate` command |
| `src/activations/workspace/service.rs` | Already fixed: call `update_synced` for updates |

### Testing Strategy

1. **Unit tests**: Mock both storage formats, verify merge logic
2. **Integration tests**: Create repos.yaml + synced.yaml, verify diff accuracy
3. **Migration tests**: Verify data preservation and backup creation

## Alternatives Considered

### Option B: Write to Both Locations

Keep both files, write to both on every sync.

**Pros**: No migration needed
**Cons**: Redundant data, drift risk, complexity

### Option C: Use Only synced.yaml

Move all synced state to separate file, remove `_synced` from repos.yaml.

**Pros**: Clear separation of concerns
**Cons**: Requires rewriting `update_synced()`, loses benefit of co-located data

## Decision

**Accepted: Option A (Unified with Migration)**

Implementation order:
1. **Phase 1 (immediate)**: Merge on read - fix `load_synced()` to combine both sources
2. **Phase 2 (follow-up)**: Migration command to consolidate data
3. **Phase 3 (cleanup)**: Remove legacy support after migration

Rationale:
- Single source of truth eliminates sync bugs
- Inline `_synced` keeps related data together
- Migration is one-time cost
- Backwards compatible during transition

## Implementation Status

### Phase 1: Merge on Read - COMPLETED (2026-01-16)

**Changes implemented:**

| File | Change |
|------|--------|
| `src/adapters/org_storage.rs:170-206` | `load_synced()` now merges both `synced.yaml` (baseline) and `_synced` from `repos.yaml` (takes precedence) |
| `src/adapters/org_storage.rs:36-94` | Added `load_synced_file()` helper to read legacy `synced.yaml` format |
| `src/storage/org_storage.rs` | Added `paths()` getter to expose `HyperforgePaths` for adapter |

**Key behavior:**
1. Load all repos from `synced.yaml` into HashMap (keyed by repo name)
2. Overlay any `_synced` data from `repos.yaml` (newer data takes precedence)
3. Return merged collection

### Testing Results

**Test Date:** 2026-01-16

#### Diff Command Verification

Before fix:
- `org hypermemetic repos diff` showed all 13 repos as `to_create` (false positives)
- Root cause: `load_synced()` was only reading `_synced` from repos.yaml, which was empty

After fix:
```
$ synapse plexus hyperforge org hypermemetic repos diff
in_sync: 2
to_create: 0      ← Fixed! No false positives
to_delete: 1
to_update: 6
untracked: 5
```

**Breakdown:**
- `in_sync`: 3 repos (hub-core, dockerfiles, claude-container)
- `to_update`: 6 repos (mostly description diffs on codeberg)
- `to_delete`: 1 repo (claude-container removed from github config)
- `untracked`: 5 repos (test repos not in local config)
- `to_create`: **0** - key fix verified

#### Storage State Verification

**`~/.config/hyperforge/orgs/hypermemetic/`:**
- `synced.yaml` (5KB): Contains 13 repos with forge_states from historical syncs
- `repos.yaml` (1.3KB): Contains 8 repos in desired state, no `_synced` fields yet

The merge-on-read correctly loads all 13 repos from `synced.yaml` as baseline synced state.

#### Sync Command Verification

The `update_repo()` implementation was also fixed during this session:
- `src/bridge/github.rs`: Added PATCH API implementation
- `src/bridge/codeberg.rs`: Added PATCH API implementation
- Verified: `hypermemetic-infra` visibility changed to private on GitHub

### Workspace Sync Status

| Workspace | Synced with Remote | Notes |
|-----------|-------------------|-------|
| hypermemetic | Partial | 6 repos need description updates on Codeberg |
| hypermemetic-infra | N/A | This is a repo, not a separate workspace |

**Current pending sync actions:**
- `hypermemetic-infra`: Update visibility on both forges (config says private, synced says public)
- `synapse`, `hub-macro`, `substrate`, `substrate-protocol`: Update description on Codeberg
- `claude-container`: Delete from GitHub (removed from forges list)

### Phase 2: Migration Command - PENDING

Not yet implemented. With Phase 1 complete, the system works correctly but legacy `synced.yaml` remains. Migration command will consolidate data into `repos.yaml._synced` fields.

### Phase 3: Remove Legacy Support - PENDING

Blocked by Phase 2 completion.

## References

- `src/adapters/org_storage.rs:170-206` - Updated `load_synced()` implementation
- `src/adapters/org_storage.rs:36-94` - New `load_synced_file()` helper
- `src/storage/org_storage.rs:157-176` - Current `update_synced()` implementation
- `src/activations/workspace/service.rs:604-617` - Sync state update calls
- `~/.config/hyperforge/orgs/hypermemetic/synced.yaml` - Example legacy file (13 repos)
