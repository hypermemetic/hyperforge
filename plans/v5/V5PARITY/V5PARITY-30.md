---
id: V5PARITY-30
title: "WS-CHECK-VERIFY — pre/post-sync invariant audits"
status: Pending
type: implementation
blocked_by: []
unlocks: []
---

## Problem

v4 had `workspace.check` (run BEFORE sync to confirm everything is in a sane state) and `workspace.verify` (run AFTER sync to confirm nothing got mangled). v5 has `workspaces.sync` and `workspaces.diff` (V5PARITY-18, Pending) but no dedicated invariant-audit hooks. Users miss safety nets.

## Required behavior

**`workspaces.check --name W`** — pre-sync invariants. Read-only. Emits one `check_finding` per detected anomaly + a `workspace_check_summary` aggregate. Anomalies (closed enum):

- `MissingCheckout` — workspace yaml lists member but the path doesn't exist on disk.
- `OrphanCheckout` — directory exists at expected path but isn't a git repo.
- `WrongRemote` — checkout's `origin` URL doesn't match the org yaml's canonical remote.
- `DetachedHead` — checkout has no current branch.
- `DirtyWithUpstream` — checkout has uncommitted changes AND ahead/behind status (sync would fail).

`workspace_check_summary { name, total, ok, anomalies: u32 }`. Exit-clean if zero anomalies.

**`workspaces.verify --name W`** — post-sync invariants. Same shape but the findings are different:

- `MetadataDrift` — local `RepoMetadataLocal` differs from forge metadata after a sync (suggests sync was partial or reverted).
- `LocalUntracked` — local repo has uncommitted changes the sync didn't surface (V5PARITY-3 already prohibits sync on dirty trees, but verify catches drift after the fact).
- `MissingForge` — repo registered locally but forge returns 404 (suggests the repo was deleted out-of-band).

## What must NOT change

- `workspaces.sync` semantics — these are advisory before/after, not gates.
- D6 partial-failure tolerance — both methods continue past per-member errors and report them in the aggregate.
- V5PARITY-18 `workspaces.diff` overlap — `check`/`verify` don't replicate `diff`'s file-level output. They flag anomalies, not changes.

## Acceptance criteria

1. `workspaces.check --name W` against a healthy workspace emits zero `check_finding` events and a summary with `anomalies: 0`.
2. After deleting one member's checkout dir, `check` emits `check_finding { kind: "missing_checkout", ref: ... }` and `anomalies: 1`.
3. `workspaces.verify --name W` after a successful sync against a clean workspace emits zero findings.
4. After flipping a repo's metadata on the forge out-of-band, `verify` emits `check_finding { kind: "metadata_drift" }`.
5. Both methods are read-only — running them does not write anything to `~/.config/hyperforge/` or to checkouts.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-30.sh` → exit 0 (tier 1 covers the non-forge anomalies; tier 2 covers metadata-drift / missing-forge).
- Ready → Complete in-commit.
