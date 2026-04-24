---
id: V5ORGS-6
title: "orgs.update — patch org provider without clobbering other fields"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users need to change an org's provider on an existing org without
rewriting the whole yaml — specifically without touching `credentials[]`
or `repos[]`, which are owned by other methods. Rename is out of scope
per V5ORGS-1.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | must match an existing `orgs/<OrgName>.yaml` |
| `provider` | `ProviderKind` | no | when present, replaces the current provider |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one `OrgSummary` event reflecting the post-update state | `provider` is the new value; `repo_count` unchanged from pre-call |
| no-op | typed error event | omitting every optional field is an error, not a silent success |
| not found | typed error event | names the `OrgName` |
| unknown variant | typed error event | names the offending input |

Post-condition on `dry_run: false` success: `orgs/<org>.yaml` on disk has
the new `provider` and everything else — `name`, `credentials[]` (including
order), `repos[]` (including remote entries) — byte-equivalent after
round-trip through the V5CORE-3 loader. Write obeys D8.

Post-condition on `dry_run: true`: same event, `orgs/<org>.yaml`
byte-identical to pre-call content.

Edge cases:

- All optional fields omitted: typed error (nothing to change); file untouched.
- `provider` is an unknown variant: typed error at the wire boundary; file untouched.
- `org` absent from disk: typed not-found error; no files written.

## What must NOT change

- `credentials[]` — managed exclusively by V5ORGS-7 and V5ORGS-8.
- `repos[]` — managed exclusively by the V5REPOS epic.
- The org's on-disk filename — rename is out of scope (V5ORGS-1).
- Any other org's file, any workspace file, and `secrets.yaml`.

## Acceptance criteria

1. Against `org_with_credentials`, `orgs.update org=demo provider=codeberg` emits an `OrgSummary` with `provider == "codeberg"` and unchanged `repo_count`; a follow-up `orgs.get org=demo` reports `provider == "codeberg"` and the exact same `credentials` list (same length, same `CredentialEntry` values in the same order) as before.
2. After (1), a fresh-daemon `orgs.get org=demo` reports the new provider — state is on disk.
3. Against `org_with_credentials`, `orgs.update org=demo provider=codeberg dry_run=true` emits the same shape event but `orgs.get org=demo` on a fresh daemon still reports `provider == "github"`.
4. Against `minimal_org`, `orgs.update org=demo` (no optional fields) emits a typed error; `orgs/demo.yaml` is byte-identical.
5. Against `minimal_org`, `orgs.update org=nonexistent provider=github` emits a typed error naming `nonexistent`; no file is modified.
6. `orgs.update org=demo provider=not_a_variant` emits a typed error; `orgs/demo.yaml` is byte-identical.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-6.sh` → exit 0.
- Status flips in-commit with the implementation.
