---
id: V5LIFECYCLE-5
title: "RepoLifecycle state + ops::repo::{dismiss, purge}"
status: Ready
type: implementation
blocked_by: [V5LIFECYCLE-4]
unlocks: [V5LIFECYCLE-6, V5LIFECYCLE-7, V5LIFECYCLE-8]
---

## Problem

Soft-delete needs a place to record that a repo is dismissed and which forges were privatized. Purge needs that same state to gate on. Protection needs a boolean. All three are v5.1-level additions to the repo record.

## Required behavior

Extend `RepoMetadataLocal` in `src/v5/config.rs` with three new fields (per CONTRACTS §types row "RepoMetadataLocal (v5.1)"):

| Field | Type | Default | Semantics |
|---|---|---|---|
| `lifecycle` | `RepoLifecycle` | `active` | `active` (normal), `dismissed` (soft-deleted, still in yaml), `purged` (never seen in yaml — only exists in transit) |
| `privatized_on` | `BTreeSet<ProviderKind>` | empty | set of providers where privatization succeeded during soft-delete |
| `protected` | `bool` | `false` | when `true`, dismiss + purge operations refuse this repo |

Serialization: absent fields on read default as above. `skip_serializing_if` on the defaults → existing yaml files remain byte-identical after a load→save roundtrip with no mutations.

Add two pure mutation functions to `src/v5/ops/repo`:

| Function | Behavior |
|---|---|
| `dismiss(&mut OrgRepo, privatized: BTreeSet<ProviderKind>)` | sets `metadata.lifecycle = dismissed`, extends `metadata.privatized_on ∪= privatized`; leaves everything else intact |
| `purge(&mut OrgConfig, name: &RepoName)` | removes the repo entry from `org.repos`; returns `Result<(), LifecycleError>` where `LifecycleError::NotDismissed` is raised if `lifecycle != dismissed`, `LifecycleError::Protected` if `protected == true` |

Neither function touches the filesystem or the forge. State mutation only. Callers (V5LIFECYCLE-6/7) sandwich these between the forge calls + the `ops::state::save_org` write.

## What must NOT change

- Existing fixtures and org yamls without the new fields continue to load (defaults fill in).
- A load → save roundtrip on existing yamls produces byte-identical output (no gratuitous emission of default values).
- Tier-1 regression passes.

## Acceptance criteria

1. Every existing org yaml fixture under `tests/v5/fixtures/` loads without error and round-trips byte-identically.
2. A new fixture with `lifecycle: dismissed` + `privatized_on: [github]` on one repo loads correctly and all three fields appear in `repos.list` output.
3. `ops::repo::dismiss` called on an `active` repo transitions it to `dismissed` and preserves `privatized_on` accumulation (calling twice with different sets = union).
4. `ops::repo::purge` called on an `active` repo returns `LifecycleError::NotDismissed`.
5. `ops::repo::purge` called on a `dismissed` but `protected: true` repo returns `LifecycleError::Protected`.
6. `ops::repo::purge` called on a `dismissed` non-protected repo removes it from the org's repo list.

## Completion

- Run `bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-5.sh` → exit 0.
- Status flips in-commit.
