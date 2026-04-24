---
id: V5ORGS-5
title: "orgs.delete — remove org yaml with dry_run"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users need to remove an org entirely, with a preview option (D7). The
deletion must be local-filesystem only — it MUST NOT contact any forge
(README §4 invariant: forge-side deletion is a separate explicit flag,
out of scope for this ticket).

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | must match an existing `orgs/<OrgName>.yaml` |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one event identifying the deleted `OrgName` | e.g. `{type: "org_deleted", name: OrgName}` |
| not found | typed error event | names the `OrgName` |

Post-condition on `dry_run: false` success: `orgs/<org>.yaml` no longer
exists on disk. Other files under `orgs/`, `workspaces/`, and
`secrets.yaml` are byte-identical to their pre-call contents.

Post-condition on `dry_run: true`: the same deletion event is emitted,
but `orgs/<org>.yaml` still exists on disk and is byte-identical to
pre-call content.

Edge cases:

- `org` not on disk: typed not-found error; no event of the success type emitted; filesystem unchanged.
- Workspaces referencing the org in `repos[]` entries: this ticket does NOT validate or cascade. Workspace yamls are untouched.

## What must NOT change

- No forge-side remote is contacted. Deletion is local-file-only.
- Other orgs, all workspaces, and `secrets.yaml` remain untouched.
- v4 org files under the same directory (if any) are untouched.

## Acceptance criteria

1. Against `two_orgs`, `orgs.delete org=demo` emits the deletion event; after the call `orgs/demo.yaml` does not exist and `orgs/acme.yaml` is byte-identical to before.
2. After (1), `orgs.list` on a fresh daemon on the same `$HF_CONFIG` emits exactly one `OrgSummary` (`acme`).
3. Against `two_orgs`, `orgs.delete org=demo dry_run=true` emits the same-shape deletion event but `orgs/demo.yaml` still exists byte-identical after the call.
4. Against `minimal_org`, `orgs.delete org=nonexistent` emits a typed error naming `nonexistent`; no file under `orgs/` is modified.
5. Against `minimal_org` with a `workspaces/` fixture entry that references `demo/foo`, `orgs.delete org=demo` still succeeds and leaves every `workspaces/*.yaml` byte-identical.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-5.sh` → exit 0.
- Status flips in-commit with the implementation.
