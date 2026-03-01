# CONF-6: Config-First Workspace Operations + WorkspaceRunner Adoption

**blocked_by**: [CONF-3, CONF-4]
**unlocks**: [CONF-7]

## Scope

Two goals in one ticket (they touch the same methods):

1. Rewire workspace methods to use the config-first flow (LocalForge → materialize)
2. Convert parallel workspace methods to use `WorkspaceRunner` from CONF-4

This is the largest ticket — it rewrites the bulk of `workspace.rs` (~3800 lines → ~2200-2500).

## Method-by-Method Changes

### Group A: Convert to WorkspaceRunner (concurrency rewrite)

These methods currently copy-paste JoinSet/chunking boilerplate. Convert to `run_batch` / `run_batch_blocking`:

| Method | Current Concurrency | Target |
|--------|-------------------|--------|
| **check** | spawn_blocking, chunk 8 | `run_batch_blocking(repos, 8, check_fn)` |
| **push_all** | spawn_blocking, chunk 8 | `run_batch_blocking(repos, 8, push_fn)` |
| **exec** | async Command, optional sequential | `run_batch(repos, Async(n) or Sequential, exec_fn)` |
| **clone** | async, configurable concurrency | `run_batch(repos, Async(concurrency), clone_fn)` |
| **set_default_branch** | async forge API, chunk 8 | `run_batch(tasks, Async(8), set_branch_fn)` |
| **sync phase 6** | async forge API, unbounded | `run_batch(pairs, Async(0), diff_fn)` |
| **sync phase 7** | async forge API, unbounded | `run_batch(pairs, Async(0), apply_fn)` |
| **sync phase 8** | spawn_blocking, chunk 8 | `run_batch_blocking(repos, 8, push_fn)` |

### Group B: Config-first rewrites

#### `workspace.init`

**Before**: Calls `init(path, opts)` which writes config+remotes directly, no LocalForge.

**After**:
1. Discover unconfigured repos
2. For each: build `RepoRecord` from inferred org/forges, register in LocalForge
3. Save LocalForge YAML
4. `materialize()` each repo

This merges the current init Phase 2 and sync Phase 4 into a single coherent step. After `workspace.init`, repos are both configured on disk AND registered in LocalForge.

#### `workspace.sync`

**Before**: 8-phase pipeline (discover → init → re-discover → register → import → diff → apply → push). Phases 2 and 4 are the bridge between filesystem and registry.

**After**: Simplified pipeline since init now registers in LocalForge:
1. **Discover** — filesystem scan
2. **Register** — upsert all discovered repos into LocalForge (with `merge_from_config`)
3. **Import** — incremental import from remote forges (ETag-based)
4. **Diff** — parallel diff per org/forge pair (via `run_batch`)
5. **Apply** — parallel create/update on remotes (via `run_batch`)
6. **Materialize** — project LocalForge state back to disk for any records with local_path
7. **Push** — parallel git push (via `run_batch_blocking`)

The re-discover phase disappears. The init phase merges into register. Materialize replaces the implicit config-writing that happened during init.

#### `workspace.move_repos`

**Before**: Four sequential steps (config → remotes → registry → directory), each can fail independently.

**After**:
1. Validate all repos exist in source, not in target
2. For each repo:
   a. Update `record.org` in source LocalForge → remove from source → upsert in target
   b. Update `record.local_path` to target directory
   c. Move directory on disk
   d. `materialize(target_org, &record, new_path, default_opts)` — rewrites config + reconciles remotes
3. Save both LocalForge YAMLs

Materialize handles config + remotes atomically from the updated record. No separate steps that can drift.

#### `workspace.clone`

**Before**: Clones from LocalForge, adds mirror remotes, no config.

**After**: Clone + `materialize()` per repo. Same as CONF-5's repo.clone but batched via `run_batch`.

### Group C: No changes needed (read-only or already correct)

| Method | Why no change |
|--------|--------------|
| discover | Read-only filesystem scan |
| diff | Read-only comparison |
| analyze | Read-only dep graph analysis |
| detect_name_mismatches | Read-only comparison |
| verify | Read-only config validation |
| unify | Writes workspace manifests, not per-repo config |
| validate | Runs containers, doesn't write config |
| package_diff | Read-only version comparison |
| publish | Publishes packages, not config concern |
| bump | Version bump, could materialize but low priority |

## Discovery Helper Adoption

Replace 11+ instances of:
```rust
let ctx = match discover_workspace(&workspace_path) {
    Ok(ctx) => ctx,
    Err(e) => { yield HyperforgeEvent::Error { message: ... }; return; }
};
```

With:
```rust
let ctx = match discover_or_bail(&workspace_path) {
    Ok(ctx) => ctx,
    Err(event) => { yield event; return; }
};
```

## Acceptance Criteria

- [ ] All 8 parallel methods use `run_batch` / `run_batch_blocking`
- [ ] `workspace.init` registers repos in LocalForge
- [ ] `workspace.sync` pipeline simplified to 7 phases (from 8)
- [ ] `workspace.move_repos` uses materialize instead of manual config+remote steps
- [ ] `workspace.clone` materializes after cloning
- [ ] Discovery helper used by all discovery-based methods
- [ ] No behavioral change from user perspective (same events, same outcomes)
- [ ] `workspace.rs` reduced by ~1000+ lines
- [ ] `cargo build --release` succeeds
- [ ] Existing tests pass

## Notes

- This is the most labor-intensive ticket. Consider splitting into CONF-6a (WorkspaceRunner adoption only, no config changes) and CONF-6b (config-first rewrites) if the scope proves too large.
- The `sync` method is ~1000 lines and the most complex. It may warrant its own sub-ticket.
- Event ordering may change slightly with `run_batch` (completion order vs. input order). This is acceptable — the current JoinSet behavior is already completion-ordered.
