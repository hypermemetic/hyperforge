---
id: V5PARITY-29
title: "GITIGNORE-SYNC — propagate a canonical .gitignore across workspace members"
status: Pending
type: implementation
blocked_by: []
unlocks: []
---

## Problem

v4 had `build.gitignore_sync` for keeping a canonical `.gitignore` template synchronized across every member of a workspace. Useful when you maintain a polyrepo with shared editor / build / OS conventions and don't want each repo to drift. v5 has no equivalent — members' `.gitignore` files are independent.

## Required behavior

**`build.gitignore_sync --name W [--source PATH] [--mode merge|overwrite] [--dry_run bool]`**

Two modes:
- `merge` (default): adds any line from the source template that's not already present in the member's `.gitignore`. Existing lines preserved; comments untouched.
- `overwrite`: replaces the member's `.gitignore` with the source verbatim.

**Source resolution order:**
1. `--source PATH` (explicit absolute or relative path on the daemon host).
2. `<workspace_path>/.gitignore.template` if the workspace path has one.
3. Otherwise: error with `no_template`.

**Per-member events**: `gitignore_updated { ref, mode, lines_added, lines_unchanged, dry_run }`. Members with no `.gitignore` get one created (in merge mode this is equivalent to overwrite).

**Aggregate**: `workspace_gitignore_summary { name, total, updated, unchanged, errored }`.

## What must NOT change

- V5PARITY-19's filter / dry-run cross-cut applies — `--filter <glob>` and `--dry_run` work the same as the workspace git verbs.
- `.hyperforge/config.toml` is untouched; this only writes the repo's `.gitignore`.
- D13 — file writes route through `ops::fs::write_atomic` (V5LIFECYCLE-2's atomic write helper).

## Acceptance criteria

1. `build.gitignore_sync --name W --source /tmp/template.gitignore` against a 3-member workspace where one member has no `.gitignore` adds the file; the other two get missing-line additions.
2. `--dry_run true` emits the events as if writing happened but the files on disk are byte-identical to before.
3. `--mode overwrite` replaces the file content entirely; `lines_unchanged: 0` always when overwrite mode succeeds.
4. Re-running merge mode is a no-op — second run reports `lines_added: 0, lines_unchanged: N` for every member.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-29.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
