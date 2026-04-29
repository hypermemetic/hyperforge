---
id: V5PARITY-23
title: "AUTH-DISCOVERY — providerless requirements + external-token detect"
status: Complete
type: implementation
blocked_by: []
unlocks: [V5PARITY-20, V5PARITY-21]
---

## Problem

`auth_requirements` today takes an org and reports back what's missing. That's chicken-and-egg for first-time users: you can't know what scopes to request before you have an org configured. There's also no way to ask the daemon "what credentials do I have available externally" — e.g. is `gh` logged in, what scopes does the existing token cover, what orgs can it see.

## Required behavior

**Two new root methods:**

| Method | Behavior |
|---|---|
| `auth.requirements_for --provider <github\|codeberg\|gitlab>` | Returns the static scope/permission list this provider needs for full hyperforge functionality. Independent of org config. Emits `auth_requirements { provider, required_scopes, recommended_scopes, optional_scopes }`. |
| `auth.detect_external [--provider <p>]` | Probes for known external auth sources on the daemon host: `gh auth status`, `glab auth status`, `~/.netrc`. For each found, emits `external_auth_detected { provider, source, username, accessible_orgs?, scopes? }` (no token value — only metadata). With no `--provider` filter, probes all three. |

**Why root not under a child.** Auth discovery is a pre-org step — there's no org to attach it to yet. Living on the root keeps the activation tree shape: configuration before identity.

**`accessible_orgs`** — populated when the source can list orgs (gh exposes `gh api /user/orgs`); omitted otherwise. Token values are NEVER emitted.

## What must NOT change

- Existing `auth_check` and `auth_requirements` (per-org context) stay.
- D9 secret-redaction — `auth.detect_external` is metadata-only.
- v4-style auto-import flows (`gh` token reuse) are NOT introduced here; that's V5PARITY-21's `gh-token://` form. This ticket only EXPOSES the data.

## Acceptance criteria

1. `auth.requirements_for --provider github` emits `{required_scopes: ["repo", "read:org"], recommended_scopes: ["delete_repo"], optional_scopes: []}` (or whatever the v5 audit determines — list pinned at implementation time).
2. `auth.detect_external --provider github` on a host with `gh auth login` done emits `external_auth_detected { provider: "github", source: "gh-cli", username: "<u>", accessible_orgs: [...], scopes: [...] }`. No token value in the payload.
3. `auth.detect_external --provider github` on a host without `gh` emits zero events plus a final `done` (not an error — absence of auth is a fact, not a failure).
4. The daemon never invokes the gh CLI to *use* the token in this ticket — only `gh auth status` and `gh api /user/orgs`.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-23.sh` → exit 0 (tier 1 — uses a stub `gh` shim on PATH).
- Ready → Complete in-commit.
