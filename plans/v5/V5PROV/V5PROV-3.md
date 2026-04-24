---
id: V5PROV-3
title: "GitHub adapter: implement create_repo, delete_repo, repo_exists"
status: Ready
type: implementation
blocked_by: [V5PROV-2]
unlocks: [V5PROV-6, V5PROV-7, V5PROV-8, V5PROV-9]
---

## Problem

The GitHub adapter (V5REPOS-9) implements metadata read/write only.
V5PROV-2 pinned three lifecycle methods on the trait; this ticket
implements them against the GitHub REST API.

## Required behavior

The GitHub adapter implements all three methods defined in V5PROV-2
with these endpoint mappings (v4's adapter is the reference implementation):

| Method | HTTP | Endpoint |
|---|---|---|
| create_repo | POST | `/orgs/{org}/repos` **or** `/user/repos` when the org is actually a user account (adapter must distinguish) |
| delete_repo | DELETE | `/repos/{org}/{name}` |
| repo_exists | GET | `/repos/{org}/{name}` → 200 = exists, 404 = absent |

Visibility mapping: `public` → `{private: false, visibility: "public"}`,
`private` → `{private: true}`. `internal` is not supported on
`github.com` → return `ForgePortError { class: unsupported_visibility }`
without making the API call.

## What must NOT change

- V5REPOS-9's existing read/write behavior, return shape, and error classes.
- The `ForgeAuth` resolver flow — credentials are still fetched per call.
- The CONTRACTS §types `Remote { url, provider? }` wire shape.

## Acceptance criteria

1. `create_repo` on a new name creates the repo on github.com (verified via `gh repo view` or a GitHub API GET from the test).
2. `repo_exists` returns `Ok(true)` for the created repo and `Ok(false)` for a known-missing name, using the same credentials.
3. `delete_repo` removes the repo (verified by `repo_exists` returning `Ok(false)` afterwards).
4. `create_repo` with `visibility: internal` fails with `unsupported_visibility` without issuing the API call (the test counts HTTP requests or asserts latency < typical API call).
5. `create_repo` of an already-existing name returns `ForgePortError { class: conflict }`.
6. With a blank token, all three methods return `ForgePortError { class: auth }`.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-3.sh` → exit 0 (tier 2 — requires `HF_V5_TEST_CONFIG_DIR` with github tokens).
- Status flips in-commit.
