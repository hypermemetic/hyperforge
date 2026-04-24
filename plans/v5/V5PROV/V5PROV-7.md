---
id: V5PROV-7
title: "repos.delete: extend delete_remote to call adapter.delete_repo"
status: Ready
type: implementation
blocked_by: [V5PROV-2, V5PROV-3]
unlocks: [V5PROV-9]
---

## Problem

V5REPOS-6 already accepts `delete_remote: bool` but returns
`adapter_error/unsupported_field` when true because the trait had no
delete capability. With V5PROV-2 landing `delete_repo`, `delete_remote`
now has a real implementation path.

## Required behavior

`repos.delete` execution order when `delete_remote: true`:
1. Validate inputs (V5REPOS-6 validations unchanged).
2. Call `adapter.delete_repo` against the first remote.
   - Success: proceed.
   - Error: emit `error` with the adapter's class, leave the local entry intact.
3. Drop the local org YAML entry (atomic per D8).
4. Emit `repo_deleted` success event.

When `delete_remote: false` (default), behavior unchanged from V5REPOS-6:
drop local entry only, no forge call.

New events:

| Event | Emitted when | Payload |
|---|---|---|
| `remote_deleted` | After successful `delete_repo` | `ref: RepoRef`, `url: RemoteUrl` |

## What must NOT change

- Default (`delete_remote: false`) behavior.
- `dry_run: true` makes no forge or disk change.
- Error events retain the D9 envelope shape.

## Acceptance criteria

1. Without `delete_remote`, behavior unchanged.
2. With `delete_remote: true` on an existing remote, the method deletes the remote (verified via `repo_exists` returning `Ok(false)` after) and drops the local entry.
3. With `delete_remote: true` on a repo whose remote doesn't exist (e.g., previously deleted out-of-band), adapter returns `not_found`; the local entry is still dropped (informational event, not a failure) — distinguishable from the auth/network error paths.
4. With `delete_remote: true` but blank credentials, the method fails with `auth` and the local entry is NOT dropped.
5. `dry_run: true` with `delete_remote: true` emits the `remote_deleted`-then-`repo_deleted` stream without any disk or forge mutation.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-7.sh` → exit 0 (tier 2).
- Status flips in-commit.
