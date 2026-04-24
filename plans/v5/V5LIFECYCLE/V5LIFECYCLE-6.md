---
id: V5LIFECYCLE-6
title: "repos.delete — soft semantics per D12 (privatize on remotes, mark dismissed)"
status: Complete
type: implementation
blocked_by: [V5LIFECYCLE-5]
unlocks: [V5LIFECYCLE-11]
---

## Problem

Per D12, `repos.delete` is a **soft-delete**: privatize on remote forges, mark `lifecycle: dismissed`, keep the record in the org yaml. Protected repos refuse. The reverted V5PROV-7 (hard-delete-cascade) is gone; this ticket is the replacement.

## Required behavior

Method signature:

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | |
| `name` | `RepoName` | yes | |
| `dry_run` | `bool` | no (default false) | D7 |

Execution order (with `dry_run: false`):

1. Load org yaml via `ops::state::load_all`.
2. Find the repo; if absent → `error { code: not_found }`.
3. If `protected == true` → `error { code: protected, message: "repo '<name>' is protected; toggle via repos.protect first" }`.
4. If `lifecycle == dismissed` → **idempotent success**: no forge call, emit `repo_dismissed` with `already: true` and return (the record is already in the soft-delete state).
5. For each provider in the set of `repo.remotes`' derived providers (via `ops::repo::derive_provider`):
   - Call `ops::repo::privatize_on_forge` — which in turn calls `adapter.update_repo(visibility: Private)`.
   - On success: add provider to `privatized` set, emit `forge_privatized { provider, url }`.
   - On error: emit `privatize_error { provider, error_class, message }`; continue to the next provider.
6. Call `ops::repo::dismiss(&mut repo, privatized)`.
7. Call `ops::state::save_org`.
8. Emit `repo_dismissed { ref, privatized_on: Set<ProviderKind>, already: false }`.

On `dry_run: true`: same event stream up to step 4, but skip the actual forge calls (emit simulated `forge_privatized` events with `dry_run: true` flag in payload) and skip save_org. Filesystem byte-identical.

New events:

| Event | Payload |
|---|---|
| `forge_privatized` | `ref`, `provider`, `url`, `dry_run: bool` |
| `privatize_error` | `ref`, `provider`, `error_class`, `message` |
| `repo_dismissed` | `ref`, `privatized_on: Set<ProviderKind>`, `already: bool` |

## What must NOT change

- V5REPOS-6's `repos.remove` method stays as-is (hard local-only removal) for backward compatibility with scripts that used it; its semantics are distinct from `repos.delete`.
- `repos.add --create_remote` path (V5PROV-6) unchanged.
- D13 — all forge calls go through `ops::repo::privatize_on_forge`; this ticket adds that helper to the ops layer.

## Acceptance criteria

1. Soft-delete a single-forge repo: emit one `forge_privatized`, then `repo_dismissed`. Org yaml shows `lifecycle: dismissed` + `privatized_on: [github]`. On the forge, the repo now has `visibility: private` (verifiable via `gh repo view`).
2. Soft-delete a multi-forge repo where one provider fails (e.g., blank cred): emit `forge_privatized` for the success, `privatize_error` for the failure, `repo_dismissed` with `privatized_on` reflecting only the successes. Local record is still marked dismissed.
3. Soft-delete a protected repo: emits `error { code: protected }`; no forge calls, yaml byte-identical.
4. Soft-delete an already-dismissed repo: emits `repo_dismissed { already: true }` without calling any forge; yaml byte-identical.
5. `dry_run: true` emits the same event stream but neither forge nor yaml are mutated.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-6.sh` → exit 0 (tier 2: requires HF_V5_TEST_CONFIG_DIR).
- Status flips in-commit.
