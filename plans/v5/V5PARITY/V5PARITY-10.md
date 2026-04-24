---
id: V5PARITY-10
title: "BUILD-RELEASE — bump, publish, release, release_all"
status: Pending
type: implementation
blocked_by: [V5PARITY-9]
unlocks: [V5PARITY-12]
---

## Problem

v4's release pipeline: bump a version, publish to crates.io (or npm/PyPI), tag the repo, trigger GitHub releases, and coordinate across multiple repos that share a semver cadence. v5 has nothing.

## Required behavior

**Extend `src/v5/build/`** with `release.rs`, `publish.rs`.

| Method | Behavior |
|---|---|
| `build.bump --org X --name N [--bump major\|minor\|patch] [--to VERSION]` | Parses the repo's manifest, increments version per semver rule (or sets exact version with `--to`), writes back, commits `chore: bump version to X`, tags `vX.Y.Z`. Uses `ops::git`. Emits `version_bumped { old, new }`. |
| `build.publish --org X --name N [--channel crates.io\|npm\|pypi]` | Invokes the appropriate publish command (`cargo publish`, `npm publish`, `twine upload`). Tier 2. Emits `package_published` on success. Requires credentials resolved through the existing `SecretResolver`. |
| `build.release --org X --name N` | Composes: `bump` → `push_refs` → `publish` → create GitHub/Codeberg/GitLab release (via `ForgePort` — adds `create_release` trait method). One-shot release. |
| `build.release_all --name W [--only-changed true]` | For each member of workspace W (optionally filtered to those with commits since last tag), run `release`. Bounded parallelism + partial-failure-tolerant per D6. |

**ForgePort trait extension:** `create_release(repo_ref, tag, title, body, auth) -> Result<(), ForgePortError>` — adds to the trait + all three adapters.

## What must NOT change

- V5PARITY-3's `repos.push_refs` is the only place that pushes git refs. `build.release` calls it through the `ops::git` helper the same way.
- D13 — `build/*` never directly invokes `git` or adapters; always via `ops::*`.
- Credentials resolve through the same pattern as forge operations.

## Acceptance criteria

1. `build.bump --bump patch` against a repo at version 0.1.0 writes 0.1.1, commits, tags, and emits `version_bumped`.
2. `build.publish --channel crates.io` publishes a crate (tier 2 — SKIP-clean without `CARGO_REGISTRY_TOKEN` or equivalent). The `SecretResolver` pulls the token from `secrets://cargo/token`.
3. `build.release` against a repo emits the full sequence: `version_bumped`, `push_done`, `package_published`, `release_created` (with the forge-side URL).
4. `build.release_all --name W` iterates members; per-member events; aggregate `release_summary`.
5. A failed publish (bad token) leaves the repo in a consistent state — the tag still got created but no forge-side release. Subsequent `release` is idempotent on the already-tagged version.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-10.sh` → exit 0 (tier 2 — needs publish credentials).
- Ready → Complete in-commit.
