---
id: V5PARITY-33
title: "GH-PRIVATE-REPOS — list private repos when authed user owns the target"
status: Complete
type: bugfix
blocked_by: []
unlocks: []
---

## Problem

`repos.import --org <user> --forge github` against a personal GitHub account currently returns only that user's *public* repos. Private repos owned by the same user — even when the configured token has `repo` scope — are silently missing.

Reproduces with: an account that has both public and private repos; `gh auth login` with `repo` scope; `orgs.bootstrap` + `repos.import`. The import_summary reports the public-count, not the total.

## Root cause

`src/v5/adapters/github.rs` `list_repos`:
1. Tries `GET /orgs/{name}/repos?per_page=100`.
2. On 404 (the target is a user, not an org), falls back to `GET /users/{name}/repos?per_page=100`.

`/users/{name}/repos` is the **public** users endpoint — it returns only public repos regardless of authentication. To get private repos for the authenticated user, the call must be `GET /user/repos?affiliation=owner&per_page=100` (the **user-context** endpoint, no `s`).

## Required behavior

When the org endpoint 404s, attempt the user-context endpoint first:

1. `GET /user/repos?affiliation=owner&per_page=100`
2. Filter results by `owner.login == <name>` (so listing a different user's account doesn't accidentally surface the authed user's repos).
3. If non-empty, use those (private + public).
4. If empty (target ≠ authed user, or unauthenticated), fall back to `GET /users/{name}/repos?per_page=100` for the public-only set.

Pagination via the Link header continues to work for both paths.

## What must NOT change

- Org-account flow: `/orgs/{name}/repos` works as before; no extra calls.
- `RemoteRepo` shape on the wire.
- D9: token values remain out of every event payload.

## Acceptance criteria

1. `repos.import --org <authed-user> --forge github` against an account with public + private repos returns the union (matching `gh api /user/repos?affiliation=owner --jq 'length'`).
2. `repos.import --org <other-user> --forge github` returns the public-only set (current behavior preserved for non-self targets).
3. `repos.import --org <real-org> --forge github` is byte-identical to current behavior — no extra API calls.
4. The existing V5PARITY-2 test stays green.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-33.sh` → exit 0 (tier 2 — needs a real github account with private repos; tier 1 covers the request shape via httpmock).
- Ready → Complete in-commit.
