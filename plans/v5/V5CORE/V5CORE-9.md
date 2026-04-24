---
id: V5CORE-9
title: "Integration test harness — lib.sh surface and Rust runner"
status: Complete
type: implementation
blocked_by: [V5CORE-2]
unlocks: [V5CORE-10, V5ORGS-1, V5REPOS-1, V5WS-1]
---

## Problem

Every ticket's acceptance is scripted as bash that sources a shared
harness. No such harness exists. Until it does, every test script fails
at `source` — that's the intended TDD red. This ticket ships the harness.

## Required behavior

### Bash surface — `tests/v5/harness/lib.sh`

Implements exactly the function set pinned in CONTRACTS §harness (copied
here verbatim — if this table drifts from §harness, §harness wins):

| Function                              | Behavior                                                                                      |
|---------------------------------------|-----------------------------------------------------------------------------------------------|
| `hf_spawn`                            | Spawns the v5 daemon on an ephemeral TCP port. Exports `$HF_PORT` and `$HF_CONFIG` (a fresh temp dir). Returns after the daemon reports ready (schema query succeeds). Registers `hf_teardown` as an EXIT trap. |
| `hf_cmd <args...>`                    | Runs `synapse -P $HF_PORT lforge hyperforge <args>`. Stdout is the RPC event stream as NDJSON. |
| `hf_load_fixture <fixture_name>`      | Copies every file under `tests/v5/fixtures/<fixture_name>/` into `$HF_CONFIG`, creating dirs. |
| `hf_put_secret <secret_ref> <value>`  | Writes `<value>` into `$HF_CONFIG/secrets.yaml` under the `<path>` portion of `<secret_ref>`. Creates the file if missing. Atomic (D8). |
| `hf_assert_event <jq_filter>`         | Reads stdin as NDJSON. Exit 0 iff at least one line satisfies the jq filter. Else exit 1 with a diagnostic listing the events seen. |
| `hf_assert_no_event <jq_filter>`      | Exit 0 iff zero lines satisfy the filter.                                                     |
| `hf_assert_count <jq_filter> <n>`     | Exit 0 iff exactly `n` lines satisfy the filter.                                              |
| `hf_teardown`                         | Kills the spawned daemon. Removes `$HF_CONFIG`. Idempotent. Safe to call twice.               |

Invariants across helpers:

- Ephemeral port selection never collides across parallel test processes (two concurrent `hf_spawn` calls get distinct ports).
- `$HF_CONFIG` is under the OS temp dir; teardown removes it even on test failure.
- All helpers use `set -e`-safe constructs; a failing assertion aborts the script with a non-zero exit.
- No helper reads state from the caller's env beyond `$PATH` and standard UNIX vars.

### Rust runner — `tests/v5_integration.rs`

One Rust test file that discovers every `.sh` file under `tests/v5/*/`
(excluding `harness/` and `fixtures/`) and runs each as one `#[test]`.

| Input | Source | Notes |
|---|---|---|
| script path | filesystem discovery at test time | one `#[test]` per script |
| script tier | magic comment `# tier: <N>` on line 2 of the script (absent = tier 1) | gates execution |

| Output | Shape | Notes |
|---|---|---|
| pass | script exit 0 | test passes |
| fail | script non-zero exit | test fails; stderr and captured stdout are included in the failure message |

Tier gating:

- Default `cargo test --test v5_integration` runs only tier-1 scripts.
- `--features tier2` adds tier-2 scripts.
- `--features tier3` adds tier-3 scripts.
- A script with an unknown tier value is a hard error at discovery time.

Edge cases:

- Zero scripts discovered: the runner reports zero tests (still exits 0); not an error.
- A script marked `tier: 3` is skipped (not failed) under the default feature set.

## What must NOT change

- The §harness table in CONTRACTS.md is the source of truth for the bash
  surface. This ticket implements it; it does not amend it.
- Test scripts authored before this ticket lands continue to fail at
  `source` until the harness exists. That is intentional.

## Acceptance criteria

1. `bash tests/v5/V5CORE/V5CORE-9.sh` exits 0. The script: (a) calls `hf_spawn`, (b) runs `hf_cmd` with a trivial argument (introspection or status), (c) asserts at least one event is produced, (d) captures `$HF_CONFIG`, (e) calls `hf_teardown`, (f) verifies the captured `$HF_CONFIG` no longer exists on disk.
2. Two concurrent invocations of the script in separate shells both pass (ephemeral ports do not collide).
3. `cargo test --test v5_integration` discovers every `tests/v5/<EPIC>/*.sh` file and turns each into one `#[test]`; the set excludes anything under `tests/v5/harness/` and `tests/v5/fixtures/`.
4. A script containing `# tier: 3` is **not** executed under default features and is reported as ignored/skipped, not as a pass or fail.
5. A script containing `# tier: 99` causes the runner to fail at discovery with a diagnostic naming the file and the bad tier value.
6. Forcing a sourced helper failure (e.g., `hf_assert_event 'nonexistent'` on an empty stream) causes the script to exit non-zero; the runner reports that script as failed.

## Completion

- Run `bash tests/v5/V5CORE/V5CORE-9.sh` → exit 0.
- Run `cargo test --test v5_integration v5core_9` → passes.
- Status flips in-commit with the implementation.
