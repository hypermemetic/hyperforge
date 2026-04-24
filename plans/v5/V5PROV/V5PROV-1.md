---
id: V5PROV-1
title: "v5 Provisioning — match v4's workspace→repo→remote flow"
status: Epic
type: epic
blocked_by: []
unlocks: []
---

## Goal

Close the v4 → v5 feature gap for provisioning: the user can create a
workspace, register a new repo under an org, and run `workspaces.sync`
to materialize it on the forge — all through synapse RPC, no manual
`gh repo create` or hand-editing.

When done, the workflow is:

```bash
synapse … hyperforge workspaces create name=dev path=/tmp/dev
synapse … hyperforge repos add --org hypermemetic --name demo \
    --remotes '[...]' --create_remote true --visibility private \
    --description "whatever"
synapse … hyperforge workspaces add_repo --name dev --ref hypermemetic/demo
synapse … hyperforge workspaces sync --name dev
# → remote repo exists on github.com/hypermemetic/demo; workspace_sync_report
#   shows in_sync:1
```

`workspaces.sync` also picks up members registered locally but missing
on the forge (e.g., after `repos.add` without `--create_remote`) and
creates them automatically — matching v4's 8-phase pipeline's
"apply creates" behavior.

## Dependency DAG

```
V5PROV-2 (ForgePort trait extension — pins create_repo/delete_repo/repo_exists)
    │
    ├─ V5PROV-3 (GitHub adapter impl)
    ├─ V5PROV-4 (Codeberg adapter impl)
    └─ V5PROV-5 (GitLab adapter impl)
    │     (three parallel)
    │
    ├─ V5PROV-6 (repos.add --create_remote + visibility + description)
    ├─ V5PROV-7 (repos.delete --delete_remote)
    └─ V5PROV-8 (workspaces.sync creates remote-only members)
    │     (three parallel; all depend on ≥1 adapter landed)
    │
V5PROV-9 (end-to-end checkpoint — tier-2 verified against live forge)
```

## Tickets

| ID | Status | Summary |
|----|--------|---------|
| V5PROV-2 | Pending | `ForgePort` trait: add `create_repo`, `delete_repo`, `repo_exists` (D10) |
| V5PROV-3 | Pending | GitHub adapter: implement the three lifecycle methods |
| V5PROV-4 | Pending | Codeberg adapter: implement the three lifecycle methods |
| V5PROV-5 | Pending | GitLab adapter: implement the three lifecycle methods |
| V5PROV-6 | Pending | `repos.add`: add `create_remote`, `visibility`, `description` params (D11) |
| V5PROV-7 | Pending | `repos.delete`: extend `delete_remote` to call adapter.delete_repo (D11) |
| V5PROV-8 | Pending | `workspaces.sync`: detect remote-only members, call create_repo (D11) |
| V5PROV-9 | Pending | Checkpoint: end-to-end tier-2 verification on real forge |

## Out of scope (reserved for V5PARITY)

- `repos.rename` / `repos.set_default_branch` / `repos.set_archived` individual methods (the trait adds the underlying capability via `update_repo` / existing push; user-level methods are V5PARITY)
- `workspaces.discover` (filesystem scan → workspace yaml bootstrap) — separate epic
- `repos.clone` / `repos.status` / `repos.init` (per-repo `.hyperforge/config.toml`) — V5PARITY
- Incremental repo listing with ETag caching — V5PARITY
- Root methods (`config_show`, `auth_requirements`, `begin`, `reload`, etc.)

## What must NOT change

- Every v5 tier-1 test must still pass after this epic lands.
- The existing `sync_diff` / `workspace_sync_report` shapes from V5REPOS-13 and V5WS-9 remain unchanged; V5PROV-8 adds a new per-member kind (`created`) alongside the existing `in_sync`/`drifted`/`errored`.
- D3 (original) is explicitly superseded by D10 in CONTRACTS; V5REPOS-2's ticket references D3 — evaluators should re-read V5REPOS-2 under D10 and confirm the implementation's trait surface is extended, not replaced.

## Risks

- **R1: Codeberg/GitLab visibility variants.** `internal` is GitLab-only; `private` on Codeberg maps to "limited". Each adapter rejects variants its provider doesn't support; test scripts skip non-supported combinations.
- **R2: Partial-creation rollback.** If `create_remote=true` succeeds on the forge but the local yaml write fails, the forge has an orphan repo. D11 specifies rollback on forge error (local-first); rollback on local error is harder. V5PROV-6 pins which direction: the implementation MUST write the local entry first, then call adapter.create_repo; on forge error, the local entry is rolled back (unregistered).
- **R3: Idempotency of `workspaces.sync` on already-remote repos.** After the first sync creates a member, subsequent syncs must not re-create. V5PROV-8 must call `repo_exists` before `create_repo`; acceptance includes re-running sync is a no-op on the create path.
