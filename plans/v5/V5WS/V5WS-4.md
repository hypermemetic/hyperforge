---
id: V5WS-4
title: "workspaces.create — write new workspace yaml with dry_run"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users currently must hand-edit `workspaces/<name>.yaml` to onboard a
workspace. `workspaces.create` writes that file from typed parameters,
validating every repo ref exists in its org yaml. D7.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | filename-safety at the wire boundary |
| `ws_path` | `FsPath` | yes | named `ws_path` to avoid synapse path-expansion of a param named `path` |
| `repos` | `[WorkspaceRepo]` | no (default `[]`) | each entry must resolve against `orgs/<org>.yaml` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `WorkspaceSummary` | `repo_count == len(repos)` |
| already exists / invalid name / invalid path / unknown repo ref | typed error event | names the offending input |

Pinned here: the workspace yaml shape — top-level keys `name`, `path`,
`repos`; entries are `WorkspaceRepo` (serde untagged). V5WS-3/5/6/7/8/9
read and write this shape.

Post-condition on `dry_run: false` success: `workspaces/<name>.yaml`
exists on disk and round-trips through V5CORE-3 to the input. Write is
atomic per D8. Post-condition on `dry_run: true`: same event emitted,
no file exists.

Edge cases: `name` or `ws_path` constraint failure → typed error, no
write (even under `dry_run`). `name` already exists → typed error,
file byte-identical. Any unresolved ref → typed error naming every
unresolved entry, no write. `ws_path` not existing on disk is
accepted — `create` never touches the target directory.

## What must NOT change

- v4's `workspace.*` namespace. v5 writes only `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yamls are READ-only — `create` validates against them but never mutates.
- The filesystem at `ws_path` is never touched (no `mkdir`, no clone). Binding dir-to-ref is reconcile's job.
- Secret redaction rule: `create` accepts no secret values.

## Acceptance criteria

1. Against `ws_empty`, `workspaces.create name=main ws_path=/tmp/dev repos='[]'` emits a `WorkspaceSummary` with `name == "main"`, `repo_count == 0`; `workspaces/main.yaml` exists after.
2. After (1), a fresh-daemon `workspaces.list` on the same `$HF_CONFIG` includes `main` — state is on disk.
3. `workspaces.create name=main ws_path=/tmp/dev repos='["demo/widget"]'` succeeds; `workspaces.get name=main` returns `repos` containing `"demo/widget"` in string form.
4. `dry_run=true` on any success scenario emits the same `WorkspaceSummary` but leaves no file on disk.
5. Against `ws_with_one_repo`, `workspaces.create name=main …` emits a typed error naming `main`; existing file byte-identical.
6. `workspaces.create … repos='["ghost/nothing"]'` emits a typed error naming `ghost/nothing`; no file written.
7. `workspaces.create name=bad/name …` and `workspaces.create … ws_path=relative/path …` each emit a typed error and write no file.

## Completion

- Run `bash tests/v5/V5WS/V5WS-4.sh` → exit 0.
- Status flips in-commit with the implementation.
