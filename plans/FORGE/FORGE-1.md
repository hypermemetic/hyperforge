# FORGE-1: Flatten `origin + mirrors` to `forges: Vec<Forge>`

## Goal

Remove the false hierarchy of "primary forge" (`origin`) vs "read-only copies" (`mirrors`). All forges are equal peers in a distributed forge network. Hyperforge should observe what exists on each forge and reconcile, not prescribe a primary.

The per-repo config (`.hyperforge/config.toml`) already uses a flat `forges: Vec<String>`. The `Repo` struct and `repos.yaml` should match.

## Motivation

Discovered while working on the rename refactor: `workspace_diff` labels repos on remotes but not in LocalForge as "delete", when they're just "remote_only" — LocalForge doesn't know about them yet. The origin/mirrors split also forces awkward API surfaces (`repos_create` takes `origin` + `mirrors` separately) and prevents natural merge semantics on import.

## Dependency DAG

```
  FORGE-2 (Repo struct)     FORGE-3 (OrgDefaults)
    │                           │
  ┌─┼───────┐                   │
  ▼ ▼       ▼                   │
FORGE-4  FORGE-5                │
(adapters) (sync)               │
  │         │                   │
  └────┬────┘───────────────────┘
       ▼
    FORGE-6 (hub.rs)
       │
       ▼
    FORGE-7 (verify)
```

## Tickets

| Ticket | Summary | Blocked By | Unlocks |
|--------|---------|------------|---------|
| FORGE-2 | Repo struct: replace `origin + mirrors` with `forges` | — | FORGE-4, FORGE-5 |
| FORGE-3 | Add `OrgDefaults` struct | — | FORGE-6 |
| FORGE-4 | Update adapter `to_repo()` methods | FORGE-2 | FORGE-6 |
| FORGE-5 | Update `symmetric_sync.rs`: `SyncOp::RemoteOnly`, filter | FORGE-2 | FORGE-6 |
| FORGE-6 | Update `hub.rs`: event variant, methods, `get_org_defaults()` | FORGE-2, FORGE-3, FORGE-4, FORGE-5 | FORGE-7 |
| FORGE-7 | Verify: `cargo check`, `cargo test`, manual smoke tests | FORGE-6 | — |

## Phase Breakdown

**Phase 1** (parallel): FORGE-2 + FORGE-3 — struct changes, no method logic
**Phase 2** (parallel): FORGE-4 + FORGE-5 — adapter and sync fixes
**Phase 3**: FORGE-6 — hub method updates (the big one)
**Phase 4**: FORGE-7 — verification

## Files Touched

- `src/types/repo.rs` — FORGE-2
- `src/types/mod.rs` — FORGE-3
- `src/adapters/github.rs` — FORGE-4
- `src/adapters/codeberg.rs` — FORGE-4
- `src/adapters/gitlab.rs` — FORGE-4
- `src/adapters/local_forge.rs` — FORGE-4
- `src/services/symmetric_sync.rs` — FORGE-5
- `src/hub.rs` — FORGE-6
