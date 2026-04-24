---
id: V5PARITY-13
title: "V5PARITY checkpoint — v4 retireable"
status: Ready
type: checkpoint
blocked_by: [V5PARITY-2, V5PARITY-3, V5PARITY-4, V5PARITY-5, V5PARITY-6, V5PARITY-7, V5PARITY-8, V5PARITY-9, V5PARITY-10, V5PARITY-11, V5PARITY-12]
unlocks: []
---

## Problem

Verify the epic delivered what it promised: **v5 covers every v4 capability** such that v4 can be retired, AND the DRY + data-structure invariants from V5LIFECYCLE + V5PARITY-12 hold.

## User stories

1. **Daily repo management.** Create a workspace, import an org's repos, clone every member, dirty-check them, pull them, push them.
2. **Remote management.** Add a remote, rename a repo on the forge (and have every workspace yaml update), set_default_branch across a whole workspace.
3. **Lifecycle.** Create → dismiss → purge a repo end-to-end, with protection refusing both destructive steps until cleared.
4. **Analytics.** `workspaces.repo_sizes` on a workspace with 3+ members returns correct per-member sizes + aggregate.
5. **Auth.** `secrets.set` writes a token; `auth_check` confirms it works against the forge.
6. **Build.** `build.unify` on a multi-language workspace produces the unified manifest; `build.release` on a tier-2-enabled repo completes end-to-end.
7. **Onboarding.** `begin` on an empty config dir produces a usable starting state.

## State-of-epic map

| Story | Check | Expected |
|---|---|---|
| U1 | workspace create + import + clone + dirty + pull + push | green (tier 2) |
| U2 | rename + set_default_branch across workspace | green (tier 2) |
| U3 | create → delete → purge with protection guards | green (tier 2) |
| U4 | repo_sizes aggregate | green (tier 1) |
| U5 | secrets.set + auth_check | green (tier 2) |
| U6 | build.unify + build.release | green (tier 2) |
| U7 | begin on empty config | green (tier 1) |

## DRY invariants (inherited from V5LIFECYCLE-11, tightened per V5PARITY-12)

- `serde_yaml::{from_str,to_string}` outside `ops/`, `secrets/`, `config.rs` → red
- `adapter.*` outside `ops/` → red
- `for_provider(` outside `ops/`, `adapters/` → red
- `compute_drift(` outside `ops/` → red
- `Command::new("git")` outside `ops/git.rs` → red (new, per V5PARITY-3's addition)

## Acceptance criteria

1. The checkpoint script runs every user story's assertion (tier 2 stories SKIP-clean without `HF_V5_TEST_CONFIG_DIR`).
2. The DRY grep invariants all green.
3. `cargo test --test v5_integration` under default parallelism: 100% pass rate across 10 consecutive invocations (V5PARITY-12's harness fix verified).
4. No orphan repos left on the test forge after cleanup.
5. Ticket-state audit: every V5PARITY-2..12 has `status: Complete`.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-13.sh` → exit 0 (mix tier).
- Ready → Complete in-commit.
