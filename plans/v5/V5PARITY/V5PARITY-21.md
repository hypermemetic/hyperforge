---
id: V5PARITY-21
title: "ORG-BOOTSTRAP — one-shot `orgs.bootstrap` RPC"
status: Pending
type: implementation
blocked_by: []
unlocks: [V5PARITY-20, V5PARITY-22]
---

## Problem

Adding an org currently takes four RPCs in strict order: `secrets.set` → `orgs.create` → `orgs.set_credential` → `repos.import`. The parameter naming disagrees across them (`--name` vs `--org`, `--credential_type` vs `--type`-on-the-wire). New users trip on this; scripts have to be careful. The atomic intent — "track this org with this token" — is one operation; the four-RPC sequence is an artifact.

## Required behavior

**`orgs.bootstrap`** — single RPC that composes the existing surface:

| Param | Behavior |
|---|---|
| `--name <org>` | required |
| `--provider <github\|codeberg\|gitlab>` | required |
| `--token <value-or-special>` | required for token-auth providers; accepts a literal string OR a special form (see below) |
| `--secret_key <secrets://...>` | optional override; defaults to `secrets://<provider>/<org>/token` |
| `--import bool` | default `true`; runs `repos.import` after credential is wired |
| `--dry_run bool` | preview-only |

**Special token forms** (resolved server-side, never echoed in events):
- `gh-token://` — read from `gh auth token` subprocess
- `env://VAR` — read from `VAR` env var on the daemon
- raw string — store directly

Emits one event per stage (`secret_set`, `org_created`, `credential_added`, `import_summary`) plus a final `bootstrap_done { org, provider, repos_added }` aggregate. On failure at any stage, emits the partial events plus `bootstrap_failed { stage, message }` — caller can inspect what landed.

**Idempotency.** Re-running with the same `--name`/`--provider` updates the credential and re-runs import; doesn't error on existing org.

## What must NOT change

- The four underlying RPCs (`secrets.set`, `orgs.create`, `orgs.set_credential`, `repos.import`) stay; `orgs.bootstrap` is composition, not replacement.
- Wire format for the per-stage events stays identical to the standalone calls.
- D9 secret-redaction — token values never appear in events.

## Acceptance criteria

1. `orgs.bootstrap --name foo --provider github --token <raw>` results in the same on-disk state as the four-RPC sequence (orgs/foo.yaml + secrets.yaml entry + import-populated repos).
2. `orgs.bootstrap --name foo --provider github --token gh-token://` reads from `gh auth token` and proceeds. Without `gh` installed, emits `bootstrap_failed { stage: "token_resolve" }`.
3. `orgs.bootstrap … --import false` skips the import and ends after `credential_added`.
4. Re-running on an existing org updates the credential and re-imports without error.
5. `--dry_run true` emits all events with `dry_run: true` markers and writes nothing.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-21.sh` → exit 0 (tier 1 — uses raw-string token; tier 2 stage covers gh-token://).
- Ready → Complete in-commit.
