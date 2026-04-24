---
id: V5PARITY-11
title: "BUILD-DIST-EXEC — distribution channels + run/exec"
status: Ready
type: implementation
blocked_by: [V5PARITY-9]
unlocks: [V5PARITY-12]
---

## Problem

v4 has `build.{init_configs, binstall_init, brew_formula, dist_init, dist_show}` for setting up binary distribution (cargo-binstall + Homebrew formula generation) plus `build.{run, exec}` for invoking arbitrary dev-loop commands across workspace members.

## Required behavior

**Extend `src/v5/build/`** with `dist.rs`, `exec.rs`.

| Method | Behavior |
|---|---|
| `build.init_configs --org X --name N` | Seeds a minimal `.hyperforge/dist.toml` in the repo checkout describing its distribution targets. Idempotent. |
| `build.binstall_init --path P` | Adds `cargo-binstall` metadata to `Cargo.toml` under `[package.metadata.binstall]` + generates a release workflow that uploads prebuilt binaries. |
| `build.brew_formula --org X --name N --tap <user>/homebrew-tap` | Generates a Homebrew formula file against the latest release; optionally pushes to a tap repo (tier 2). |
| `build.dist_init --name W` | Workspace-wide: initializes distribution metadata across every member that's missing it. |
| `build.dist_show --name W` | Read-only: reports the current distribution configuration per member. |
| `build.run --name W --cmd "<shell>"` | Runs the given shell command inside every member directory in parallel (bounded). Aggregates stdout/stderr per member. |
| `build.exec --org X --name N --cmd "<shell>"` | Runs `<shell>` in a single repo's checkout. |

## What must NOT change

- `ops::git` + `ops::state` are the only state/subprocess entry points.
- `.hyperforge/config.toml` stays per-checkout identity-only (V5LIFECYCLE-9). Distribution config lives in a separate `.hyperforge/dist.toml` file to keep concerns split.
- D13 — `build/*` never directly invokes `git`/`cargo`/`npm`/`brew`; always through helpers in `ops::*` or `build/*/exec`.

## Acceptance criteria

1. `build.init_configs` on a fresh repo writes `.hyperforge/dist.toml`; re-running is a no-op.
2. `build.binstall_init` adds the required `[package.metadata.binstall]` stanza to `Cargo.toml` and does NOT alter any other field.
3. `build.brew_formula --tap X/Y` produces a formula at the configured location; with `--dry_run true`, emits the formula content as an event but doesn't write.
4. `build.run --name W --cmd "echo $PWD"` emits one `exec_output` per member with its respective path in stdout.
5. `build.exec --cmd "ls"` against one repo emits a single `exec_output`.
6. Non-zero exit from `run` on one member doesn't abort the rest — aggregate report shows per-member exit codes.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-11.sh` → exit 0 (mostly tier 1; `brew_formula` push + distribution publishing are tier 2).
- Ready → Complete in-commit.
