---
id: V5PROV-4
title: "Codeberg adapter: implement create_repo, delete_repo, repo_exists"
status: Ready
type: implementation
blocked_by: [V5PROV-2]
unlocks: [V5PROV-9]
---

## Problem

Codeberg (Gitea-compatible) adapter (V5REPOS-10) implements metadata
read/write only. This ticket adds the three lifecycle methods from
V5PROV-2 against the Gitea REST API.

## Required behavior

| Method | HTTP | Endpoint |
|---|---|---|
| create_repo | POST | `/orgs/{org}/repos` **or** `/user/repos` when the org is the authenticated user |
| delete_repo | DELETE | `/repos/{org}/{name}` |
| repo_exists | GET | `/repos/{org}/{name}` → 200 = exists, 404 = absent |

Visibility mapping: `public` → `{private: false}`, `private` →
`{private: true}`. `internal` is Gitea-extension territory — unsupported
in v1; return `ForgePortError { class: unsupported_visibility }`.

## What must NOT change

- V5REPOS-10's existing read/write behavior.
- Gitea-specific field exposure beyond the four DriftFieldKind members.

## Acceptance criteria

Mirror V5PROV-3's six criteria against the Codeberg API. Any script
that verifies tier-2 behavior SKIPs cleanly if the tier-2 config dir
lacks Codeberg credentials.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-4.sh` → exit 0 (tier 2).
- Status flips in-commit.
