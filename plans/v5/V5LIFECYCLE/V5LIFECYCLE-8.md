---
id: V5LIFECYCLE-8
title: "repos.protect — toggle protection bit"
status: Complete
type: implementation
blocked_by: [V5LIFECYCLE-5]
unlocks: [V5LIFECYCLE-11]
---

## Problem

A repo marked `protected: true` refuses `repos.delete` and `repos.purge`. Users need a way to set and clear the bit.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | |
| `name` | `RepoName` | yes | |
| `protected` | `bool` | yes | idempotent target state |
| `dry_run` | `bool` | no (default false) | D7 |

Execution:

1. Load + find repo.
2. Set `metadata.protected = <value>`.
3. Save via `ops::state::save_org` (unless `dry_run`).
4. Emit `repo_protection_set { ref, protected: bool }`.

Setting the same value as already set is idempotent — no yaml change, still emits the event.

## What must NOT change

- Existing tier-1 tests unchanged (protection isn't exercised there).
- The `protected` field added by V5LIFECYCLE-5 is its storage; this ticket is just the setter.

## Acceptance criteria

1. `repos.protect --org X --name Y --protected true` writes `protected: true` into the org yaml. `repos.get` shows it.
2. `repos.protect --org X --name Y --protected false` clears it. `repos.get` shows `protected: false` or absent (implementer's choice — CONTRACTS says default false is skip_serializing_if).
3. `repos.delete` on a protected repo fails as V5LIFECYCLE-6 AC3 asserts.
4. `repos.purge` on a protected+dismissed repo fails as V5LIFECYCLE-7 AC3 asserts.
5. `dry_run: true` emits the event but leaves yaml byte-identical.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-8.sh` → exit 0.
- Status flips in-commit.
