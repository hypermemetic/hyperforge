---
id: V5PROV-6
title: "repos.add: add create_remote + visibility + description params"
status: Complete
type: implementation
blocked_by: [V5PROV-2, V5PROV-3]
unlocks: [V5PROV-9]
---

## Problem

`repos.add` (V5REPOS-5) registers the repo in the org YAML but never
touches the forge. Per D11, callers need a single command that both
registers AND creates on the forge with visibility control.

## Required behavior

`repos.add` gains three new optional parameters. Existing params
unchanged.

| New param | Type | Default | Notes |
|---|---|---|---|
| `create_remote` | `bool` | `false` | When true, call `adapter.create_repo` after the local entry is written |
| `visibility` | `ProviderVisibility` | `private` | Passed to `create_repo`. Ignored when `create_remote` is false |
| `description` | `String` | `""` | Passed to `create_repo`. Ignored when `create_remote` is false |

Execution order (pinned by R2 resolution in V5PROV-1):
1. Validate inputs (all V5REPOS-5 validations remain).
2. Write the local org YAML entry (atomic per D8).
3. If `create_remote: true`:
   a. Call `repo_exists` on the adapter for the first remote.
   b. If already exists → emit `error` with `code: conflict, message: "repo already exists on remote"`; local entry IS rolled back (unregistered); caller sees no mutation.
   c. If absent → call `create_repo`; on success emit `repo_created` event; on adapter error → roll back local entry, emit `error` with the adapter's class.
4. Emit the usual `repo_added` success event whether `create_remote` was true or false.

Events (new):

| Event | Emitted when | Payload |
|---|---|---|
| `repo_created` | After successful `create_repo` | `ref: RepoRef`, `url: RemoteUrl` (the first remote) |

## What must NOT change

- Existing V5REPOS-5 behavior when `create_remote` is false (backward compatible).
- `dry_run: true` still does no disk or forge writes — in dry-run, the `create_remote` flow emits the same event stream it would emit on success, without any actual API call.
- Secret redaction rule: the token resolving through to create_repo never surfaces in any event.

## Acceptance criteria

1. Without `create_remote`, behavior is identical to V5REPOS-5 (regression).
2. With `create_remote: true` and a non-existent remote repo name, the method creates the repo (verifiable via adapter.repo_exists returning true after) and emits `repo_created` followed by `repo_added`.
3. With `create_remote: true` on an already-existing remote name, the method rolls back the local entry (pre-call state restored on disk) and emits an `error` with `code: conflict`.
4. `dry_run: true` with `create_remote: true` emits the `repo_created`-then-`repo_added` event stream AND leaves both disk and forge byte-identical.
5. Adapter network failure during `create_repo` triggers local rollback; `repos.list --org X` afterward does NOT include the rolled-back repo.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-6.sh` → exit 0 (tier 2).
- Status flips in-commit.
