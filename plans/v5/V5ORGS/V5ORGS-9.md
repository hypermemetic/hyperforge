---
id: V5ORGS-9
title: "ORGS checkpoint — CRUD + credentials composes end-to-end"
status: Ready
type: checkpoint
blocked_by: [V5ORGS-2, V5ORGS-3, V5ORGS-4, V5ORGS-5, V5ORGS-6, V5ORGS-7, V5ORGS-8]
unlocks: []
---

## Problem

Each V5ORGS implementation ticket is verified in isolation. We need a
single green checkpoint that proves the composed surface satisfies the
V5ORGS-1 user stories — no new behavior, just aggregation.

## User stories → verifying scripts

| # | User story | Verified by |
|---|---|---|
| U1 | Onboard a new org (create + set_credential from scratch). | `tests/v5/V5ORGS/V5ORGS-4.sh`, `V5ORGS-7.sh` |
| U2 | Inspect without leaking (get returns `OrgDetail` with no secret plaintext). | `tests/v5/V5ORGS/V5ORGS-3.sh` |
| U3 | Rotate a credential (set_credential replaces same-key entry in place). | `tests/v5/V5ORGS/V5ORGS-7.sh` |
| U4 | Delete with confidence (delete + `dry_run` preview, leaves siblings intact). | `tests/v5/V5ORGS/V5ORGS-5.sh` |
| U5 | Survive restart (list/get on a fresh daemon equals pre-restart). | `tests/v5/V5ORGS/V5ORGS-2.sh`, `V5ORGS-4.sh`, `V5ORGS-8.sh` |
| U6 | Patch provider without clobbering credentials or repos. | `tests/v5/V5ORGS/V5ORGS-6.sh` |
| U7 | Remove one credential without touching others or the secret store. | `tests/v5/V5ORGS/V5ORGS-8.sh` |

## State-of-epic map

Filled at checkpoint evaluation time. Target terminal state: all green.

| Story | Status | One-liner |
|---|---|---|
| U1 onboard | red/yellow/green | _filled by checkpoint run_ |
| U2 inspect | red/yellow/green | _filled by checkpoint run_ |
| U3 rotate | red/yellow/green | _filled by checkpoint run_ |
| U4 delete | red/yellow/green | _filled by checkpoint run_ |
| U5 restart | red/yellow/green | _filled by checkpoint run_ |
| U6 patch | red/yellow/green | _filled by checkpoint run_ |
| U7 remove_credential | red/yellow/green | _filled by checkpoint run_ |

## Required behavior

The checkpoint introduces **zero new daemon behavior**. Its test script
runs every sibling V5ORGS-*.sh in turn, aggregates pass/fail per user
story, and prints the state-of-epic map in U1..U7 order. It spawns no
synapse commands of its own.

Edge cases:

- Any sibling script failing → the owning user story marks red; other stories still report their own status; overall exit non-zero.
- A sibling script missing or not executable → checkpoint fails with a diagnostic naming the missing file.

## What must NOT change

- No new methods, activations, fixtures, or helpers are introduced here.
- If a user story cannot be verified with the sibling scripts as written, the fix is to the **sibling ticket** (amend its acceptance criteria), not this one.

## Acceptance criteria

1. Every sibling script `V5ORGS-{2,3,4,5,6,7,8}.sh` is present and executable.
2. `bash tests/v5/V5ORGS/V5ORGS-9.sh` runs each sibling script and exits 0 iff every sibling exited 0.
3. The checkpoint script prints one line per user story in the form `U<N> <green|yellow|red>: <one-liner>` in the order U1..U7.
4. The checkpoint script introduces no new synapse RPC calls and no new fixtures beyond those present in sibling scripts.

## Completion

- Run `bash tests/v5/V5ORGS/V5ORGS-9.sh` → exit 0.
- Status flips in-commit with the last implementation ticket it aggregates.
