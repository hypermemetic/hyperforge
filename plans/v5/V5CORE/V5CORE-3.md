---
id: V5CORE-3
title: "YAML config loaders and round-trip for the three schemas"
status: Complete
type: implementation
blocked_by: [V5CORE-2]
unlocks: [V5CORE-10, V5ORGS-1, V5REPOS-1, V5WS-1]
---

## Problem

The daemon has no way to read its on-disk config. Downstream epics need
typed views of `config.yaml`, per-org files, and per-workspace files that
load losslessly and survive a round-trip (load → re-serialize → deep-equal).

## Required behavior

Three schemas, each rooted at a path relative to `$HF_CONFIG`:

| File                       | Top-level fields                                                                 |
|----------------------------|----------------------------------------------------------------------------------|
| `config.yaml`              | `default_workspace?: WorkspaceName`, `provider_map: { DomainName: ProviderKind }` |
| `orgs/<OrgName>.yaml`      | `name: OrgName`, `forge: { provider: ProviderKind, credentials: [CredentialEntry] }`, `repos: [{name: RepoName, remotes: [Remote]}]` |
| `workspaces/<WorkspaceName>.yaml` | `name: WorkspaceName`, `path: FsPath`, `repos: [WorkspaceRepo]`             |

All newtypes and composites come from CONTRACTS §types. `WorkspaceRepo`
uses untagged serde (string shorthand OR object form per §types). Unknown
top-level fields are a hard error, not silently dropped (closed-variant
rule from §types applies to structs too).

Loader inputs (one per schema):

| Input | Type | Required | Notes |
|---|---|---|---|
| file contents | UTF-8 YAML | yes | syntax error → `InvalidYaml` error |
| file path basename | depends on schema | yes | `orgs/<OrgName>.yaml` basename MUST equal the in-file `name` |

Loader outputs:

| Output | Shape | Notes |
|---|---|---|
| Success | typed value | the composite for that schema |
| Failure | error with file path and reason | never silent |

Writer output:

| Output | Shape | Notes |
|---|---|---|
| YAML bytes | deterministic ordering | re-parsing yields value equal to input |

Edge cases:

- Missing `config.yaml`: treated as an empty config (no default workspace, empty provider map). Not an error.
- Missing `orgs/` or `workspaces/` directory: treated as zero orgs / zero workspaces.
- `orgs/<OrgName>.yaml` basename mismatches the in-file `name`: hard error naming the file.
- Unknown `ProviderKind`, `CredentialType`, or top-level field: hard error at the wire boundary.

## What must NOT change

- v4 config files under the same directory are neither read nor written by these loaders.
- Writes follow D8 (atomic: write temp + rename).

## Acceptance criteria

1. Loading `tests/v5/fixtures/empty/` yields a parsed config with no default workspace, an empty provider map, zero orgs, zero workspaces.
2. Loading `tests/v5/fixtures/minimal_org/` yields exactly one `OrgDetail`-equivalent value whose `name`, `provider`, `credentials`, and `repos` match the fixture verbatim.
3. Round-trip: for every fixture, loading then serializing then re-loading yields a value deep-equal to the first load.
4. Corrupting `config.yaml` with invalid YAML produces a load error whose message names `config.yaml`.
5. A fixture with unknown top-level field produces a load error that names the offending field.
6. A fixture with `orgs/foo.yaml` whose in-file `name: bar` produces a load error that names both `foo` and `bar`.

## Completion

- Run `bash tests/v5/V5CORE/V5CORE-3.sh` → exit 0.
- Status flips in-commit with the implementation.
