---
id: V5PARITY-18
title: "WS-DIFF — workspace-wide change visibility"
status: Pending
type: implementation
blocked_by: [V5PARITY-14, V5PARITY-15]
unlocks: []
---

## Problem

V5PARITY-14 added `workspaces.status` (clean/dirty per member). It tells you *which* members changed but not *what* changed. For coordinated edits, release prep, and pre-merge audits, you need to see the actual file-level diff signal across the workspace.

## Required behavior

**`workspaces.diff --name W [--from REF] [--to REF] [--name_only bool]`**

Three modes by argument shape:
- No `--from` / `--to` → diff working tree against `HEAD` (the "what's uncommitted" mode).
- `--from REF` only → diff `HEAD` against `REF`.
- `--from REF --to REF2` → diff between two refs.

**Per-member event:** `member_diff { ref, files_changed, insertions, deletions, files: [{path, status: "added|modified|deleted|renamed", insertions, deletions}] }`. With `--name_only true`, the `files` array contains `{path, status}` only — no line counts.

**Workspace-wide aggregate:** `workspace_diff_summary { name, total_members, members_with_changes, total_files_changed, total_insertions, total_deletions }`.

**Members with no diff** still emit a `member_diff` event with zero counters (so consumers can correlate against the workspace member list). Errors emit `member_git_result { status: "errored", message }` and the summary's `errored` counter ticks.

## What must NOT change

- D13 — the diff implementation routes through `ops::git` (V5PARITY-15's git2 backend makes per-member diff cheap; without it, falls back to subprocess `git diff --numstat`).
- V5PARITY-14's iteration shape — `workspaces.diff` uses the same `member_ctxs` helper.
- D6 — partial failure tolerance.

## Acceptance criteria

1. `workspaces.diff --name W` against a workspace where one member has uncommitted changes emits a `member_diff` with `files_changed > 0` for that member and `files_changed == 0` for the rest.
2. `workspaces.diff --name W --from HEAD~1 --to HEAD` shows the last commit's changes per member (or zero per-member events if a member's HEAD~1 doesn't resolve).
3. `--name_only true` emits the `files` array without `insertions`/`deletions` per file but keeps the per-member counters.
4. `workspace_diff_summary` totals match the sum of per-member counters.
5. Errored members (e.g. `--from` ref doesn't resolve) emit `member_git_result { errored }` and the summary's `errored` counter is non-zero.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-18.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
