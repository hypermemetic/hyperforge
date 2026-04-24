---
id: V5REPOS-5
title: "repos.add — register a new repo with initial Remote[] in an org"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users currently cannot register a repo without hand-editing the org YAML.
`repos.add` writes a new entry into an existing `orgs/<OrgName>.yaml`,
validating the URL set against §types constraints and leaving every
other org field untouched.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |
| `name` | `RepoName` | yes | MUST NOT collide with an existing repo entry in that org |
| `remotes` | `[Remote]` | yes | at least one; each validates against the `RemoteUrl` constraint; if `provider` is absent it MUST derive per V5REPOS-12 |
| `dry_run` | `bool` | no | default false per D7; when true, no write occurs |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one confirmation event carrying the resulting `RepoDetail` | same wire shape as `repos.get` |
| dry_run preview | identical event stream to the real run | flagged as preview; no filesystem change |
| validation failure | typed error event naming the offending field | e.g., invalid URL, duplicate name, derivation failure |
| not found | typed error event | `org` absent |

Edge cases:

- Empty `remotes` list: validation error; a repo with zero remotes cannot be created through this method.
- Duplicate `name` under the same org: validation error; no write.
- A remote URL whose domain is not in `provider_map` AND has no `provider` override: validation error (derivation failure from V5REPOS-12).
- After a successful write, the org file's other fields (`forge`, `credentials`, other `repos` entries) are byte-identical to before except for the new entry.

## What must NOT change

- v4's `repo.*` namespace.
- Per D7, `dry_run: true` MUST emit the same events as a real run without touching disk.
- Per D8, the write is atomic (temp + rename).
- No forge API call is made. `repos.add` is purely local.

## Acceptance criteria

1. Against `minimal_org`, `repos.add org=demo name=widget remotes=[{url:"https://github.com/demo/widget.git"}]` succeeds; a subsequent `repos.get org=demo name=widget` emits a `RepoDetail` matching the request.
2. The same call with `dry_run=true` emits the same confirmation events; the org file on disk is byte-identical to its pre-call state.
3. Respawning the daemon after a successful (non-dry) add yields the same `repos.get` output — daemon state equals disk state.
4. Adding with `remotes=[]` emits a typed error; no file change.
5. Adding with `name=widget` twice emits a typed duplicate-name error on the second call; the first entry remains.
6. Adding with a URL whose domain has no `provider_map` entry and no `provider` override emits a derivation-failure error; no file change.
7. No event emitted by `repos.add` contains a plaintext credential value even if `secrets.yaml` is populated.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-5.sh` → exit 0.
- Status flips in-commit with the implementation.
