---
id: V5CORE-4
title: "Embedded secret store with SecretRef resolver capability"
status: Complete
type: implementation
blocked_by: [V5CORE-2]
unlocks: [V5CORE-10, V5REPOS-1]
---

## Problem

Org YAML holds `CredentialEntry.key` as a `SecretRef` of the form
`secrets://<path>`. No component can turn a `SecretRef` into a plaintext
secret. Downstream ForgePort adapters (V5REPOS) need a stable capability
to resolve references without learning YAML details.

## Required behavior

Introduce a named capability: **`SecretResolver`**. Its public surface is
one fallible resolution operation:

| Input | Type | Required | Notes |
|---|---|---|---|
| reference | `SecretRef` | yes | must match `secrets://<path>` (§types) |

| Output / Event | Shape | Notes |
|---|---|---|
| Success | resolved plaintext `String` | never logged, never returned across the wire as part of any response type |
| Not found | typed "not found" error | references the offending `SecretRef` string |
| Malformed ref | typed "invalid ref" error | rejects anything not matching the `SecretRef` constraint |

v1 backend: a YAML file at `$HF_CONFIG/secrets.yaml`. Top-level shape is a
mapping where keys are the `<path>` portion of `SecretRef` and values are
strings. Missing file = empty store = all lookups return not-found.

The capability is the **contract**; the YAML backend is one implementation
of it. Other backends (OS keyring, remote KMS) are post-v5 and must slot
in without touching adapter callers.

Edge cases:

- `secrets.yaml` missing or empty: every resolution is not-found, never an error.
- `secrets.yaml` exists but is not valid YAML: hard error on first resolution attempt; message names the file.
- Non-string value under a key: hard error naming the key.
- `SecretRef` not matching `secrets://<path>` shape: rejected before any file I/O.

## What must NOT change

- Secret redaction rule from CONTRACTS §types: no method whose return
  type contains `CredentialEntry` may include resolved values.
- v4's `hyperforge-auth` sidecar. It continues to exist; v5 does not
  consume or replace it.

## Acceptance criteria

1. Writing a key `gh-token` → value `ghp_abc` via `hf_put_secret secrets://gh-token ghp_abc`, then resolving `secrets://gh-token`, yields `ghp_abc`.
2. Resolving `secrets://missing-key` against a populated file yields the typed not-found error (observable as an error event naming `secrets://missing-key`).
3. With no `secrets.yaml` file at all, resolving any valid `SecretRef` yields not-found (not an I/O error).
4. Resolving `not-a-secret-ref` (missing the `secrets://` prefix) yields the typed invalid-ref error.
5. A corrupted `secrets.yaml` produces an error event whose message names `secrets.yaml`.
6. No response event across the entire daemon surface includes the literal plaintext `ghp_abc` outside of the resolver's own success return.

## Completion

- Run `bash tests/v5/V5CORE/V5CORE-4.sh` → exit 0.
- Status flips in-commit with the implementation.
