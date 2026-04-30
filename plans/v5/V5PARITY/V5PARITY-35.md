---
id: V5PARITY-35
title: "SET-FORGES — typed RPC for scoping repos to specific forges"
status: Complete
type: implementation
blocked_by: [V5PARITY-34]
unlocks: []
---

## Problem

V5PARITY-34 made per-repo `forges` authoritative. Setting it requires either editing `.hyperforge/config.toml` by hand and running `sync_config --mode pull`, or editing the org yaml directly. For bulk operations ("scope these 17 archived repos to no forges") users reach for shell loops that iterate workspace members and call `sync_config` per repo. That loop is the missing RPC.

## Required behavior

**`repos.set_forges --org X --name N --forges <list>` (single repo):**

- `--forges` accepts a comma-separated list of provider names (`github`, `codeberg`, `gitlab`) or the literal `none` (= empty list, scoped to no forges) or `unset` (= remove the field, legacy unscoped behavior).
- Updates the org yaml's `OrgRepo.forges`.
- If a local checkout for the repo can be located (via any workspace's `path/<name>/`), also writes through to `<checkout>/.hyperforge/config.toml`.
- Idempotent. Re-running with the same value emits `forges_set { changed: false }`.

**`workspaces.set_forges --name W --forges <list> [--filter <glob>] [--dry_run bool]` (workspace-level):**

- Iterates every member (filtered if `--filter` given), calls the same logic as the single-repo version per member.
- Per-member event: `forges_set { ref, forges, changed }`.
- Aggregate: `workspace_set_forges_summary { name, total, ok, errored, unchanged }`.
- `--dry_run true` emits the events with `dry_run: true` markers and writes nothing.

**Special `--forges` values** (closed enum at the wire boundary):

| Value | Meaning |
|---|---|
| `github,codeberg` etc. | scope to listed providers |
| `none` | empty list — scoped to NO forges |
| `unset` | remove the field; legacy unscoped behavior |

Anything else → `validation` error.

## What must NOT change

- V5PARITY-34's routing semantics — this ticket only supplies a typed setter.
- D9 / D13 / D6.
- `repos.sync_config` stays — it's still the way to propagate user-edited files. `set_forges` is the inverse direction (RPC drives, file follows).

## Acceptance criteria

1. `repos.set_forges --org demo --name widget --forges codeberg` updates `orgs/demo.yaml`'s `widget.forges` to `[codeberg]` AND writes through to `.hyperforge/config.toml` if a checkout exists.
2. `repos.set_forges … --forges none` sets `forges: []` (scoped to no forges).
3. `repos.set_forges … --forges unset` removes the field (back to unscoped).
4. `workspaces.set_forges --name W --forges none --filter "axon,cannabus-*"` scopes only filter-matching members; emits per-member event + aggregate.
5. `--dry_run true` emits all events but writes nothing on disk.
6. Re-running with the same value emits `forges_set { changed: false }` (idempotent).
7. Invalid provider name → `validation` error before any write.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-35.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
