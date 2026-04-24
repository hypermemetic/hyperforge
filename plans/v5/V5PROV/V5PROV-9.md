---
id: V5PROV-9
title: "V5PROV checkpoint — end-to-end workflow on real forge"
status: Ready
type: checkpoint
blocked_by: [V5PROV-2, V5PROV-3, V5PROV-4, V5PROV-5, V5PROV-6, V5PROV-7, V5PROV-8]
unlocks: []
---

## Problem

Verify the user stories the epic was motivated by — that the composed
V5PROV methods deliver the workflow end-to-end on a live forge.

## User stories (this checkpoint verifies)

1. **Stand up a new workspace.** `workspaces create name=X path=Y` succeeds; `workspaces list` includes it.
2. **Register + provision a repo in one call.** `repos add --org O --name N --remotes '[...]' --create_remote true --visibility private --description D` emits `repo_created` then `repo_added`; afterwards `gh repo view O/N --json visibility` returns `private`.
3. **Add to workspace + sync.** `workspaces add_repo --name X --ref O/N` then `workspaces sync --name X` emits `sync_diff { status: in_sync }` and `workspace_sync_report { total: 1, in_sync: 1, created: 0 }` (since repo already exists from story 2).
4. **Remote-only creation via sync.** Alternative path: `repos add` without `--create_remote`, then `workspaces sync` creates the remote automatically; report shows `created: 1`.
5. **Delete cascade.** `repos delete --org O --name N --delete_remote true` removes the local entry AND the forge repo; `gh repo view O/N` returns 404.
6. **Zero-leak invariant.** No event across the entire workflow contains the resolved token value.

## State-of-epic map

| User story | Script assertion | Expected |
|---|---|---|
| U1: workspace create | `workspaces create` + `workspaces list` | Green |
| U2: add + create_remote | `repos add --create_remote true` + `gh repo view` | Green (tier 2) |
| U3: sync existing | `workspaces sync` → report | Green (tier 2) |
| U4: sync creates remote | `workspaces sync` with absent remote → `created: 1` | Green (tier 2) |
| U5: delete cascade | `repos delete --delete_remote true` + `gh repo view` 404 | Green (tier 2) |
| U6: no token leakage | grep all event streams for token | Green |

Stories U2..U5 require a tier-2 config. Without it, the checkpoint
script SKIPs those assertions and prints `yellow` for each — not red.
U1 and U6 are tier 1; always green when implementation is correct.

## Acceptance criteria

1. The checkpoint script creates a throwaway repo name prefixed `v5prov-ckpt-$(date +%s)` to avoid colliding with prior runs.
2. Every user story's scripted assertion either passes or emits `yellow: <story> — <reason>`.
3. The checkpoint script leaves the forge in the same state it started: any repo created by the script is deleted by the end (via U5's cascade); any transient workspace yaml is removed.
4. No assertion references a hardcoded token — all credentials flow through the tier-2 config's `secrets.yaml` per the CONTRACTS §harness pattern.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-9.sh` → exit 0 (SKIP-clean without tier-2 config; tier-2 verified with it).
- Status flips to Complete.
