---
id: V5WS-9
title: "workspaces.sync — orchestrate repos.sync over members, aggregate WorkspaceSyncReport"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9, V5REPOS-13]
unlocks: [V5WS-10]
---

## Problem

Users need a workspace-level command that runs `repos.sync` on every
member and returns an aggregate — how many members in sync, drifted,
errored — plus per-repo `SyncDiff` for drill-in. Per D6, one failing
member does not abort the batch.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing workspace |

| Output / Event | Shape | Notes |
|---|---|---|
| per member | one `SyncDiff` event per member | shape pinned by V5REPOS-13 |
| aggregate | one `WorkspaceSyncReport` at stream end | `name`, `total`, `in_sync`, `drifted`, `errored`, `per_repo: [SyncDiff]` |
| workspace not found | typed error event | names the `WorkspaceName`; no report event |

Per-member events emit in yaml order (deterministic). The aggregate
is the final non-terminator event.

Execution model pinned here (addresses R2): members are processed
**serially** in yaml order. No parallelism in v1 — keeps rate-limit
behavior predictable and per-member ordering stable. Concurrency
tuning is deferred to a future ticket.

Failure model pinned here (addresses R3, per D6): any per-member
failure surfaces as a `SyncDiff` with `status == "errored"`, counted
in `WorkspaceSyncReport.errored`; sync continues to the next member.
Overall RPC exit is success.

Edge cases: zero-member workspace → no `SyncDiff`; one report with
`total == 0` and `per_repo == []`. Dangling ref (org yaml absent) or
missing adapter creds → errored member; batch continues. Duplicate
entries → each synced independently; `total` counts entries.

## What must NOT change

- Workspace yaml is NOT modified by `sync` — ever. Reconcile is the only method that rewrites it at runtime.
- Org yamls are READ-only — V5REPOS-13 itself only reads `orgs/*.yaml`; `sync` introduces no new write path.
- The filesystem at the workspace's `path` is never touched — metadata sync reads/writes no git working trees.
- `delete_remote` is NOT a parameter — `sync` is read-only against every forge.

## Acceptance criteria

(Tier 2 — exercises real forge adapters through V5REPOS-13.)

1. Against `ws_with_one_repo` with valid creds, `workspaces.sync name=main` emits exactly one `SyncDiff` then one `WorkspaceSyncReport` with `name == "main"`, `total == 1`, `in_sync + drifted + errored == 1`, `per_repo` of length 1.
2. Against `ws_cross_org` with creds for both orgs, `SyncDiff` events appear in yaml order (`demo/widget` then `acme/tool`), followed by one report with `total == 2`.
3. With one org missing creds, `workspaces.sync name=multi` emits two `SyncDiff` events (the cred-less one `status == "errored"`), then a report with `errored == 1` and `total == 2`. RPC exits success.
4. Against a zero-member workspace, zero `SyncDiff` events and one report with `total == 0`, `in_sync == drifted == errored == 0`, `per_repo == []`.
5. `workspaces.sync name=nonexistent` emits a typed not-found; no report event.
6. Before/after every call, every file under `workspaces/`, `orgs/`, and the workspace's on-disk `path` is byte-identical.

## Completion

- Run `bash tests/v5/V5WS/V5WS-9.sh` → exit 0 (tier 2).
- Status flips in-commit with the implementation.
