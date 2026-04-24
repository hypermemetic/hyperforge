---
id: V5ORGS-7
title: "orgs.set_credential — add or replace one CredentialEntry by key"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users need to attach or rotate a single credential on an existing org
without rewriting the yaml wholesale. The method never accepts a secret
plaintext — only a `SecretRef` or `FsPath` — preserving the invariant
that org yaml contains references, not values.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | must match an existing `orgs/<OrgName>.yaml` |
| `key` | `SecretRef` \| `FsPath` | yes | matches the `CredentialEntry.key` contract (§types) |
| `credential_type` | `CredentialType` | yes | closed set: `token`, `ssh_key` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one event identifying the affected `org` and the resulting `CredentialEntry` | distinguishes "added" vs "replaced" via a field caller can assert on |
| not found | typed error event | names the `OrgName` |
| invalid key | typed error event | rejects a `key` matching neither `SecretRef` nor `FsPath` |
| unknown type | typed error event | rejects an unknown `CredentialType` variant |

Behavior:

- If no existing entry has the same `key`, append a new `CredentialEntry` at the end of `credentials[]`.
- If an existing entry has the same `key`, replace that one entry in place; its index in `credentials[]` is preserved.
- Every other field of the org yaml (`name`, `provider`, `repos[]`, credential entries with different keys) is byte-equivalent after round-trip through the V5CORE-3 loader.

Write obeys D8 (atomic).

Post-condition on `dry_run: true`: same event as `dry_run: false` with the
corresponding "added"/"replaced" label, but `orgs/<org>.yaml` is
byte-identical to pre-call content.

Edge cases:

- `key` is a plaintext secret (e.g. `ghp_abc`) and not a `SecretRef` or `FsPath`: typed error; file untouched. This is the primary safeguard against accidental secret-in-yaml.
- `org` absent: typed not-found error; no file written.

## What must NOT change

- `orgs/<org>.yaml` never contains a resolved secret value; only `SecretRef` or `FsPath`.
- `provider`, `repos[]`, and every credential entry whose `key` differs from the input `key`.
- Other org files, workspace files, and `secrets.yaml` are untouched.

## Acceptance criteria

1. Against `minimal_org`, `orgs.set_credential org=demo key=secrets://gh-token credential_type=token` emits a success event identifying the add; `orgs.get org=demo` then reports a single `CredentialEntry` with `key == "secrets://gh-token"` and `type == "token"`; `provider` remains `github`.
2. After (1), `orgs.set_credential org=demo key=secrets://gh-token credential_type=token` a second time emits a success event identifying a replace (not an add); `orgs.get org=demo` still reports exactly one `CredentialEntry` at the same index.
3. Against `org_with_credentials`, setting a credential with a new `key` appends; `orgs.get org=demo` reports the pre-existing entry first (same index) followed by the new one.
4. Against `minimal_org`, `orgs.set_credential org=demo key=secrets://gh-token credential_type=token dry_run=true` emits the same shape event but `orgs.get org=demo` on a fresh daemon still reports zero credentials.
5. `orgs.set_credential org=demo key=ghp_leaky_plaintext credential_type=token` emits a typed error naming the invalid key; `orgs/demo.yaml` is byte-identical.
6. `orgs.set_credential org=nonexistent key=secrets://x credential_type=token` emits a typed not-found error.
7. After a real (non-dry-run) call, the contents of `orgs/<org>.yaml` never contain the literal plaintext of any value seeded via `hf_put_secret` at that ref.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-7.sh` → exit 0.
- Status flips in-commit with the implementation.
