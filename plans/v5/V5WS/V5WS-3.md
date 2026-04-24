---
id: V5WS-3
title: "workspaces.get — WorkspaceDetail with resolved WorkspaceRepo entries"
status: Pending
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users need one workspace's full shape — its path and its repo refs —
without hand-reading the yaml. The return type must preserve both
string and object forms of `WorkspaceRepo` as they appear on disk.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing `workspaces/<WorkspaceName>.yaml` |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `WorkspaceDetail` event | `repos` is `[WorkspaceRepo]`, each entry either string or object form (serde untagged) |
| not found | typed error event | names the `WorkspaceName` that was requested |

Edge cases:

- `name` absent from disk: typed not-found error; no `WorkspaceDetail` emitted.
- Workspace yaml has an empty `repos` list: `WorkspaceDetail.repos` is the empty list, not missing.
- Workspace yaml mixes string and object forms: both shapes appear in `repos` in source order.
- `name` parameter missing: typed error (missing required parameter).

## What must NOT change

- v4's `workspace.*` namespace unchanged. v5 reads only from `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yaml files are READ-only from v5 workspaces. `get` does NOT resolve refs against `orgs/*.yaml` — it returns whatever refs the workspace yaml contains (unvalidated). Dangling refs are a reconcile concern, not a `get` concern.
- No filesystem mutation — `workspaces.get` is read-only.

## Acceptance criteria

1. Against `ws_with_one_repo`, `workspaces.get name=main` emits exactly one `WorkspaceDetail` with `name == "main"`, `path` equal to the fixture path, and `repos` of length 1.
2. Against `ws_cross_org`, `workspaces.get name=multi` returns a `WorkspaceDetail` whose `repos` list contains two entries — one per org — in source order.
3. A workspace yaml containing one string-form entry and one object-form entry round-trips through `workspaces.get` with both shapes preserved (string stays string, object stays object).
4. `workspaces.get name=nonexistent` emits a typed error event referencing `nonexistent`; no `WorkspaceDetail` event is emitted.
5. `workspaces.get` without the `name` parameter emits a typed error event (missing required parameter).

## Completion

- Run `bash tests/v5/V5WS/V5WS-3.sh` → exit 0.
- Status flips in-commit with the implementation.
