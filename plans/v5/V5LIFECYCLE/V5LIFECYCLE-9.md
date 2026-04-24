---
id: V5LIFECYCLE-9
title: "repos.init — write .hyperforge/config.toml in a repo checkout"
status: Complete
type: implementation
blocked_by: [V5LIFECYCLE-2]
unlocks: [V5LIFECYCLE-10, V5LIFECYCLE-11]
---

## Problem

Per D14 a repo checkout MAY carry a `.hyperforge/config.toml` that declares its identity. Today no method writes one. This ticket adds `repos.init`.

## Required behavior

Method signature (mirrors the subset of v4's `repo init` that v5 cares about; v5 does not auto-register in an org, does not `git init`, does not run hooks):

| Input | Type | Required | Notes |
|---|---|---|---|
| `path` | `FsPath` | yes | the repo directory. **Named `target_path` to avoid the synapse `path`-autoexpansion footgun** (memory: synapse expands param literally named `path`) |
| `org` | `OrgName` | yes | |
| `repo_name` | `RepoName` | yes | |
| `forges` | `Vec<ProviderKind>` | yes | the forges the repo should sync to |
| `default_branch` | `String` | no | defaults to `main` |
| `visibility` | `ProviderVisibility` | no | defaults to `private` |
| `description` | `String` | no | defaults to empty |
| `dry_run` | `bool` | no (default false) | D7 |
| `force` | `bool` | no (default false) | when false, refuse if `<target_path>/.hyperforge/config.toml` already exists |

Execution:

1. Validate `target_path` exists and is a directory.
2. If the file exists and `force: false` → `error { code: already_exists, message: "use --force to overwrite" }`.
3. Construct `HyperforgeRepoConfig` (CONTRACTS §types) from the params.
4. Serialize to TOML.
5. Write via `ops::fs::write_hyperforge_config` (new helper under `ops::`, D13) atomically: `<target>/.hyperforge/config.toml.tmp` → rename.
6. Emit `hyperforge_config_written { path: <target>/.hyperforge/config.toml, repo_name, org }`.

Read-side helper `ops::fs::read_hyperforge_config(dir) -> Result<Option<HyperforgeRepoConfig>, io::Error>` is part of this ticket — it returns `Ok(None)` when the file is absent (not an error). V5LIFECYCLE-10 consumes it.

## What must NOT change

- `ops::state::*` (V5LIFECYCLE-2) — this ticket adds a sibling module `ops::fs` or extends `ops::repo`; it does NOT mix `.hyperforge/config.toml` into yaml-layer helpers.
- The org yaml — `repos.init` does NOT auto-register the repo in an org yaml. Registration is `repos.add` (V5PROV-6).

## Acceptance criteria

1. Run against a fresh empty dir → writes `.hyperforge/config.toml`; the file TOML-parses to a `HyperforgeRepoConfig` with the input fields.
2. Run twice without `--force` → second run emits `error { code: already_exists }`; file byte-identical.
3. Run twice with `--force true` → second run overwrites; file matches the second invocation's inputs.
4. Run with `dry_run: true` → emits `hyperforge_config_written` event but file is not created (directory byte-identical).
5. `ops::fs::read_hyperforge_config(<dir>)` returns `Ok(Some(cfg))` after step 1, `Ok(None)` when the file is absent, `Err` on malformed TOML.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-9.sh` → exit 0 (tier 1).
- Status flips in-commit.
