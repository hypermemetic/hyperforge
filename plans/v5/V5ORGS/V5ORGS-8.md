---
id: V5ORGS-8
title: "orgs.remove_credential — remove one CredentialEntry by key"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users need to remove one credential from an org by `key`, preserving all
other fields and all other credential entries. The paired inverse of
V5ORGS-7.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | must match an existing `orgs/<OrgName>.yaml` |
| `key` | `SecretRef` \| `FsPath` | yes | must match a `CredentialEntry.key` currently in the org |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one event identifying the affected `org` and the removed `key` | |
| not found (org) | typed error event | names the `OrgName` |
| not found (key) | typed error event | names the missing `key`; distinguishable from the org-not-found error |

Behavior:

- Removes exactly the `CredentialEntry` whose `key` equals the input. Order of remaining entries is preserved.
- Every other field of the org yaml (`name`, `provider`, `repos[]`) is byte-equivalent after round-trip through the V5CORE-3 loader.
- Write obeys D8 (atomic).

Post-condition on `dry_run: true`: same event, but `orgs/<org>.yaml` is
byte-identical to pre-call content.

Edge cases:

- `key` not present in the org's `credentials[]`: typed not-found error; file untouched.
- `org` absent: typed not-found error; no file written. Distinguishable from key-not-found.
- Org has exactly one matching credential: after removal `credentials[]` is the empty list.

## What must NOT change

- `provider`, `repos[]`, and every credential entry whose `key` differs from the input `key`.
- Other org files, workspace files, and `secrets.yaml`.
- The `secrets.yaml` entry at the removed `key`'s path: this method does NOT delete the resolved value in the secret store. That is a user action.

## Acceptance criteria

1. Against `org_with_credentials`, `orgs.remove_credential org=demo key=secrets://gh-token` emits a success event; `orgs.get org=demo` then reports `credentials == []`; `provider` and `repos` are unchanged.
2. After (1), a fresh daemon on the same `$HF_CONFIG` confirms `credentials == []` via `orgs.get`.
3. Against `org_with_credentials`, `orgs.remove_credential org=demo key=secrets://gh-token dry_run=true` emits the same shape event but `orgs.get org=demo` on a fresh daemon still reports the pre-existing `CredentialEntry`.
4. Against `org_with_credentials`, `orgs.remove_credential org=demo key=secrets://nonexistent` emits a typed key-not-found error; `orgs/demo.yaml` is byte-identical.
5. Against `minimal_org`, `orgs.remove_credential org=nonexistent key=secrets://x` emits a typed org-not-found error, distinguishable from the key-not-found error in (4).
6. After a real removal, `secrets.yaml` content is byte-identical to pre-call content (the secret value is not deleted from the store).

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-8.sh` → exit 0.
- Status flips in-commit with the implementation.
