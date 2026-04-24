---
id: V5REPOS-9
title: "GitHub ForgePort adapter"
status: Complete
type: implementation
blocked_by: [V5REPOS-2]
unlocks: [V5REPOS-13, V5REPOS-14, V5REPOS-15]
---

## Problem

The `ForgePort` capability pinned in V5REPOS-2 must have a working
GitHub implementation before `repos.sync` / `repos.push` can land.
This ticket delivers that adapter — nothing more, nothing less.

## Required behavior

The adapter is dispatched when a `Remote`'s derived provider
(V5REPOS-12) is `github`. It implements both capability methods from
V5REPOS-2 against `api.github.com` using the credential entry resolved
from the org via V5CORE-4's secret resolver.

**Read metadata (V5REPOS-2 capability method 1).**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | provider == `github` |
| `repo_ref` | `RepoRef` | yes | |

| Output / Event | Shape | Notes |
|---|---|---|
| success | metadata value with exactly the four `DriftFieldKind` fields | `default_branch`, `description`, `archived`, `visibility` |
| error | one of the five error classes from V5REPOS-2 | `not_found`, `auth`, `network`, `unsupported_field`, `rate_limited` |

**Write metadata (V5REPOS-2 capability method 2).**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | |
| `repo_ref` | `RepoRef` | yes | |
| `fields` | map of `DriftFieldKind` → value | yes | only declared fields are written |

Credential selection: the adapter consults the org's `credentials[]`
list, picking the first entry of `CredentialType::token` and resolving
its `SecretRef` through the V5CORE-4 resolver. Missing credential → `auth`.

Edge cases:

- Private-repo access without sufficient token scope → `auth`, not `not_found`.
- GitHub's `visibility` is a tri-state (`public`, `private`, `internal`); it serializes as `String`.
- Rate-limit response from the GitHub API → `rate_limited`; the error MUST surface the reset window if provided in response headers.

## What must NOT change

- The `ForgePort` capability pinned in V5REPOS-2. If a GitHub-only field seems needed, it is an adapter-specific extension, not a trait addition.
- Per D3, only `{default_branch, description, archived, visibility}` traverse the capability.
- Per the Secret redaction rule, the resolved token MUST NOT appear in any event emitted by the adapter.

## Acceptance criteria

1. With `$HF_TEST_GITHUB_ORG`, `$HF_TEST_GITHUB_REPO`, and `$HF_TEST_GITHUB_TOKEN` set, the read-metadata capability called with a GitHub-domain remote returns a metadata value whose top-level keys are exactly `default_branch`, `description`, `archived`, `visibility`.
2. With the same env vars, the write-metadata capability applied with `{description: "hyperforge-v5-repos-9 <timestamp>"}` succeeds; a subsequent read reflects the new description. The test MUST restore the original description before exit.
3. Calling the read capability with the token env var unset (or blank) produces an `auth` error event; no partial success.
4. Calling the read capability against `<org>/definitely-does-not-exist-<timestamp>` produces a `not_found` error event.
5. No event emitted by the adapter at any point in the test contains the literal value of `$HF_TEST_GITHUB_TOKEN`.
6. Test script exits 0 with a `SKIP:` line when any of the three required env vars is unset — not a failure.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-9.sh` → exit 0.
- Status flips in-commit with the implementation.
