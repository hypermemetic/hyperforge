---
id: V5REPOS-12
title: "URL → ProviderKind derivation via domain map + per-remote override"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-13, V5REPOS-14, V5REPOS-15]
---

## Problem

Adapters are dispatched per-remote, so every `Remote` must resolve to
exactly one `ProviderKind`. Two inputs participate: the remote's
`provider:` override (if present) and the global `provider_map` from
`config.yaml`. This ticket pins the derivation as a total function and
verifies the result observable on the wire via `repos.get` — no
debug surface needed.

## Required behavior

Derivation rule (total, deterministic):

1. If `Remote.provider` is set, that is the result. The URL's domain is
   not consulted.
2. Else extract the host from `Remote.url` (SSH `git@host:path` form, HTTPS
   `https://host/path` form, and `ssh://user@host/path` form all parse to
   a host). Lowercase.
3. Look up the host in `config.yaml`'s `provider_map`. If present, that
   is the result.
4. Otherwise: derivation fails — a typed error at the wire boundary,
   naming the URL and the extracted host. Never a fallback default.

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | shape from §types |
| `config.yaml` contents | `{DomainName: ProviderKind}` map | yes | owned by V5CORE-3, consumed here |

| Output / Event | Shape | Notes |
|---|---|---|
| success | `ProviderKind` attached to the remote in every `RepoDetail`/`RepoSummary` that surfaces it | the derived provider is observable on the wire |
| failure | typed error naming URL + host | never silently "unknown" |

Verification strategy (per ticketing guidance): rather than a debug
surface, derivation is verified indirectly by reading a repo and asserting
the remote's provider in the `RepoDetail` event matches the expected
value. This keeps the wire surface minimal.

Edge cases:

- URL with no parseable host (e.g., local path `../foo.git`): derivation
  fails with a host-extraction error, not a lookup error.
- Domain map key matches case-insensitively (both sides lowercased before
  comparison). An uppercase entry in the map is still a hard error at
  load time via the `DomainName` constraint from §types.
- Per-remote `provider:` override set to an unknown variant is rejected
  at load time (closed-variant rule), not at derivation time.

## What must NOT change

- The shape of `provider_map` in `config.yaml`. Owned by V5CORE-3.
- The `Remote` shape. Owned by V5REPOS-7's ticket body once it pins it;
  this ticket only consumes it.

## Acceptance criteria

1. Against the `org_with_repo` fixture, `repos.get org=demo name=widget` emits a `RepoDetail` whose first remote's derived `provider` equals `github`.
2. Against the `org_with_mirror_repo` fixture, `repos.get org=demo name=widget` emits a `RepoDetail` whose two remotes derive to `github` and `codeberg` respectively, in the order declared.
3. Against the `org_with_custom_domain_repo` fixture, the repo's single remote derives to `gitlab` — the per-remote override wins over the (absent) domain-map entry.
4. A remote whose URL domain is not in `provider_map` and has no override produces a typed error referencing both the URL and the extracted host; no silent default.
5. Removing the `provider_map` entry for `github.com` from a fixture and reloading causes `repos.get` on a github-domain remote to emit the derivation error; adding the entry back restores success without a daemon restart.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-12.sh` → exit 0.
- Status flips in-commit with the implementation.
