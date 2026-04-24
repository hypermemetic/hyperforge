---
id: V5ORGS-3
title: "orgs.get — OrgDetail for one org, never leaks secret values"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users need one org's full shape — provider, credential keys/types, and
repo list — without hand-reading the yaml. The return type must obey the
Secret redaction rule: never a resolved plaintext value.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | must match an existing `orgs/<OrgName>.yaml` |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `OrgDetail` event | `credentials` is a list of `CredentialEntry` (key + type only) |
| not found | typed error event | names the `OrgName` that was requested |

Edge cases:

- `org` absent from disk: typed not-found error; no `OrgDetail` event emitted.
- `org` present with an empty `credentials` list: `OrgDetail.credentials` is the empty list, not missing.
- `org` present with one or more credentials: each `CredentialEntry` in the result carries only `key` (a `SecretRef` or `FsPath`) and `type` (`CredentialType`). No resolved value ever appears.

## What must NOT change

- v4's `orgs_get`-equivalent behavior on the v4 daemon.
- Secret redaction rule from CONTRACTS §types: even if `secrets.yaml` holds a value for the credential's `key`, that value MUST NOT appear anywhere in the `orgs.get` event stream.

## Acceptance criteria

1. Against `minimal_org`, `orgs.get org=demo` emits exactly one `OrgDetail` event where `name == "demo"`, `provider == "github"`, `credentials == []`, `repos == []`.
2. Against `org_with_credentials`, `orgs.get org=demo` returns an `OrgDetail` whose `credentials` contains exactly one `CredentialEntry` with the fixture's `key` and `type == "token"`.
3. With `hf_put_secret secrets://gh-token ghp_leak_me_please` seeded before the call, no event from `orgs.get` contains the literal string `ghp_leak_me_please`.
4. `orgs.get org=nonexistent` against any fixture emits an error event whose message references `nonexistent`; no `OrgDetail` event is emitted.
5. `orgs.get` without the `org` parameter emits a typed error event (missing required parameter).

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-3.sh` → exit 0.
- Status flips in-commit with the implementation.
