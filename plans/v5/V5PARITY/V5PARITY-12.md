---
id: V5PARITY-12
title: "CLEANUP — data-structure tightenings + parallel-test harness + residuals"
status: Pending
type: implementation
blocked_by: [V5PARITY-2, V5PARITY-3, V5PARITY-4, V5PARITY-5, V5PARITY-6, V5PARITY-7, V5PARITY-8, V5PARITY-9, V5PARITY-10, V5PARITY-11]
unlocks: [V5PARITY-13]
---

## Problem

The data-structure audit surfaced several tightenings worth doing before V5PARITY's checkpoint claims "v5 is cleaner than v4." The parallel-test concurrency race in `hf_spawn` also needs a fix — it's been papered over with serial test runs. A few residual doc-comment / naming inconsistencies from earlier epics round out the list.

## Required behavior

### Data-structure tightenings

| Change | Rationale |
|---|---|
| `RepoLifecycle` default becomes `Active`; `RepoMetadataLocal.lifecycle: RepoLifecycle` (non-`Option`). | Makes the "always has a lifecycle" invariant typed rather than convention. Default still serializes as absent via `#[serde(skip_serializing_if = "RepoLifecycle::is_default")]` so fixtures round-trip unchanged. |
| `RepoMetadataLocal.protected: bool` (non-`Option`; default false). | Same rationale — don't model "unset vs false" when they're semantically identical. |
| Introduce `RepoIdentity { org, name }` as an alias for `RepoRef` at the RPC wire boundary OR keep `RepoRef` as-is but rename on-wire to `ref` consistently. (Pick one; implementer's call at ticket time.) | Reduces drift between wire shape and Rust struct name. |
| Document (in code comments, not separate types) that `Remote[0]` is the canonical / primary remote. Add a `canonical_remote()` helper method on `OrgRepo`. | Makes the position-based convention visible without introducing a type-level primary/mirror split. |

### Parallel-test harness fix

- `hf_spawn` currently allocates `$HF_PORT` via a "pick ephemeral + retry" loop. Under high-parallelism `cargo test`, two spawns can race the OS between allocation and bind. **Replace with:** bind a listener on port 0, capture the OS-assigned port, close the listener, then spawn the daemon on that port. This closes the TOCTOU window. Implement in `tests/v5/harness/lib.sh`'s `__hf_pick_port`.
- Additionally: serialize the write to `secrets.yaml` / fixture overlay via `flock` on `$HF_CONFIG` so that two tests using the same template fixture don't race on cp.

### Residuals

- `V5PROV-6/7` doc comments in `src/v5/repos.rs` that grep-flag-false-positive even with V5LIFECYCLE-11's `///` exclusion — audit the surviving cases and either rephrase the comments or tighten the grep regex.
- Verify V5LIFECYCLE-10's `config_drift` shape agrees with V5PARITY-2's `discover_match` — both read `.hyperforge/config.toml`; align their event fields (one emits `declared_org/declared_repo` fields; the other should too).
- `src/v5/orgs.rs` `ReadOrgError::Io` carries `#[allow(dead_code)]` — at V5PARITY-12 time, verify the variant is actually needed (if not, remove; if yes, plumb through to callers).

## What must NOT change

- Wire event shapes — lifecycle/protected defaults stay serialized-absent, so all existing fixtures round-trip byte-identical.
- V5LIFECYCLE-11's DRY invariants stay green; this ticket may TIGHTEN them (narrowing exclusions) but must not loosen them.
- Every prior V5PARITY ticket's acceptance tests keep passing.

## Acceptance criteria

1. Round-trip every committed fixture under `tests/v5/fixtures/` through a load→save cycle — byte-identical before and after (no gratuitous lifecycle/protected emission).
2. `cargo test --test v5_integration` under default parallelism passes 100% of runs across 10 sequential invocations (no flaky v5core_10 / v5lifecycle_8 / v5orgs_4 / v5repos_4 under parallel).
3. V5LIFECYCLE-11 checkpoint's DRY greps still green.
4. `grep -RE '///.*adapter\\.(create_repo|delete_repo)' src/v5/` returns zero remaining doc-comment false-positives, OR the grep pattern is updated to inherently exclude them.
5. `config_drift` event and `discover_match { status: matched }` share the same field vocabulary for the repo identity.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-12.sh` → exit 0.
- Ready → Complete in-commit.
