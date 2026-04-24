---
id: V5PROV-5
title: "GitLab adapter: implement create_repo, delete_repo, repo_exists"
status: Complete
type: implementation
blocked_by: [V5PROV-2]
unlocks: [V5PROV-9]
---

## Problem

The GitLab adapter (V5REPOS-11) implements metadata read/write only.
This ticket adds the three lifecycle methods from V5PROV-2 against the
GitLab REST API.

## Required behavior

| Method | HTTP | Endpoint |
|---|---|---|
| create_repo | POST | `/projects` with `namespace_id` resolved from the org name |
| delete_repo | DELETE | `/projects/{url-encoded org/name}` |
| repo_exists | GET | `/projects/{url-encoded org/name}` → 200 exists, 404 absent |

Visibility mapping: `public` → `visibility: "public"`, `private` →
`visibility: "private"`, `internal` → `visibility: "internal"` (this
is the one variant where `internal` is supported; the other two
adapters reject it).

GitLab project paths use org/name with `/` URL-encoded as `%2F`.

## What must NOT change

- V5REPOS-11's existing read/write behavior and custom-host handling.

## Acceptance criteria

Mirror V5PROV-3's six criteria against the GitLab API, with one
addition: `create_repo` with `visibility: internal` SUCCEEDS on
GitLab (the inverse of V5PROV-3's unsupported_visibility check).

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-5.sh` → exit 0 (tier 2).
- Status flips in-commit.
