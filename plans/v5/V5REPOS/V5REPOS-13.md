---
id: V5REPOS-13
title: "repos.sync — pull metadata, emit SyncDiff with DriftField per mismatch"
status: Ready
type: implementation
blocked_by: [V5REPOS-2, V5REPOS-12]
unlocks: [V5REPOS-15, V5WS-9]
---

## Problem

Users cannot see whether their org-yaml view of a repo's metadata agrees
with the forge's. `repos.sync` reads remote metadata through `ForgePort`,
compares against local state, and emits a `SyncDiff` reporting each
mismatch. Read-only; no writes to disk or forge. This ticket also pins
the wire shape of `SyncDiff` that V5WS-9 will aggregate per-workspace.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing org file |
| `name` | `RepoName` | yes | MUST match a repo entry under that org |
| `remote` | `RemoteUrl` | no | when present, sync only the matching remote; else sync every remote and aggregate |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `SyncDiff` event per remote compared | `ref`, `status ∈ SyncStatus`, `drift: [DriftField]` |
| not found | typed error event | `org`, `name`, or (when supplied) `remote` absent |
| adapter error | `SyncDiff` with `status == "errored"` and an error field | does NOT abort other remotes — each remote produces its own `SyncDiff` |

`SyncStatus` mapping (per §types):
- `in_sync` → zero entries in `drift`
- `drifted` → one or more `DriftField` entries, one per mismatched `DriftFieldKind`
- `errored` → the forge call failed (one of the five V5REPOS-2 error classes); `drift` is empty

Local state for comparison: the four `DriftFieldKind` fields are NOT
declared on repos in the org yaml today. For v1, `repos.sync` treats
the local values as "unknown" if not explicitly declared — drift is
reported against whatever values the user has configured. The
implementation may pin the local declaration site as part of this
ticket's scope, but the wire shape is what matters here.

Edge cases:

- Repo with multiple remotes on different providers: one `SyncDiff` per remote, adapters dispatched per-provider (V5REPOS-12).
- `remote` parameter supplied but no matching remote under the repo: typed not-found error; no `SyncDiff`.
- Adapter for one remote fails while another succeeds: both `SyncDiff` events are emitted, one `errored`, one `in_sync`/`drifted`.
- No filesystem mutation. No forge mutation.

## What must NOT change

- v4's sync behavior.
- The `ForgePort` capability pinned in V5REPOS-2: `repos.sync` MUST NOT require a method the trait doesn't declare.
- Per the Secret redaction rule, no event contains a resolved credential value.
- `SyncDiff`'s wire shape from §types is pinned here; V5WS-9 depends on stability.

## Acceptance criteria (tier 2)

1. With `$HF_TEST_GITHUB_ORG`, `$HF_TEST_GITHUB_REPO`, `$HF_TEST_GITHUB_TOKEN` set, a fixture pointing at the test repo with local metadata matching the forge produces one `SyncDiff` event with `.status == "in_sync"` and `(.drift | length) == 0`.
2. With the local description deliberately mismatched, `repos.sync` produces one `SyncDiff` with `.status == "drifted"` and `.drift` containing at least one `DriftField` whose `.field == "description"`, `.local` equal to the local value, `.remote` equal to the forge value.
3. A repo with two remotes (e.g., a GitHub remote and an unreachable custom-domain remote) produces two `SyncDiff` events; at least one `in_sync`/`drifted` and at least one `errored`.
4. `repos.sync org=... name=... remote=<url>` limits output to exactly one `SyncDiff` matching that URL.
5. After any `repos.sync` call, the org file on disk is byte-identical to its pre-call state (sync is read-only).
6. No event emitted by `repos.sync` contains the literal value of `$HF_TEST_GITHUB_TOKEN`.
7. Test script exits 0 with a `SKIP:` line when required env vars are unset.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-13.sh` → exit 0.
- Status flips in-commit with the implementation.
