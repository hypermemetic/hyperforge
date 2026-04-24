---
id: V5REPOS-15
title: "REPOS checkpoint — user-story verification + state-of-epic map"
status: Complete
type: checkpoint
blocked_by: [V5REPOS-2, V5REPOS-3, V5REPOS-4, V5REPOS-5, V5REPOS-6, V5REPOS-7, V5REPOS-8, V5REPOS-9, V5REPOS-10, V5REPOS-11, V5REPOS-12, V5REPOS-13, V5REPOS-14]
unlocks: []
---

## Problem

Each V5REPOS implementation ticket is verified in isolation. We need one
green checkpoint that proves the composed surface satisfies the epic's
user stories — no new behavior, just aggregation.

## User stories → verifying scripts

| # | User story (from V5REPOS-1) | Verified by |
|---|---|---|
| U1 | Register an existing remote: add a repo by name + URL; domain auto-resolves to provider. | `V5REPOS-5.sh`, `V5REPOS-4.sh`, `V5REPOS-12.sh` |
| U2 | Add a mirror: add a second remote on a different provider without needing its credentials at add time. | `V5REPOS-7.sh` |
| U3 | Remove without destroying: default `repos.remove` drops only the local entry; `delete_remote: true` opt-in for forge delete. | `V5REPOS-6.sh` |
| U4 | Sync metadata: `repos.sync` reports drift between local and forge; no writes. | `V5REPOS-13.sh` |
| U5 | Push metadata: `repos.push` propagates local edits to the forge. | `V5REPOS-14.sh` |
| U6 | Cross-provider: a repo's GitHub remote and Codeberg remote sync/push dispatch per-remote. | `V5REPOS-13.sh` + `V5REPOS-9.sh` + `V5REPOS-10.sh` |
| U7 | Custom-domain provider: a repo whose remote's domain has an explicit `provider:` override (or a matching `provider_map` entry) works end-to-end. | `V5REPOS-12.sh`, `V5REPOS-4.sh` |

## State-of-epic map

Filled at checkpoint evaluation time. Target terminal state: all green.

| Story | Status | One-liner |
|---|---|---|
| U1 register | red/yellow/green | _filled by checkpoint run_ |
| U2 add mirror | red/yellow/green | _filled by checkpoint run_ |
| U3 remove safely | red/yellow/green | _filled by checkpoint run_ |
| U4 sync | red/yellow/green | _filled by checkpoint run_ |
| U5 push | red/yellow/green | _filled by checkpoint run_ |
| U6 cross-provider | red/yellow/green | _filled by checkpoint run_ |
| U7 custom domain | red/yellow/green | _filled by checkpoint run_ |

## Required behavior

The checkpoint introduces ZERO new daemon behavior. Its test script runs
every sibling V5REPOS-2..14 script in turn, aggregates pass/fail, and
prints the state-of-epic map. It does not spawn additional synapse
commands beyond what those scripts spawn.

Tier handling: the checkpoint runs the tier-1 scripts unconditionally
and runs the tier-2 scripts when their required env vars are present.
A skipped (SKIP) tier-2 script counts as yellow, not red, in the map —
a partial checkpoint. The checkpoint's own exit is:

- 0 iff every tier-1 script passes AND every tier-2 script either passes or skips cleanly.
- non-zero iff any script fails (missing file, non-zero exit for a non-SKIP reason).

Edge cases:

- A sibling script missing → checkpoint fails with a diagnostic naming the missing file.
- A sibling script exits with `SKIP:` on stdout → counted as yellow; not a failure.

## What must NOT change

- This ticket adds no methods, activations, fixtures, or helpers.
- If a user story cannot be verified with the sibling scripts as written, the fix is to the sibling ticket (amend its acceptance criteria), not this one.

## Acceptance criteria

1. Every sibling script `V5REPOS-{2..14}.sh` is present and executable.
2. `bash tests/v5/V5REPOS/V5REPOS-15.sh` runs each sibling script and exits 0 iff every sibling passed or skipped cleanly.
3. The checkpoint script prints one line per user story in the form `U<N> <green|yellow|red>: <one-liner>` in order U1..U7.
4. The checkpoint script introduces no new synapse RPC calls and no new fixtures beyond those already present.
5. When all tier-2 env vars are unset, the checkpoint still exits 0 with U4/U5/U6 marked yellow (SKIP) and U1/U2/U3/U7 green.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-15.sh` → exit 0.
- Status flips in-commit with the last implementation ticket it aggregates.
