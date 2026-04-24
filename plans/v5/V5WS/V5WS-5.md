---
id: V5WS-5
title: "workspaces.delete — remove workspace yaml with dry_run and delete_remote cascade"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users need to remove a workspace, with preview (D7) and an opt-in
cascade that also deletes every member repo on its forge (README §4 —
forge-side deletion requires explicit `delete_remote: true`).

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing workspace |
| `delete_remote` | `bool` | no (default false) | when true, cascade per-member via the same forge-delete path used by `repos.remove delete_remote=true` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| workspace deleted | event identifying the `WorkspaceName` | e.g. `{type: "workspace_deleted", name}` |
| per-member cascade (when `delete_remote: true`) | one event per member | carries `ref: RepoRef` + status (`forge_deleted` / `forge_delete_failed`); batch continues past per-member failure |
| not found | typed error event | names the `WorkspaceName` |

Post-conditions. `dry_run: false`, `delete_remote: false`:
`workspaces/<name>.yaml` is gone; org yamls, `secrets.yaml`, and other
workspaces are byte-identical. `dry_run: false`, `delete_remote: true`:
same plus per-member forge deletion invoked; workspace yaml is removed
even if some cascade events fail. `dry_run: true` (either mode): every
event is emitted (including cascade events) but nothing is deleted and
no forge endpoint is contacted.

Edge cases: `name` not on disk → typed not-found, no filesystem change.
Workspace with zero members + `delete_remote: true` → no cascade events,
yaml removed. Cascade failure for a member (no adapter creds etc.) →
failure event emitted; yaml still removed on `dry_run: false`.

## What must NOT change

- v4's `workspace.*` namespace. v5 writes only `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yamls are READ-only — cascade mode does NOT rewrite `orgs/*.yaml`. The cascade is forge-side only.
- Default (`delete_remote: false`) NEVER contacts any forge.
- The filesystem tree at the workspace's `path` is never touched in either mode.

## Acceptance criteria

1. Against `ws_with_one_repo`, `workspaces.delete name=main` emits a deletion event; `workspaces/main.yaml` is gone; every file under `orgs/` is byte-identical.
2. After (1), `workspaces.list` on a fresh daemon on the same `$HF_CONFIG` emits zero summaries.
3. `workspaces.delete name=main dry_run=true` emits the same deletion event but leaves `workspaces/main.yaml` byte-identical.
4. `workspaces.delete name=ghost` emits a typed not-found naming `ghost`; no file under `$HF_CONFIG` changes.
5. `workspaces.delete name=main dry_run=true delete_remote=true` emits per-member cascade events (one per ref) AND the workspace-deleted event; nothing is deleted locally; `orgs/` is byte-identical; no tier-2 traffic occurred.
6. Before/after every scenario above, the filesystem tree at the workspace's `path` is byte-identical.

## Completion

- Run `bash tests/v5/V5WS/V5WS-5.sh` → exit 0.
- Status flips in-commit with the implementation.
