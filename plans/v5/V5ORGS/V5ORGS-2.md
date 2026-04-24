---
id: V5ORGS-2
title: "orgs.list — stream OrgSummary per org"
status: Pending
type: implementation
blocked_by: [V5CORE-3, V5CORE-6, V5CORE-9]
unlocks: [V5ORGS-9]
---

## Problem

Users cannot discover which orgs the daemon knows about. `orgs.list`
enumerates every org on disk as a typed `OrgSummary` stream, suitable for
scripting and for humans via synapse.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|

(No inputs.)

| Output / Event | Shape | Notes |
|---|---|---|
| one event per org | `OrgSummary` | from CONTRACTS §types; `repo_count` is the length of that org's `repos[]` |
| stream terminator | standard synapse completion | caller observes stream end |

Ordering: case-sensitive ascending by `OrgName`. Deterministic across calls
on an unchanged config dir.

Edge cases:

- Zero org files: emits zero `OrgSummary` events, then terminates normally. Not an error.
- A file under `orgs/` that fails to parse as an org yaml: emits a typed error event naming the file; other orgs still stream.
- Non-`.yaml` files (or dotfiles) under `orgs/`: ignored (V5CORE-3 loader contract).

## What must NOT change

- v4's `orgs_list` under port 44104 is unaffected (separate namespace, separate daemon).
- `orgs.list` never resolves any `SecretRef` (Secret redaction rule).
- No filesystem mutation — `orgs.list` is read-only.

## Acceptance criteria

1. Against the `empty` fixture, `orgs.list` produces zero `OrgSummary` events and completes successfully.
2. Against `minimal_org`, exactly one event satisfies `.type == "org_summary" and .name == "demo" and .provider == "github" and .repo_count == 0`.
3. Against `two_orgs`, exactly two `org_summary` events are emitted, in ascending `name` order (`acme` before `demo`).
4. No event emitted by `orgs.list` contains a plaintext secret value, even if `secrets.yaml` is populated.
5. After moving an org file out and back in, two successive `orgs.list` calls against a fresh daemon return equal event sequences.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-2.sh` → exit 0.
- Status flips in-commit with the implementation.
