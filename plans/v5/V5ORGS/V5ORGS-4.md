---
id: V5ORGS-4
title: "orgs.create — write new org yaml with dry_run"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users currently must hand-edit `orgs/<name>.yaml` to onboard an org.
`orgs.create` writes that file from typed parameters, with `dry_run`
support per D7.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `OrgName` | yes | filename-safety enforced at the wire boundary |
| `provider` | `ProviderKind` | yes | closed set; unknown variants rejected |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `OrgSummary` event representing the newly created org | `repo_count == 0`, credentials absent from summary |
| already exists | typed error event | names the conflicting `OrgName` |
| invalid name | typed error event | names the offending input |

Post-condition on `dry_run: false` success: a new `orgs/<name>.yaml` exists
on disk whose content round-trips through the V5CORE-3 loader to an org
with the given `name`, `provider`, empty `credentials`, and empty `repos`.
Writes obey D8 (atomic).

Post-condition on `dry_run: true`: the same `OrgSummary` event is emitted,
but no file exists at `orgs/<name>.yaml` after the call.

Edge cases:

- `name` fails the `OrgName` constraint (contains `/`, leading `.`, >64 chars, non-ASCII): typed error; no file written even in the dry-run case.
- `name` already exists on disk: typed error; existing file untouched (byte-identical).
- `provider` is an unknown variant: typed error at the wire boundary.

## What must NOT change

- Other org files under `orgs/`. Only the target file may appear on disk after the call.
- v4-owned filenames under the same directory (if any) are untouched.
- Secret redaction rule: `orgs.create` never accepts a secret value; credentials are added later via V5ORGS-7.

## Acceptance criteria

1. Against `empty`, `orgs.create name=hypermemetic provider=github` emits an `OrgSummary` with `name == "hypermemetic"`, `provider == "github"`, `repo_count == 0`; after the call `orgs/hypermemetic.yaml` exists.
2. After (1), a fresh-daemon `orgs.list` (teardown + respawn on the same `$HF_CONFIG`) includes `hypermemetic` — state is on disk, not in memory.
3. Against `empty`, `orgs.create name=hypermemetic provider=github dry_run=true` emits the same shape `OrgSummary` event but `orgs/hypermemetic.yaml` does NOT exist after the call.
4. Against `minimal_org`, `orgs.create name=demo provider=github` emits a typed error naming `demo`; `orgs/demo.yaml` content is byte-identical to before the call.
5. `orgs.create name=bad/name provider=github` emits a typed error and writes no file.
6. `orgs.create name=hypermemetic provider=nonsense` emits a typed error and writes no file.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-4.sh` → exit 0.
- Status flips in-commit with the implementation.
