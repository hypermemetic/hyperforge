---
id: V5PROV-2
title: "ForgePort trait: add create_repo, delete_repo, repo_exists"
status: Complete
type: implementation
blocked_by: []
unlocks: [V5PROV-3, V5PROV-4, V5PROV-5, V5PROV-6, V5PROV-7, V5PROV-8]
---

## Problem

Per D10 (revised D3), `ForgePort` must expose lifecycle methods alongside
the metadata intersection. V5REPOS-2's trait is the metadata-only surface;
this ticket adds the three lifecycle methods every provisioning flow
depends on.

## Required behavior

Three new capability methods on the `ForgePort` trait. Existing methods
(read_metadata, write_metadata) are unchanged.

| Capability | Inputs | Outputs |
|---|---|---|
| **create_repo** | `RepoRef`, `ProviderVisibility`, `description: String`, `ForgeAuth` | `Ok(())` on success; typed `ForgePortError` on failure |
| **delete_repo** | `RepoRef`, `ForgeAuth` | `Ok(())` on success; typed error on failure |
| **repo_exists** | `RepoRef`, `ForgeAuth` | `Ok(bool)` — true when the remote repo exists and is reachable with these credentials |

Error classes (closed set for all three methods): `not_found`, `auth`,
`network`, `rate_limited`, `unsupported_visibility` (new — create_repo
only; raised when the visibility variant is not supported by the
provider).

Edge cases:
- `create_repo` on an already-existing repo → `ForgePortError { class: conflict }` (new class — only for create_repo). Callers distinguish from `not_found` by the class.
- `delete_repo` on a missing repo → `ForgePortError { class: not_found }` (not silent success).
- `repo_exists` distinguishes `Ok(false)` (repo missing) from `Err(ForgePortError { class: auth })` (credential can't even check).

## What must NOT change

- V5REPOS-2's read_metadata + write_metadata signatures.
- The existing four-field DriftFieldKind intersection.
- Adapter ticket contracts (V5REPOS-9/10/11) — those must grow to implement the three new methods; this ticket pins what they must grow to.

## Acceptance criteria

1. Schema introspection (via V5REPOS-2's `forge_port_schema` or equivalent) now reports three additional method names: `create_repo`, `delete_repo`, `repo_exists`.
2. The closed error set (discoverable via the schema surface or via V5PROV-9's checkpoint) contains the original five plus `conflict` (create-only) and `unsupported_visibility` (create-only).
3. No adapter can pass its tier-2 script (V5PROV-3/4/5) without implementing the three methods with the pinned signatures.
4. `repo_exists` never mutates forge state — a successful call leaves the remote byte-identical, verifiable by calling twice and asserting the second returns the same `Ok(bool)` without any state-change side-effect.

## Completion

- Run `bash tests/v5/V5PROV/V5PROV-2.sh` → exit 0.
- Status flips in-commit with the implementation.
