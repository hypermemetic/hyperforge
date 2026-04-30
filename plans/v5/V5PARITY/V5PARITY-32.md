---
id: V5PARITY-32
title: "V5-AS-DEFAULT — make v5 the canonical hyperforge"
status: Complete
type: implementation
blocked_by: []
unlocks: []
---

## Problem

v5 is functionally the daily driver — orgs, repos, workspaces, secrets, build, the onboarding flow, the git2 backend, the typed git ops all work end-to-end. But:

1. The crate version is still `4.1.3`.
2. The canonical `hyperforge` binary points at v4 code; v5 is shipped as `hyperforge-v5`.
3. The default port is split: v4 on 44104, v5 on 44105.
4. README leads with v4 framing.
5. Top-level `docs/` describes v4; v5 docs are siloed under `docs/v5/`.

A new user `cargo install`-ing `hyperforge` gets v4. Confusing.

## Required behavior

**Cargo.toml**:
- Bump `version` from `4.1.3` to `5.0.0`.
- The `[[bin]] name = "hyperforge"` entry now points at the v5 source. The v4 source is preserved under a `hyperforge-legacy` name (one-release courtesy for any consumer with hard-coded automation).
- A `[[bin]] name = "hyperforge-v5"` alias is preserved (same source as `hyperforge`) so the test harness and any explicit-v5 callers don't break.

**Default port**: v5 takes 44104 (v4's port). v4 (now `hyperforge-legacy`) keeps 44104 internally — but since you should only run one daemon at a time, and v5 is the new default, the port is effectively v5's. CONTRACTS D1 updated to reflect the new pinning.

**Top-level docs**:
- `README.md` rewritten to lead with v5 — install, daemon spawn, onboarding flow, common commands.
- `docs/v5/*.md` content stays where it is (don't rearrange under `docs/` root) so existing links don't break, but `docs/README.md` (if/when added) points there.
- A `MIGRATION.md` at the repo root documents the v4→v5 handoff: the legacy binary, the config-format coexistence, the four v4-only features still missing (V5PARITY-{28,29,30,31}).

**Tests**:
- The harness's `__hf_find_bin` lookup tries `target/debug/hyperforge` first, falls back to `target/debug/hyperforge-v5`. Existing tests work either way.

## What must NOT change

- Wire surface — every event shape, method name, parameter is identical pre/post.
- The v4 binary still builds. Anyone needing v4 runs `cargo run --bin hyperforge-legacy`.
- DRY invariants from V5LIFECYCLE-11 stay green.
- Existing tests pass without modification.

## Acceptance criteria

1. `cargo build` produces `target/debug/{hyperforge, hyperforge-v5, hyperforge-legacy}` binaries — the first two are byte-for-byte identical (or trivially so).
2. `target/debug/hyperforge --port 44104 --config-dir ~/.config/hyperforge` starts the v5 daemon on 44104.
3. `cargo metadata --format-version 1 | jq -r '.packages[0].version'` returns `"5.0.0"`.
4. `README.md`'s opening sentence describes v5, not v4.
5. The full V5PARITY tier-1 suite passes.
6. V5LIFECYCLE-11 DRY invariants stay green.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-32.sh` → exit 0 (tier 1; checks Cargo.toml shape + README + binary list).
- Ready → Complete in-commit.
