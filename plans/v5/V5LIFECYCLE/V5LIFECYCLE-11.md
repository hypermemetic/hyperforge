---
id: V5LIFECYCLE-11
title: "V5LIFECYCLE checkpoint — lifecycle matrix + DRY grep invariant"
status: Complete
type: checkpoint
blocked_by: [V5LIFECYCLE-2, V5LIFECYCLE-3, V5LIFECYCLE-4, V5LIFECYCLE-5, V5LIFECYCLE-6, V5LIFECYCLE-7, V5LIFECYCLE-8, V5LIFECYCLE-9, V5LIFECYCLE-10]
unlocks: []
---

## Problem

Verify the user stories the epic was motivated by + enforce the DRY invariant (D13) as a mechanical grep test that any future regression will trip.

## User stories (the checkpoint verifies)

1. **Soft-delete round-trip.** `repos.delete X` on an active repo → forge shows visibility: private, org yaml shows `lifecycle: dismissed` + `privatized_on`. Forge repo still exists.
2. **Dismissed stays listable.** `repos.list` still returns the dismissed repo. `repos.get` returns it with `lifecycle: dismissed`.
3. **Purge completes the life.** `repos.purge X` on the dismissed repo → forge shows 404, local record gone.
4. **Protection blocks both.** `repos.protect X --protected true` → subsequent `repos.delete` and `repos.purge` both fail with `error { code: protected }`.
5. **Init round-trip.** `repos.init --target_path /tmp/demo --org O --repo_name R --forges [github]` creates `.hyperforge/config.toml`; re-running without --force fails; re-running with --force succeeds.
6. **Workspace config drift event.** Local dir's `.hyperforge/config.toml` disagrees with the workspace's org yaml assignment → reconcile emits `config_drift`.
7. **Dismissed skipped in sync.** `workspaces.sync` on a workspace containing a dismissed member emits `sync_skipped` for it by default.

## State-of-epic map

| User story | Assertion | Expected |
|---|---|---|
| U1: soft-delete | `hf_cmd repos delete X` → `gh repo view` shows private + org yaml shows dismissed | Green (tier 2) |
| U2: dismissed still listable | `hf_cmd repos list` + `hf_cmd repos get` | Green |
| U3: purge cascade | `hf_cmd repos purge X` → `gh repo view` 404, `repos.list` empty | Green (tier 2) |
| U4: protection | `hf_cmd repos protect X --protected true` + delete/purge both refuse | Green |
| U5: .hyperforge init | `repos.init` writes file; re-run without --force fails | Green |
| U6: config_drift | reconcile flags identity mismatch | Green |
| U7: dismissed skipped in sync | `workspaces.sync` emits `sync_skipped` | Green |

U1/U3 require `delete_repo` scope on the gh token (or equivalent API capability). Checkpoint classifies yellow on scope absence via `gh auth status | grep delete_repo`, not red.

## DRY invariant — the grep tests

Also asserts the D13 structural invariants as grep tests:

| Grep | Expected |
|---|---|
| `grep -RE 'serde_yaml::from_str\|serde_yaml::to_string\|fs::(read_to_string\|write)' src/v5/ | grep -v '^src/v5/(ops\|secrets)/'` | empty |
| `grep -RE 'adapter\.(read_metadata\|write_metadata\|create_repo\|delete_repo\|repo_exists\|update_repo)' src/v5/ | grep -v '^src/v5/ops/'` | empty |
| `grep -RE 'for_provider' src/v5/ | grep -v '^src/v5/(ops\|adapters)/'` | empty |
| `grep -RE 'compute_drift' src/v5/ | grep -v '^src/v5/ops/'` | empty |

Any non-empty match is a DRY violation → red.

## Acceptance criteria

1. The checkpoint script runs every user story assertion and prints one line per story (green/yellow/red).
2. The DRY grep block runs and prints one line per grep (pass/fail) with the first violating file on fail.
3. Exit 0 iff no red stories and no grep failures. Yellow (tier-2 scope) is acceptable.
4. The checkpoint cleans up: any repo created during the run is dismissed then purged; any workspace created is deleted. After the script exits, `gh repo list hypermemetic --limit 100 | grep 'v5lifecycle-ckpt'` returns empty.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-11.sh` → exit 0.
- Status flips in-commit.
