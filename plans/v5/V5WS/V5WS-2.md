---
id: V5WS-2
title: "workspaces.list — stream WorkspaceSummary per workspace"
status: Pending
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users cannot discover which workspaces the daemon knows about.
`workspaces.list` enumerates every workspace on disk as a typed
`WorkspaceSummary` stream.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|

(No inputs.)

| Output / Event | Shape | Notes |
|---|---|---|
| one event per workspace | `WorkspaceSummary` | `repo_count` is the length of the workspace's `repos[]` after parsing |
| stream terminator | standard synapse completion | caller observes stream end |

Ordering: case-sensitive ascending by `WorkspaceName`. Deterministic
across calls on an unchanged config dir.

Edge cases:

- Zero workspace files: emits zero events, terminates normally. Not an error.
- A file under `workspaces/` that fails to parse: emits a typed error event naming the file; other workspaces still stream.
- Non-`.yaml` files and dotfiles under `workspaces/`: ignored (V5CORE-3 loader contract).

## What must NOT change

- v4's `workspace.*` namespace unchanged. v5 reads only from `~/.config/hyperforge/workspaces/`.
- Org yaml files are READ-only from v5 workspaces. `list` reads nothing outside the `workspaces/` subdir.
- No filesystem mutation — `workspaces.list` is read-only.
- `list` never dereferences any `<org>/<name>` ref — `repo_count` is the length of the yaml list as-written, no forge or org lookup required.

## Acceptance criteria

1. Against the `ws_empty` fixture, `workspaces.list` produces zero `WorkspaceSummary` events and completes successfully.
2. Against `ws_with_one_repo`, exactly one event satisfies `.type == "workspace_summary" and .name == "main" and .repo_count == 1` and its `path` matches the fixture's workspace path.
3. Against `ws_cross_org`, exactly two events are emitted in ascending `name` order; both have `repo_count == 1`.
4. After moving a workspace file out and back in, two successive `workspaces.list` calls against a fresh daemon return equal event sequences.
5. Running `workspaces.list` does not modify any file under `$HF_CONFIG` (byte-identical tree pre/post).

## Completion

- Run `bash tests/v5/V5WS/V5WS-2.sh` → exit 0.
- Status flips in-commit with the implementation.
