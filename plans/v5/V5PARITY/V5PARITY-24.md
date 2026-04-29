---
id: V5PARITY-24
title: "SHARED-CRED — provider-default credential fallback"
status: Complete
type: implementation
blocked_by: []
unlocks: []
---

## Problem

The same `gh` token covers every github org a single user belongs to. v5 today requires a separate `secrets://github/<org>/token` entry per org, even when they all resolve to the same physical credential. For a user in 5 orgs, that's 5 identical token entries — duplicated state, multiplied rotation cost.

## Required behavior

**Provider-default credentials.** A new well-known secret path: `secrets://<provider>/_default/token` (e.g. `secrets://github/_default/token`). When `repos.import` / `repos.sync` / any `ForgePort` call resolves a credential for org `X` under provider `P`:

1. Check the org's explicit `CredentialEntry { type: token }`.
2. If absent OR resolves to a missing secret, fall back to `secrets://<P>/_default/token`.
3. If still absent, emit the existing `auth_required` error.

**The fallback applies only when the org has no explicit credential.** An org with its own `CredentialEntry` (even one pointing at a different secret) keeps using that — no surprise overrides.

**`orgs.bootstrap` (V5PARITY-21) gains a `--use_default_token bool` flag.** When set, the org's `CredentialEntry` references `secrets://<provider>/_default/token` instead of `secrets://<provider>/<org>/token`.

**Visible in `orgs.get`.** The detail event includes `effective_credential_source: "explicit" | "provider_default" | "none"` so consumers can see which path resolved.

## What must NOT change

- `CredentialEntry` schema — same `{key, type}` shape; the `_default` form is a path convention, not a new type.
- Resolution order is strict: explicit before default, never the reverse.
- D9 — the `_default` secret is just another stored secret; no special encryption or visibility rules.

## Acceptance criteria

1. With `secrets://github/_default/token` set and `orgs/foo.yaml` carrying no credentials, `repos.import --org foo --forge github` succeeds using the default token.
2. With both a default AND an org-specific token set, `orgs.get --org foo` reports `effective_credential_source: "explicit"`; the default is unused.
3. With only a default set and `orgs.get --org foo --provider github` on an org without credentials: `effective_credential_source: "provider_default"`.
4. With nothing set: existing `auth_required` error (unchanged).
5. `orgs.bootstrap … --use_default_token true` writes the org yaml referencing `secrets://github/_default/token`, not the per-org path.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-24.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
