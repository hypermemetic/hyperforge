---
id: V5REPOS-10
title: "Codeberg ForgePort adapter (Gitea-compatible)"
status: Complete
type: implementation
blocked_by: [V5REPOS-2]
unlocks: [V5REPOS-13, V5REPOS-14, V5REPOS-15]
---

## Problem

Codeberg is Gitea-based. Users with mirrors across GitHub and Codeberg
need the `ForgePort` capability satisfied by a Codeberg adapter that
speaks the Gitea REST API. This ticket delivers that adapter.

## Required behavior

Dispatched when a `Remote`'s derived provider is `codeberg`. Implements
both capability methods from V5REPOS-2 against `codeberg.org`'s Gitea
API using the credential entry resolved from the org via V5CORE-4.

**Read metadata.**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | provider == `codeberg` |
| `repo_ref` | `RepoRef` | yes | |

| Output / Event | Shape | Notes |
|---|---|---|
| success | metadata value with exactly the four `DriftFieldKind` fields | |
| error | one of the five V5REPOS-2 error classes | |

**Write metadata.** Same input shape as V5REPOS-2 capability method 2;
same closed error class set.

Credential selection: first `CredentialType::token` entry on the org.
Resolved via V5CORE-4. Missing → `auth`.

Edge cases:

- Gitea returns `private: bool` rather than `visibility: String`. The adapter MUST map it to the §types `String` shape: `"public"` or `"private"`.
- `archived` is present on Gitea (`archived: bool`) — mapping is direct.
- Gitea's 404 for a missing repo maps to `not_found`; 401/403 → `auth`; transport errors → `network`.

## What must NOT change

- The `ForgePort` capability pinned in V5REPOS-2.
- Per D3, only `{default_branch, description, archived, visibility}` traverse the capability.
- Per the Secret redaction rule, the resolved token MUST NOT appear in any event emitted by the adapter.
- The target host is `codeberg.org`; custom Gitea instances are out of v1 scope for this adapter (they can be added via per-remote `provider: codeberg` override later, but the host-pinning decision here is `codeberg.org`).

## Acceptance criteria

1. With `$HF_TEST_CODEBERG_ORG`, `$HF_TEST_CODEBERG_REPO`, and `$HF_TEST_CODEBERG_TOKEN` set, the read-metadata capability returns a value whose top-level keys are exactly `default_branch`, `description`, `archived`, `visibility`.
2. `visibility` on a public Codeberg repo serializes as the literal string `"public"`; on a private repo as `"private"`.
3. The write capability applied with `{description: "hyperforge-v5-repos-10 <timestamp>"}` succeeds; a subsequent read reflects the new value. The test MUST restore the original description before exit.
4. Calling the read capability with the token env var unset produces an `auth` error event.
5. Calling the read capability against a repo that does not exist produces `not_found`.
6. No event emitted by the adapter contains the literal value of `$HF_TEST_CODEBERG_TOKEN`.
7. Test script exits 0 with a `SKIP:` line when any of the three required env vars is unset.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-10.sh` → exit 0.
- Status flips in-commit with the implementation.
