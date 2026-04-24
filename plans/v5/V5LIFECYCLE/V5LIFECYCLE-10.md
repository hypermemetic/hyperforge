---
id: V5LIFECYCLE-10
title: "workspaces.{reconcile,sync} consult .hyperforge/config.toml"
status: Complete
type: implementation
blocked_by: [V5LIFECYCLE-6, V5LIFECYCLE-9]
unlocks: [V5LIFECYCLE-11]
---

## Problem

A local dir that carries `.hyperforge/config.toml` has self-described identity. v4 uses this during workspace sync. v5 doesn't read the file today. Per D14, both `workspaces.reconcile` and `workspaces.sync` must consult it.

Also: `workspaces.sync` currently tries to read remote metadata for every member. For `dismissed` members this is wasted work (the repo was soft-deleted intentionally). Per R2 this ticket decides: **skip dismissed by default**, include via new flag.

## Required behavior

### reconcile additions

During the filesystem scan in `workspaces.reconcile` (V5WS-8), for each non-git-ignored dir:

1. Call `ops::fs::read_hyperforge_config(&dir)`.
2. If `Some(cfg)` AND the matched member's `<org>/<name>` differs from `cfg.org + "/" + cfg.repo_name` → emit a `config_drift` event:
   ```
   { kind: "config_drift", dir, declared_ref: <cfg>, workspace_ref: <member> }
   ```
   Org yaml still wins for deciding the match; this event is informational.
3. No mutation of the `.hyperforge/config.toml` file — reconcile is read-only.

### sync additions

Before entering the per-member sync loop:

- For each member, check `repo.metadata.lifecycle`:
  - `active` → normal sync (existing V5WS-9/V5PROV-8 flow)
  - `dismissed` → **skip by default**. Emit `sync_skipped { ref, reason: "dismissed" }`, count under a new `skipped: u32` field on `WorkspaceSyncReport`.

New method param:

| Input | Type | Required | Notes |
|---|---|---|---|
| `include_dismissed` | `bool` | no (default false) | when true, dismissed members are synced (informational only — adapter reads still happen) |

Invariant: `total == in_sync + drifted + errored + created + skipped`.

## What must NOT change

- Reconcile's existing 5-kind event set (matched, renamed, removed, new_matched, ambiguous) — this ticket ADDS a sixth (`config_drift`), doesn't replace.
- Sync's existing behavior for `active` members.
- D13 — all filesystem reads go through `ops::fs::*`.

## Acceptance criteria

1. Reconcile with a local dir that has `.hyperforge/config.toml` matching the workspace member: `config_drift` NOT emitted.
2. Reconcile with a local dir whose `.hyperforge/config.toml` declares a different `<org>/<name>` than the workspace member matched via git origin: `config_drift` emitted with the detected discrepancy; workspace yaml unchanged (org yaml wins).
3. `workspaces.sync` on a workspace with one `active` + one `dismissed` member, no `include_dismissed`: per-repo events = 1 `sync_diff` (active) + 1 `sync_skipped` (dismissed). Report: `total: 2, skipped: 1, in_sync|drifted|errored|created: 1`.
4. `workspaces.sync name=X include_dismissed=true`: the dismissed member is synced (produces a sync_diff like any active member), `skipped: 0`.
5. A dir without `.hyperforge/config.toml` behaves exactly as V5WS-8 / V5PROV-8 did before this ticket.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-10.sh` → exit 0 (tier 2).
- Status flips in-commit.
