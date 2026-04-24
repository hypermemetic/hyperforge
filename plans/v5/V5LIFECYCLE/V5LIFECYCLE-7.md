---
id: V5LIFECYCLE-7
title: "repos.purge — hard-delete, gated on dismissed"
status: Complete
type: implementation
blocked_by: [V5LIFECYCLE-4, V5LIFECYCLE-5]
unlocks: [V5LIFECYCLE-11]
---

## Problem

Some repos genuinely need to go. `repos.purge` is the hard-delete: remove the remote, remove the local record. Gated on `lifecycle: dismissed` so no accident hard-deletes a live repo.

## Required behavior

Method signature:

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | |
| `name` | `RepoName` | yes | |
| `dry_run` | `bool` | no (default false) | D7 |

Execution order:

1. Load via `ops::state::load_all`.
2. Find repo; absent → `error { code: not_found }`.
3. If `protected == true` → `error { code: protected }`.
4. If `lifecycle != dismissed` → `error { code: not_dismissed, message: "purge requires lifecycle: dismissed; run repos.delete first" }`.
5. For each provider in `privatized_on` (the set accumulated at dismiss time) + every provider derivable from the remotes list:
   - Call `ops::repo::delete_on_forge` (V5LIFECYCLE-4).
   - On success: emit `forge_deleted { provider, url }`.
   - On `not_found`: emit `forge_deleted { provider, url, note: "already gone" }` — idempotent.
   - On any other error: emit `purge_error { provider, error_class, message }` and **continue** to next provider (don't block on one failure).
6. Call `ops::repo::purge(&mut org, &name)`.
7. Save via `ops::state::save_org`.
8. Emit `repo_purged { ref }`.

`dry_run: true` skips the actual forge and yaml mutations; events emit with `dry_run: true` in payload.

## What must NOT change

- V5LIFECYCLE-6's `repos.delete` behavior.
- `repos.remove` (V5REPOS-6) — still the local-only hard removal.
- D13 — all forge calls through `ops::repo::delete_on_forge`.

## Acceptance criteria

1. Purge a `dismissed` non-protected repo: emits `forge_deleted` per provider + `repo_purged`. Local record gone (`repos.list` no longer contains it). Forge repo gone (`gh repo view` 404).
2. Purge an `active` repo: emits `error { code: not_dismissed }`. No forge calls, yaml byte-identical.
3. Purge a `dismissed` + `protected: true` repo: emits `error { code: protected }`. No forge calls, yaml byte-identical.
4. Purge a `dismissed` repo whose remote is already gone on the forge (pre-deleted): emits `forge_deleted { note: "already gone" }` + `repo_purged`; local record dropped.
5. `dry_run: true` emits the same stream, no mutations.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-7.sh` → exit 0 (tier 2).
- Status flips in-commit.
