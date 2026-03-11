# DRY Workspace Delegation + LocalForge Consistency

## Problem

Workspace methods duplicated repo-level logic instead of delegating. This caused three classes of bugs:

### 1. LocalForge state violations

Repo-level methods mutated remote forges without reflecting changes in `repos.yaml`:

| Method | Missing LocalForge update |
|--------|--------------------------|
| `repo set_default_branch` | Never wrote `record.default_branch` after setting on remotes |
| `repo purge` | Partial failures left stale `present_on` — deleted forges still listed |
| `repo delete` | Privatized on remotes but never set `record.visibility = Private` |

### 2. Code duplication

Workspace methods reimplemented the same operations with subtle differences:

| Workspace method | Duplicated from | Lines removed |
|-----------------|-----------------|---------------|
| `workspace set_default_branch` | `repo set_default_branch` | ~170 lines of forge iteration + adapter calls |
| `workspace clone` | `repo clone` | ~50 lines of clone + materialize |
| `workspace sync` Phase 7 | `repo sync` | ~140 lines (`sync_apply_diffs` helper) |
| `workspace init` | `repo init` | ~30 lines (but gained LocalForge registration) |

### 3. Asymmetric behavior

- `workspace clone` cloned + materialized but never set `local_path` on the LocalForge record
- `workspace init` wrote `.hyperforge/config.toml` but never registered repos in LocalForge
- `workspace sync` Phase 7 created/updated on remotes but never updated `present_on`

## Design Principle

**Repo-level methods are the authoritative atomic operations.** Workspace methods should be:

```
discover → filter → call repo method per repo in parallel → aggregate results
```

Both `RepoHub` and `WorkspaceHub` hold the same `HyperforgeState` (all `Arc`-wrapped, `Clone` is cheap). WorkspaceHub constructs a `RepoHub` trivially:

```rust
let repo_hub = RepoHub::new(state.clone());
```

## Implementation

### Phase 1: Fix repo-level LocalForge violations

Fixed the repo methods FIRST so that when workspace delegates to them, the fixes come for free.

**`repo set_default_branch`** (`src/hubs/repo.rs`):

After successfully calling `adapter.set_default_branch()` on all forges:
```rust
if errors.is_empty() {
    local.set_default_branch(&org, &name, &branch).await?;
    local.save_to_yaml().await?;
}
```

**`repo purge`** (`src/hubs/repo.rs`):

After each successful `adapter.delete_repo()`, immediately track progress:
```rust
for f in &deleted_forges {
    rec.present_on.remove(f);
    rec.deleted_from.push(f.clone());
}
local.update_record(&rec);
local.save_to_yaml().await;
```

This saves even on partial failure, so `present_on` always reflects reality.

**`repo delete`** (`src/hubs/repo.rs`):

After privatization, set the record's visibility:
```rust
if !privatized_forges.is_empty() {
    rec.visibility = Visibility::Private;
}
```

### Phase 2: DRY workspace methods via delegation

RepoHub methods return `impl Stream<Item = HyperforgeEvent> + Send + 'static`. Workspace collects each stream into a `Vec<HyperforgeEvent>` inside `run_batch`:

```rust
let repo_hub = RepoHub::new(state.clone());
let items: Vec<_> = repos.into_iter().map(|r| {
    let hub = Clone::clone(&repo_hub);  // Note: Clone::clone() not hub.clone()
    (hub, org, name)
}).collect();

let results = run_batch(items, 8, |(hub, org, name): (RepoHub, String, String)| async move {
    let stream = hub.method(org, name).await;
    tokio::pin!(stream);
    let events: Vec<HyperforgeEvent> = stream.collect().await;
    let has_error = events.iter().any(|e| matches!(e, HyperforgeEvent::Error { .. }));
    (name, events, has_error)
}).await;
```

**Important**: Must use `Clone::clone(&repo_hub)` because `#[plexus_macros::hub_methods]` generates an RPC trait with a `clone` method (the RPC endpoint), which shadows `Clone::clone()`. Similarly, `RepoHub::clone(&hub, ...)` for calling the RPC `clone` method (not `Clone`).

#### Delegated methods

**`workspace set_default_branch`** → `repo_hub.set_default_branch(org, name, branch, checkout, path)` per repo via `run_batch`. LocalForge update now happens automatically inside repo method.

**`workspace clone`** → `RepoHub::clone(&hub, org, name, path, forge)` per record via `run_batch`. `local_path` update + materialize now happens automatically.

**`workspace sync` Phase 7** → Two-part approach:
- Create/Update ops: delegated to `repo_hub.sync(org, name, dry_run)` — gets `present_on` updates for free
- Delete ops (privatization): kept inline — workspace-specific lifecycle logic with no repo-level equivalent

**`workspace init`** → `repo_hub.init(path, forges, org, ...)` per unconfigured repo. Now registers in LocalForge, where previously only wrote config.toml.

#### Not delegated (intentionally)

**`workspace push_all`** and **sync Phase 8 push**: Both already share the same `push::push()` free function via `run_batch_blocking`. No LocalForge violations. Delegation through RepoHub would add overhead (stream collection) without fixing any bugs.

### Phase 3: Dead code removal

Removed `sync_apply_diffs` helper (~140 lines) — fully replaced by repo sync delegation + inline privatization for Delete ops.

## Files Modified

| File | Change |
|------|--------|
| `src/hubs/repo.rs` | Phase 1: LocalForge updates in `set_default_branch`, `purge`, `delete` |
| `src/hubs/workspace.rs` | Phase 2: Delegated `set_default_branch`, `clone`, `sync` Phase 7, `init` to RepoHub |
| `src/hubs/workspace.rs` | Phase 3: Removed `sync_apply_diffs` |

Net result: -190 lines, 182/182 tests passing.

## Data Flow (after)

```
workspace method
  ├── discover + filter (workspace-specific)
  ├── validation gate (workspace-specific, if applicable)
  ├── run_batch(repos, |repo| repo_hub.method(...))
  │     └── repo method
  │           ├── forge API call
  │           ├── LocalForge update  ← previously missing
  │           └── save_to_yaml()     ← previously missing
  └── aggregate results + summary
```

## Verification

1. `cargo test --lib` — 182 tests pass
2. `cargo install --path .` + restart lforge
3. Dry-run workspace operations and spot-check `repos.yaml` for LocalForge consistency
