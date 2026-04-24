---
id: V5REPOS-6
title: "repos.remove — drop repo entry; delete_remote opt-in to forge delete"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users currently cannot deregister a repo without hand-editing the org
YAML. `repos.remove` drops the entry; per invariant 4 (README) and D7,
forge-side deletion is opt-in via `delete_remote: true`.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |
| `name` | `RepoName` | yes | MUST match a repo entry under that org |
| `delete_remote` | `bool` | no | default false; when true, the adapter for each remote performs a forge-side delete before the local drop |
| `dry_run` | `bool` | no | default false per D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one confirmation event carrying the removed `RepoRef` | paired with one adapter-delete confirmation per remote when `delete_remote=true` |
| dry_run preview | identical event stream; no filesystem change; no forge call | |
| not found | typed error event | `org` or `name` absent |
| adapter failure | typed error event from the relevant adapter | on `delete_remote=true`: first adapter error aborts; local entry MUST remain (transactional) |

Edge cases:

- `delete_remote=false` (default): purely local; no adapter resolved, no forge call, no credentials consulted.
- `delete_remote=true` with multiple remotes: adapters are invoked in declared order; first failure aborts and leaves the local entry intact.
- `delete_remote=true` with an unresolvable provider (V5REPOS-12 derivation failure on any remote): aborts before any forge call; local entry intact.
- After a successful local-only remove, other repos under the same org are unchanged.

## What must NOT change

- v4's `repo.*` namespace.
- Invariant 4: plain remove is always safe; forge-side deletion requires the explicit flag.
- Per D7, dry_run emits same events with no change.
- Per D8, local writes are atomic.
- Per the Secret redaction rule, no resolved credential value is emitted.

## Acceptance criteria

1. Against `org_with_repo`, `repos.remove org=demo name=widget` succeeds with `delete_remote` absent; a subsequent `repos.list org=demo` emits zero events.
2. With `dry_run=true`, the org file on disk is byte-identical to pre-call state and a subsequent `repos.get org=demo name=widget` still succeeds.
3. `repos.remove org=demo name=nonexistent` emits a typed not-found error; no file change.
4. `repos.remove org=demo name=widget delete_remote=true` against a fixture whose remote adapter would fail (e.g., missing credential) emits a typed adapter error; a subsequent `repos.get` still returns the repo — the local entry is preserved on forge failure.
5. Respawning the daemon after a successful non-dry remove yields `repos.list` output matching the post-remove state.
6. No event emitted by `repos.remove` contains a plaintext credential value.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-6.sh` → exit 0.
- Status flips in-commit with the implementation.
