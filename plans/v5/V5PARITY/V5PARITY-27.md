---
id: V5PARITY-27
title: "EXTERNAL-AUTH-OPS — typed wrapper over forge-CLI auth subprocesses"
status: Complete
type: implementation
blocked_by: []
unlocks: [V5PARITY-21, V5PARITY-23]
---

## Problem

V5PARITY-21 (`orgs.bootstrap` with `gh-token://` resolution) and V5PARITY-23 (`auth.detect_external` over `gh auth status` + `gh api /user/orgs`) both need to spawn the `gh` CLI as a subprocess. Without a shared module:

1. Each ticket would invent its own `Command::new("gh")` callsites.
2. V5LIFECYCLE-11's DRY grep set would gain a third subprocess source uncovered by any invariant.
3. Future glab/codeberg-cli additions would compound the duplication.

V5PARITY-15 solved exactly this shape for git: `ops::git` is the single typed entry point, with subprocess vs git2 backends behind it. This ticket does the same for forge-CLI auth.

## Required behavior

**New module: `src/v5/ops/external_auth/`** (sibling of `ops::git`):

| Function | Backend | Role |
|---|---|---|
| `detect_status(provider)` | subprocess `gh auth status` (or glab equivalent) | Returns `ExternalAuthStatus { logged_in: bool, username?, host?, scopes: Vec<String> }`. `None` when the CLI isn't installed (not an error). |
| `read_token(provider)` | subprocess `gh auth token` | Returns the token string. Token contents NEVER logged or echoed. |
| `list_accessible_orgs(provider)` | subprocess `gh api /user/orgs` | Returns `Vec<String>` org names. Errors map to a typed `ExternalAuthError`. |

**`ExternalAuthError`** — closed enum: `CliNotFound { provider } | NotLoggedIn { provider } | Network(String) | InvalidResponse(String) | Io(String)`.

**Provider dispatch** is a closed enum (`ExternalAuthProvider::{Github, Codeberg, Gitlab}`); v1 implements only `Github` (others return `CliNotFound` or unimplemented). The shape supports adding `glab`/`berg` later without changing the public API.

**D13 invariant extension.** Add a new DRY invariant to V5LIFECYCLE-11: `Command::new("gh")` lives only under `src/v5/ops/external_auth/`. Same shape as the existing `command-git` grep.

**Token redaction.** `ExternalAuthStatus` carries `scopes` and metadata, not tokens. `read_token` is the only function returning a token; callers (V5PARITY-21) consume it immediately and store it via `secrets.set` — no logging, no event payload, no Display impl that exposes it.

## What must NOT change

- D13 — adding a second subprocess source is fine as long as it's contained behind a typed module with a DRY grep.
- D9 — token values stay out of every event payload.
- The existing `ops::git` module is untouched; this is a parallel tree, not a refactor of git.

## Acceptance criteria

1. `ops::external_auth::detect_status(Github)` on a host with `gh auth login` done returns `Some(ExternalAuthStatus { logged_in: true, username: Some(_), scopes: [...], ... })`.
2. Same call on a host with no `gh` binary returns `None` (not an error).
3. `read_token(Github)` returns a non-empty string when logged in; `Err(NotLoggedIn)` when not.
4. `list_accessible_orgs(Github)` returns the user's accessible orgs (matches `gh api /user/orgs --jq '.[].login'`).
5. V5LIFECYCLE-11's DRY checkpoint includes a new `command-gh` invariant; running it returns green with `Command::new("gh")` only under `ops/external_auth/`.
6. The new module exports nothing that emits a token in its `Debug` or `Display` impl.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-27.sh` → exit 0 (tier 1 — uses a stub `gh` binary on PATH so tests don't depend on the host's auth state).
- Ready → Complete in-commit.
