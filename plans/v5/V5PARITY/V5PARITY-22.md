---
id: V5PARITY-22
title: "WS-FROM-ORG — `workspaces.from_org` one-shot creation"
status: Complete
type: implementation
blocked_by: [V5PARITY-3, V5PARITY-21]
unlocks: []
---

## Problem

After `orgs.bootstrap` registers an org with N repos, a user who wants "all of them checked out under `/code/<org>/`" has to: `workspaces.create --name X --path /code/<org>` → manually `workspaces.add_repo` N times → `workspaces.clone --name X`. That's N+2 RPCs for a single common intent.

## Required behavior

**`workspaces.from_org --org <org> --path <abs-path> [--filter <glob>] [--name <ws-name>] [--clone bool] [--update bool]`**

1. Verifies `<org>` exists and is non-empty (else `validation` error).
2. Creates `<path>` if missing.
3. Creates a workspace named `<ws-name>` (defaults to `<org>`) at `<path>`.
4. Adds every repo (or every match of `--filter`) as a workspace member.
5. With `--clone true` (default), clones each member into `<path>/<repo-name>` via `repos.clone`'s SSH-key-aware path.
6. Emits per-stage events: `workspace_created`, `member_added` per repo, and the existing `member_git_result` / `workspace_git_summary` from V5PARITY-3 for the clone phase.

**Filter syntax** matches V5PARITY-19's glob — `*-cli`, `{core/*,libs/util}`, etc.

**Idempotency.** Re-running on an existing workspace adds new members not yet present, skips already-cloned dirs (status `skipped`), and runs `pull` on existing checkouts only when `--update bool` is also passed.

## What must NOT change

- `workspaces.create`, `workspaces.add_repo`, `workspaces.clone` stay; this is composition.
- D6 partial-failure-tolerance — one bad clone doesn't roll back the workspace yaml.
- The org's `CredentialEntry { type: ssh_key }` (if any) is honored via `repos.clone`'s V5PARITY-5 SSH path.

## Acceptance criteria

1. `workspaces.from_org --org demo --path /tmp/demo-ws` against an org with 3 repos creates `/tmp/demo-ws/{repo1,repo2,repo3}` (clones), writes `workspaces/demo.yaml` with all three members.
2. `--filter "*-cli"` against the same org adds only matching repos to the workspace.
3. `--clone false` writes the workspace yaml + members but does no cloning.
4. Re-running on an existing workspace + path emits `member_git_result { status: "skipped" }` for already-present checkouts.
5. A failing clone (e.g. invalid remote) marks that member errored but other members proceed; the workspace yaml lists all of them.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-22.sh` → exit 0 (tier 1 — local bare repos).
- Ready → Complete in-commit.
