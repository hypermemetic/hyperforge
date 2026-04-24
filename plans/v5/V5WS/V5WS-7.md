---
id: V5WS-7
title: "workspaces.remove_repo — drop a RepoRef with optional forge cascade"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users drop a single repo from a workspace's membership, with an opt-in
cascade that also deletes the repo on its forge (README §4 — forge-side
deletion requires explicit `delete_remote: true`).

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing workspace |
| `repo_ref` | `RepoRef` | yes | string or object form; matches by ref regardless of the yaml entry's shape |
| `delete_remote` | `bool` | no (default false) | when true, cascade via the same forge-delete path used by `repos.remove delete_remote=true` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| repo removed | one `WorkspaceSummary` with new `repo_count` | |
| cascade (when `delete_remote: true`) | one event carrying `ref: RepoRef` + status (`forge_deleted` / `forge_delete_failed`) | |
| workspace not found / ref not a member | typed error event | names the offending input; yaml byte-identical |

Post-conditions. `dry_run: false`, `delete_remote: false`: matched
entry is removed from `workspaces/<name>.yaml`; org yamls untouched.
`dry_run: false`, `delete_remote: true`: same plus forge-delete invoked;
the yaml entry drops even if the cascade fails. `dry_run: true`: every
event emitted including cascade; no local file modified; no forge
contact.

Edge cases: yaml entry in object form (`{ref, dir}`) is matched by ref
and removed as a whole — `dir` is not material. Duplicate entries (ref
appears twice) → first removed; a typed warning names the duplicate
left behind. `delete_remote: true` without adapter creds → cascade
event reports failure; yaml entry still drops on `dry_run: false`.

## What must NOT change

- v4's `workspace.*` namespace. v5 writes only `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yamls are READ-only — cascade does NOT rewrite `orgs/*.yaml`.
- Default (`delete_remote: false`) NEVER contacts any forge.
- The local directory under the workspace `path` is never `rm`'d — `remove_repo` is yaml + (optional) forge only.

## Acceptance criteria

1. Against `ws_cross_org`, `workspaces.remove_repo name=multi repo_ref=acme/tool` emits a `WorkspaceSummary` with `repo_count == 1`; `workspaces.get` shows only `demo/widget`.
2. Same call with `dry_run=true` emits the same-shape event; yaml byte-identical.
3. With the member in object form `{ref: "demo/widget", dir: "widget-local"}`, `workspaces.remove_repo name=main repo_ref=demo/widget` leaves `repos == []`.
4. `workspaces.remove_repo name=multi repo_ref=ghost/none` emits a typed error naming `ghost/none`; file byte-identical.
5. `workspaces.remove_repo name=ghost …` emits a typed not-found naming `ghost`; no file modified.
6. `workspaces.remove_repo name=main repo_ref=demo/widget dry_run=true delete_remote=true` emits both a cascade event for the ref and a `WorkspaceSummary`; no local file modified; `orgs/` byte-identical; no tier-2 traffic occurred.
7. Across every scenario above, the filesystem tree at the workspace's `path` is byte-identical.

## Completion

- Run `bash tests/v5/V5WS/V5WS-7.sh` → exit 0.
- Status flips in-commit with the implementation.
