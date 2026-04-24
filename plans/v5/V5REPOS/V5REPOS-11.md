---
id: V5REPOS-11
title: "GitLab ForgePort adapter"
status: Complete
type: implementation
blocked_by: [V5REPOS-2]
unlocks: [V5REPOS-13, V5REPOS-14, V5REPOS-15]
---

## Problem

Users with GitLab remotes (either gitlab.com or self-hosted via
per-remote `provider: gitlab` override) need the `ForgePort` capability
implemented. This ticket delivers the GitLab adapter.

## Required behavior

Dispatched when a `Remote`'s derived provider is `gitlab`. Implements
both V5REPOS-2 capability methods against the GitLab REST v4 API using
the credential entry resolved from the org.

**Read metadata.**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | provider == `gitlab` |
| `repo_ref` | `RepoRef` | yes | |

| Output / Event | Shape | Notes |
|---|---|---|
| success | metadata value with exactly the four `DriftFieldKind` fields | |
| error | one of the five V5REPOS-2 error classes | |

**Write metadata.** Same shape as V5REPOS-2 capability method 2.

Host selection: extracted from the `Remote.url` (not hardcoded to
`gitlab.com`). Self-hosted GitLab is reachable by setting a per-remote
`provider: gitlab` override in the org YAML. The adapter builds its base
URL from the remote's host.

Credential selection: first `CredentialType::token` entry on the org.
Missing → `auth`.

Edge cases:

- GitLab's `visibility` is tri-state (`public`, `internal`, `private`) — serialized as `String` directly.
- GitLab uses URL-encoded `namespace%2Fproject` in its project endpoint; the adapter owns that encoding.
- Self-hosted GitLab with TLS to a non-public CA: out of v1 scope; transport errors → `network`.
- GitLab's 404 → `not_found`; 401/403 → `auth`; rate limit → `rate_limited`.

## What must NOT change

- The `ForgePort` capability pinned in V5REPOS-2.
- Per D3, only `{default_branch, description, archived, visibility}` traverse the capability.
- Per the Secret redaction rule, the resolved token MUST NOT appear in any event emitted by the adapter.

## Acceptance criteria

1. With `$HF_TEST_GITLAB_HOST` (defaults to `gitlab.com`), `$HF_TEST_GITLAB_ORG`, `$HF_TEST_GITLAB_REPO`, and `$HF_TEST_GITLAB_TOKEN` set, the read-metadata capability returns a value whose top-level keys are exactly `default_branch`, `description`, `archived`, `visibility`.
2. `visibility` is one of `public`, `internal`, `private` — no other values.
3. The write capability applied with `{description: "hyperforge-v5-repos-11 <timestamp>"}` succeeds; a subsequent read reflects the new value. The test MUST restore the original description before exit.
4. Calling the read capability with the token env var unset produces an `auth` error event.
5. Calling the read capability against `<org>/nonexistent-<timestamp>` produces `not_found`.
6. No event emitted by the adapter contains the literal value of `$HF_TEST_GITLAB_TOKEN`.
7. Test script exits 0 with a `SKIP:` line when any of the required env vars (`$HF_TEST_GITLAB_ORG`, `$HF_TEST_GITLAB_REPO`, `$HF_TEST_GITLAB_TOKEN`) is unset.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-11.sh` → exit 0.
- Status flips in-commit with the implementation.
