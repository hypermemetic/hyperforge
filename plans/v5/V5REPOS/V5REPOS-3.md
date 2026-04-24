---
id: V5REPOS-3
title: "repos.list — stream RepoSummary per repo in an org"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-15]
---

## Problem

Users cannot enumerate the repos registered under an org without reading
the org's YAML directly. `repos.list` streams every repo as a typed
`RepoSummary` — the counterpart to `orgs.list` for the per-org repo set.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing `orgs/<OrgName>.yaml` |

| Output / Event | Shape | Notes |
|---|---|---|
| one event per repo | `RepoSummary` | from CONTRACTS §types; `remote_count` is `remotes[].length` for that entry |
| not found | typed error event | `org` does not exist; no `RepoSummary` emitted |
| stream terminator | standard synapse completion | caller observes stream end |

Ordering: case-sensitive ascending by `RepoName`. Deterministic across
calls on an unchanged org file.

Edge cases:

- Org exists but `repos: []`: zero `RepoSummary` events, normal completion. Not an error.
- Org file fails to parse: typed error event referencing `OrgName`; no `RepoSummary`.
- `org` parameter absent: typed error event (missing required parameter).

## What must NOT change

- v4's `repo.*` namespace on port 44104.
- Read-only. No filesystem mutation.
- The org YAML schema itself. `repos.list` consumes the loader from V5CORE-3; it does not redefine the on-disk shape.

## Acceptance criteria

1. Against `minimal_org`, `repos.list org=demo` produces zero `RepoSummary` events and completes successfully.
2. Against `org_with_repo`, exactly one event satisfies `.type == "repo_summary" and .org == "demo" and .name == "widget" and .remote_count == 1`.
3. Against `org_with_mirror_repo`, exactly one `repo_summary` event is emitted with `.remote_count == 2`.
4. `repos.list org=nonexistent` emits a typed error event naming `nonexistent`; no `RepoSummary` is emitted.
5. `repos.list` without the `org` parameter emits a typed error event (missing required parameter).
6. Two successive `repos.list org=demo` calls against a fresh daemon return equal event sequences (determinism).

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-3.sh` → exit 0.
- Status flips in-commit with the implementation.
