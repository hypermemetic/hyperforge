---
id: V5REPOS-7
title: "repos.add_remote — append a Remote to an existing repo (local only)"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users currently cannot add a mirror or a second remote to an existing
repo without hand-editing YAML. `repos.add_remote` appends one `Remote`
to the repo's declared list. This ticket also pins the final wire shape
of `Remote` — either string-shorthand-equivalent `{url}` form (provider
derived via V5REPOS-12) or explicit `{url, provider}` form — as consumed
by every downstream ticket.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |
| `name` | `RepoName` | yes | MUST match a repo entry under that org |
| `remote` | `Remote` | yes | url validated against `RemoteUrl`; provider (if present) against the closed variant set; (if absent) must derive per V5REPOS-12 |
| `dry_run` | `bool` | no | default false per D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one confirmation event carrying the updated `RepoDetail` | exactly the wire shape `repos.get` returns |
| dry_run preview | identical event stream with no filesystem change | |
| validation failure | typed error event | invalid URL, duplicate URL in the same repo, derivation failure |
| not found | typed error event | `org` or `name` absent |

Edge cases:

- Duplicate URL already present under this repo: validation error; no append.
- Same URL present under a different repo in the same org: allowed (mirrors across repos are permissible).
- Remote with explicit `provider` set to a variant outside `ProviderKind`: rejected at the wire boundary.
- NO forge call is made. Credentials are not consulted. Adding a remote is purely local — a sibling provider's credentials may not yet exist at add time.

## What must NOT change

- v4's `repo.*` namespace.
- Per D7, dry_run emits same events with no change.
- Per D8, writes are atomic.
- The declared order of existing remotes in the repo. The new remote is appended last; no reorder.
- Per the Secret redaction rule, no resolved credential value is emitted.

## Acceptance criteria

1. Against `org_with_repo`, `repos.add_remote org=demo name=widget remote={url:"https://codeberg.org/demo/widget.git"}` succeeds (after ensuring `codeberg.org` is in `provider_map`); a subsequent `repos.get` emits a `RepoDetail` whose `remotes` length is 2 with the new remote last and its `provider == "codeberg"`.
2. With `dry_run=true`, the org file on disk is byte-identical to its pre-call state.
3. Adding a remote whose URL exactly equals a URL already present under the same repo emits a typed duplicate error; no append.
4. Adding a remote with explicit `{url, provider: "gitlab"}` for a custom-domain URL whose host is absent from `provider_map` succeeds — the override wins.
5. Adding a remote with `{url, provider: "unknown"}` emits a typed error at the wire boundary (closed-variant rule).
6. Adding a remote succeeds WITHOUT any credential for the new provider being present on the org — no forge call is attempted.
7. Respawning the daemon after a successful non-dry call yields a `repos.get` result matching the post-call state.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-7.sh` → exit 0.
- Status flips in-commit with the implementation.
