---
id: V5PROV-8
title: "workspaces.sync: detect remote-only members, call create_repo"
status: Complete
type: implementation
blocked_by: [V5PROV-2, V5PROV-3]
unlocks: [V5PROV-9]
---

## Problem

v4's `workspace sync` is an 8-phase pipeline whose "apply creates"
phase calls adapter.create_repo for members registered locally but
absent on the forge. V5WS-9 implements only the read-side (`repos.sync`
orchestration). This ticket adds the create phase.

## Required behavior

For each workspace member, before calling `repos.sync`:
1. Call `adapter.repo_exists(ref, first_remote)`.
   - `Ok(true)` → proceed to the existing sync flow unchanged.
   - `Ok(false)` → member is remote-only-absent. Call `adapter.create_repo` with the member's declared visibility + description (from the org YAML's `repos[].metadata`, falling back to `visibility: private, description: ""`).
     - Success: emit `sync_diff { status: created, ref, url }`, increment a new `created` counter; continue to sync to capture post-create metadata.
     - Error: emit `sync_diff { status: errored, error_class: adapter_error }`, increment `errored`, skip sync for this member. Do NOT abort the batch (D6).
   - `Err(...)` → treat as errored (same as adapter read failure).

New `WorkspaceSyncReport` field: `created: u32`. The invariant becomes
`total == in_sync + drifted + errored + created`.

New `SyncStatus` variant: `created`.

## What must NOT change

- V5WS-9's serial execution order.
- D6 partial-failure tolerance.
- Read-only-ness against filesystem and workspace yaml — this ticket only adds forge writes through adapter.create_repo, never local writes.

## Acceptance criteria

1. Workspace with a member whose org yaml lists it but whose remote doesn't exist: `workspaces.sync` creates the remote, emits `sync_diff { status: created }`, and the final report shows `created: 1, total: 1`.
2. Re-running `workspaces.sync` on the same workspace (no other changes) is idempotent: the member now exists on the remote, so `repo_exists` returns true; the sync_diff for that member shows `status: in_sync` (not `created` again).
3. Workspace with two members, one remote-only-absent and one existing: both sync_diff events emit; report shows `created: 1, in_sync: 1, total: 2`.
4. If `create_repo` fails (e.g., blank token), the per-member event shows `status: errored, error_class: auth`; report shows `errored: 1`; RPC still exits success.
5. Read-only invariant from V5WS-9 AC4 holds for existing members: org yaml and workspace yaml and the workspace `path` filesystem are byte-identical before/after — the only observable side effect is on the forge.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-8.sh` → exit 0 (tier 2).
- Status flips in-commit.
