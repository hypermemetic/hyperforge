---
id: V5REPOS-4
title: "repos.get — RepoDetail including remote list with derived providers"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users need one repo's full shape — its `RepoRef` plus every remote with
the provider derivation resolved. This is also the observation surface
that verifies V5REPOS-12's URL-to-provider rule without a debug method.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |
| `name` | `RepoName` | yes | MUST match an entry in that org's `repos[]` |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `RepoDetail` event | `ref` = `{org, name}`; `remotes` is the declared list with each entry's `provider` surfaced (derived per V5REPOS-12 when not explicit) |
| not found | typed error event | names the `org` and `name` that were requested |

Edge cases:

- Org absent: typed not-found error naming the org; no `RepoDetail`.
- Org present, repo absent: typed not-found error naming both; no `RepoDetail`.
- Repo with zero remotes: emits `RepoDetail` with `remotes == []`.
- Any remote whose provider cannot be derived (domain unknown, no override) yields a derivation error from V5REPOS-12 bubbled through this method; no partial `RepoDetail`.

## What must NOT change

- v4's repo-get equivalent on the v4 daemon.
- Read-only. No filesystem mutation.
- The wire shape of `RepoDetail` from §types is final; this ticket cannot extend it.

## Acceptance criteria

1. Against `org_with_repo`, `repos.get org=demo name=widget` emits exactly one `RepoDetail` where `.ref.org == "demo"`, `.ref.name == "widget"`, `(.remotes | length) == 1`, and `.remotes[0].provider == "github"`.
2. Against `org_with_mirror_repo`, `repos.get org=demo name=widget` emits one `RepoDetail` with `(.remotes | length) == 2`; remote providers are `github` then `codeberg` in order.
3. Against `org_with_custom_domain_repo`, the single remote's `provider` field is `gitlab` — the per-remote override takes effect.
4. `repos.get org=nonexistent name=anything` emits a typed error event; no `RepoDetail`.
5. `repos.get org=demo name=nonexistent` against `org_with_repo` emits a typed error event naming `nonexistent`; no `RepoDetail`.
6. `repos.get` missing either `org` or `name` emits a typed error event (missing required parameter).

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-4.sh` → exit 0.
- Status flips in-commit with the implementation.
