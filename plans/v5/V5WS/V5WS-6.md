---
id: V5WS-6
title: "workspaces.add_repo — append a RepoRef, validated against its org"
status: Pending
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Users extend a workspace's membership one ref at a time without
hand-editing yaml. The new entry must resolve against its org yaml.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing workspace |
| `repo_ref` | `RepoRef` | yes | accepted as string `<org>/<name>` OR object `{org, name}` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `WorkspaceSummary` with the new `repo_count` | |
| workspace not found / org not found / repo not found in org / already a member | typed error event | names the offending ref or workspace; yaml byte-identical |

Pinned here: the canonical ref string form is `<org>/<name>` — one
forward slash separating two tokens that each reject `/`. V5WS-7 and
V5WS-8 accept the same string form; V5WS-8 may rewrite string → object
form on a detected dir rename.

Post-condition on `dry_run: false` success: the ref is appended to
`workspaces/<name>.yaml` `repos` in string form `<org>/<name>`; the
file round-trips through V5CORE-3 to an equal `WorkspaceDetail`. Write
is atomic per D8. Post-condition on `dry_run: true`: same event; file
byte-identical.

Edge cases: already present (string OR object form) → typed error, no
write. Org yaml absent → typed error. Repo name not in that org's
`repos[]` → typed error.

## What must NOT change

- v4's `workspace.*` namespace. v5 writes only `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yamls are READ-only — `add_repo` validates against them but never writes.
- `delete_remote` is NOT a parameter — `add_repo` never contacts a forge.
- The filesystem at the workspace's `path` is never touched (no clone, no `mkdir`, no scan).

## Acceptance criteria

1. Against `ws_cross_org`, `workspaces.add_repo name=multi repo_ref=acme/tool` emits an already-a-member error naming `acme/tool`; file byte-identical.
2. Against `ws_empty` plus a second org `acme/tool` and workspace `main` referencing only `demo/widget`, `workspaces.add_repo name=main repo_ref=acme/tool` emits a `WorkspaceSummary` with `repo_count == 2`; `workspaces.get name=main` returns both refs in string form.
3. Same call with `dry_run=true` emits the same-shape event but the workspace yaml is byte-identical.
4. `workspaces.add_repo name=ghost …` emits a typed error naming `ghost`; no file written.
5. `workspaces.add_repo name=main repo_ref=ghost/widget` emits a typed error naming `ghost/widget`; no file written.
6. `workspaces.add_repo name=main repo_ref=demo/nothing` (org exists, repo doesn't) emits a typed error naming `demo/nothing`; no file written.

## Completion

- Run `bash tests/v5/V5WS/V5WS-6.sh` → exit 0.
- Status flips in-commit with the implementation.
