---
id: V5PARITY-14
title: "WORKSPACE-GIT — extend workspace verbs (push rename + status/checkout/commit/tag)"
status: Pending
type: implementation
blocked_by: [V5PARITY-3, V5PARITY-12]
unlocks: []
---

## Problem

V5PARITY-3 gave workspaces the four "transport" verbs (`clone`, `fetch`, `pull`, `push_all`). Daily use exposes two gaps:

1. The `pull` / `push_all` asymmetry is a v4 holdover; symmetric `pull` / `push` reads cleaner.
2. There's no way to inspect or coordinate state across members without dropping to per-repo calls. `status` (audit), `checkout` (switch a feature branch everywhere), `commit` (sync a uniform edit), and `tag` (release coordination) all show up in workspace-wide flows.

## Required behavior

**RPC methods on WorkspacesHub:**

| Method | Behavior |
|---|---|
| `workspaces.push --name W [--remote R] [--branch B]` | Replaces `push_all`. Same semantics; symmetrical naming with `pull`. `push_all` is removed (no alias — clean break, this surface is new and v4-only callers are migrating anyway). |
| `workspaces.status --name W` | Per-member `status_snapshot { ref, branch, ahead, behind, dirty, staged, unstaged, untracked }`; aggregate `workspace_status_summary { total, dirty, ahead, behind }`. Read-only. |
| `workspaces.checkout --name W --branch B [--create bool]` | Switch every member to branch `B`. With `--create true`, creates the branch if it doesn't exist locally. Per-member `member_git_result`; aggregate `workspace_git_summary` (existing shape from V5PARITY-3). |
| `workspaces.commit --name W --message M [--allow_empty bool] [--only_dirty bool]` | Run `git commit -m M` in every member. `--only_dirty true` (default) skips members with no staged changes; `--allow_empty true` overrides for ceremonial commits. |
| `workspaces.tag --name W --tag T [--message M]` | Apply tag `T` across every member. Annotated when `--message` is given; lightweight otherwise. |

All five reuse V5PARITY-3's `git_op` iterator and event vocabulary (`MemberGitResult`, `WorkspaceGitSummary`) wherever shapes match — `status` is the only one that needs a new event variant.

## What must NOT change

- V5PARITY-3's `repos.{clone,fetch,pull,push_refs,status,dirty}` per-repo methods stay byte-identical.
- V5PARITY-12's `command-git` DRY invariant — every git call goes through `ops::git::*` wrappers.
- Bounded parallelism + partial-failure-tolerance pattern (D6).

## Acceptance criteria

1. `workspaces.push --name W` succeeds against a workspace with 2+ members; `push_all` is no longer accepted (returns "command not found").
2. `workspaces.status --name W` on a workspace with one dirty member emits `status_snapshot` per member and a `workspace_status_summary { dirty: 1, ... }` aggregate.
3. `workspaces.checkout --name W --branch feat-x --create true` creates and switches to `feat-x` across every member; second invocation is a no-op (already on the branch).
4. `workspaces.commit --name W --message "uniform"` after staging the same edit in two members produces two commits; with `--only_dirty true` (default), members with no staged changes are skipped (status: `skipped`, not `errored`).
5. `workspaces.tag --name W --tag v0.1.0` applies the tag to every member; failures on individual members surface as `errored` status without aborting the run.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-14.sh` → exit 0 (tier 1; uses local bare repos).
- Ready → Complete in-commit.
