---
id: V5REPOS-8
title: "repos.remove_remote — drop a Remote by URL from an existing repo"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users currently cannot deregister a mirror or a single remote without
hand-editing YAML. `repos.remove_remote` drops one `Remote` from the
repo's declared list, identified by its URL. Purely local; per
invariant 4 and D7, forge-side resource deletion is not triggered here
(the repo entry and other remotes remain).

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |
| `name` | `RepoName` | yes | MUST match a repo entry under that org |
| `url` | `RemoteUrl` | yes | MUST match one of the repo's declared remote URLs exactly |
| `dry_run` | `bool` | no | default false per D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one confirmation event carrying the updated `RepoDetail` | same wire shape as `repos.get` |
| dry_run preview | identical event stream with no filesystem change | |
| not found | typed error event | `org`, `name`, or `url` absent |
| invariant violation | typed error event | attempting to remove the last remaining remote (a repo entry with zero remotes is disallowed by V5REPOS-5's validation; this method preserves that invariant) |

Edge cases:

- `url` matches more than one remote (impossible per V5REPOS-7's duplicate rule) — guard exists defensively; if violated the method removes the first match and emits a warning event.
- Removing the last remote: rejected. Users must `repos.remove` the entry as a whole.
- Credentials are NOT consulted. No forge call is made. The remote's forge-side resource is untouched.

## What must NOT change

- v4's `repo.*` namespace.
- Invariant 4: local-only operation; no forge-side effect.
- Per D7, dry_run emits same events with no change.
- Per D8, writes are atomic.
- The declared order of the surviving remotes.
- Per the Secret redaction rule, no resolved credential value is emitted.

## Acceptance criteria

1. Against `org_with_mirror_repo`, `repos.remove_remote org=demo name=widget url=https://codeberg.org/demo/widget.git` succeeds; a subsequent `repos.get` emits a `RepoDetail` with `(.remotes | length) == 1` whose single remote is the github URL.
2. With `dry_run=true`, the org file on disk is byte-identical to its pre-call state.
3. Against `org_with_repo`, attempting to remove the only remote emits a typed last-remote error; the repo entry and its remote are unchanged.
4. `repos.remove_remote` with a `url` not present among the repo's remotes emits a typed not-found error naming the URL; no file change.
5. `repos.remove_remote` against a nonexistent `org` or `name` emits a typed not-found error; no file change.
6. Respawning the daemon after a successful non-dry call yields a `repos.get` result matching the post-call state.
7. No event emitted by `repos.remove_remote` contains a plaintext credential value.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-8.sh` → exit 0.
- Status flips in-commit with the implementation.
