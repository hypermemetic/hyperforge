# v5 Contracts — Types, Decisions, Harness

The single reference document for anything pinned across epics. Ticket
authors cite type names and decision numbers from here; they do not
redefine them.

## §types — domain vocabulary

Strong-typed newtypes and enums that appear in ticket contracts.
Tickets reference these by **name**; the ticket that first emits a value
of a type pins its **field set**. Internal struct syntax, derive list,
and module placement are the implementer's call — the name and the
public field set are the contract.

### Identifiers (newtypes over `String`)

| Name              | Constraint                                                           |
|-------------------|----------------------------------------------------------------------|
| `OrgName`         | filename-safe (no `/`, no leading `.`, ≤64 chars, ASCII)             |
| `RepoName`        | same constraint as `OrgName`                                         |
| `WorkspaceName`   | same                                                                 |
| `RemoteUrl`       | any non-empty git URL the `git` CLI would accept                     |
| `SecretRef`       | string matching `secrets://<path>` where `<path>` is any non-empty   |
| `FsPath`          | absolute path after expansion; no `..`, no trailing `/`              |
| `DomainName`      | lowercase DNS name (e.g., `github.com`, `git.internal.acme`)         |

### Enums (closed sets for v1)

| Name                 | Variants                                                     |
|----------------------|--------------------------------------------------------------|
| `ProviderKind`       | `github`, `codeberg`, `gitlab`                               |
| `CredentialType`     | `token`, `ssh_key`                                           |
| `ReconcileEventKind` | `renamed`, `removed`, `matched`, `new_matched`, `ambiguous`  |
| `SyncStatus`         | `in_sync`, `drifted`, `errored`                              |
| `DriftFieldKind`     | `default_branch`, `description`, `archived`, `visibility`    |

Unknown variants MUST be rejected at the wire boundary, not accepted and
dropped.

### Composite types

| Name                    | Field set (order not significant)                                                          |
|-------------------------|--------------------------------------------------------------------------------------------|
| `RepoRef`               | `org: OrgName`, `name: RepoName`                                                            |
| `CredentialEntry`       | `key: SecretRef | FsPath`, `type: CredentialType`                                           |
| `Remote`                | `url: RemoteUrl`, `provider?: ProviderKind` (present only when overriding domain-map)      |
| `WorkspaceRepo`         | Either string form `<org>/<name>` **or** object `{ref: RepoRef, dir: String}`              |
| `OrgSummary`            | `name: OrgName`, `provider: ProviderKind`, `repo_count: u32`                                |
| `OrgDetail`             | `name: OrgName`, `provider: ProviderKind`, `credentials: [CredentialEntry]`, `repos: [RepoName]` |
| `RepoSummary`           | `org: OrgName`, `name: RepoName`, `remote_count: u32`                                       |
| `RepoDetail`            | `ref: RepoRef`, `remotes: [Remote]`                                                         |
| `WorkspaceSummary`      | `name: WorkspaceName`, `path: FsPath`, `repo_count: u32`                                    |
| `WorkspaceDetail`       | `name: WorkspaceName`, `path: FsPath`, `repos: [WorkspaceRepo]`                             |
| `DriftField`            | `field: DriftFieldKind`, `local: Json`, `remote: Json`                                      |
| `SyncDiff`              | `ref: RepoRef`, `status: SyncStatus`, `drift: [DriftField]`                                 |
| `ReconcileEvent`        | `kind: ReconcileEventKind`, `ref?: RepoRef`, `dir?: String`, `detail?: String`              |
| `WorkspaceSyncReport`   | `name: WorkspaceName`, `total: u32`, `in_sync: u32`, `drifted: u32`, `errored: u32`, `per_repo: [SyncDiff]` |

**Serialization rule.** All newtypes serialize `#[serde(transparent)]` as
bare strings / numbers. All enums serialize as `snake_case` strings.
`WorkspaceRepo` uses serde untagged (string OR object form) so the YAML
remains human-readable.

**Secret redaction rule.** `CredentialEntry.key` serializes as-is
(`secrets://...` references are NOT secrets; the resolved value is).
No method that returns `OrgDetail` or any other type containing
`CredentialEntry` may include resolved values. Resolution happens inside
adapters only.

---

## §decisions — resolved choices

Decisions that tickets treat as given. Risks listed in the epic
overviews that are not listed here must be surfaced to epic-evaluation
as open.

| # | Decision                                                                                                                                                                                                |
|---|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| D1 | v5 daemon default port is **44105** (v4 keeps 44104). v5 registers in the Plexus registry as `lforge-v5`.                                                                                              |
| D2 | No `hyperforge-auth` sidecar. Secret store is embedded; YAML backend only in v1.                                                                                                                       |
| D3 | `ForgePort` trait pinned as **the intersection of portable metadata**: `default_branch`, `description`, `archived`, `visibility`. Provider-specific fields are per-adapter extensions, not on the trait. |
| D4 | `repos.push` **pushes to every remote in sequence**, provider-dispatched per remote. First failure aborts; already-succeeded remotes are reported in the result. Caller may pass `--remote <url>` to target one. |
| D5 | `workspaces.reconcile` ambiguity resolution: when two local dirs share a remote URL, the **first scanned wins** (alphabetical), other candidates emit `ReconcileEventKind::ambiguous` events. No auto-fix. |
| D6 | `workspaces.sync` continues past per-repo failures. Aggregate `WorkspaceSyncReport.errored` counts them; overall exit is success; the caller inspects the report for per-repo status.                 |
| D7 | `dry_run: bool = false` is a **standard parameter** on every mutating method (`create`, `delete`, `update`, `add`, `remove`, `set_*`, `push`, `delete_remote` flows). Default false.                    |
| D8 | Atomic writes. Every yaml write is "write tempfile + rename". Concurrent writers are serialized by a per-file lock inside the daemon. Tickets inherit this contract.                                    |
| D9 | Event envelope. Every event emitted by an RPC method has a top-level `type` field (snake_case string) as the discriminator. Error events have `type: "error"` with additional `code` (snake_case, drawn from the emitting ticket's closed error-class set) and `message` (free text). Non-error events use a `type` matching the category (`org_summary`, `sync_diff`, `reconcile_event`, `schema`, etc.). Test scripts match on `.type == "<category>"`; payload field names are fixed per ticket. |

---

## §harness — test harness surface (V5CORE-9 implements)

Every ticket's test script sources this surface. The surface is the
**contract**; V5CORE-9 provides the implementation. Until V5CORE-9
ships, every test script fails at `source` — that's the intended TDD
red state.

### Bash surface

Scripts live at `tests/v5/<EPIC>/<TICKET>.sh`, executable, start with:

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"
```

Helpers provided by `tests/v5/harness/lib.sh`:

| Function                              | Behavior                                                                                      |
|---------------------------------------|-----------------------------------------------------------------------------------------------|
| `hf_spawn`                            | Spawns daemon on an ephemeral port. Exports `$HF_PORT`, `$HF_CONFIG` (temp dir). Returns on ready. |
| `hf_cmd <args...>`                    | Runs `synapse -P $HF_PORT lforge hyperforge <args>`. Stdout is the RPC event stream (NDJSON). |
| `hf_load_fixture <fixture_name>`      | Copies `tests/v5/fixtures/<fixture_name>/` over `$HF_CONFIG`. Creates dirs as needed.         |
| `hf_put_secret <secret_ref> <value>`  | Writes a secret into `$HF_CONFIG/secrets.yaml` under the given `secrets://` path.             |
| `hf_assert_event <jq_filter>`         | Reads stdin (event stream), asserts at least one event matches the jq filter. Else exit 1.   |
| `hf_assert_no_event <jq_filter>`      | Same, but asserts NO event matches.                                                           |
| `hf_assert_count <jq_filter> <n>`     | Asserts exactly `n` events match.                                                             |
| `hf_teardown`                         | Kills daemon, removes `$HF_CONFIG`. Registered as EXIT trap by `hf_spawn`. Save/respawn patterns must `cp` to an external tempdir before calling. |
| `hf_add_provider_map <domain> <prov>` | Appends `<domain>: <provider>` to `$HF_CONFIG/config.yaml` under `provider_map:` (creates the block if absent). Pure bash; no yaml parser required. |
| `hf_require_tier2 [<forge>]`         | SKIP-clean exit when `$HF_V5_TEST_CONFIG_DIR` is unset / missing `tier2.env`. Sources `tier2.env` into the script environment. Optional `<forge>` (e.g. `github`) additionally skips if that forge's `ORG`/`REPO` params are blank. |
| `hf_use_test_config`                  | Overlays `$HF_V5_TEST_CONFIG_DIR/` (minus `tier2.env`) onto `$HF_CONFIG/`. Preserves subdirs. Call after `hf_spawn`. Gives the daemon the same `config.yaml` / `orgs/*.yaml` / `secrets.yaml` shape the user would have on `~/.config/hyperforge/`. |

### Rust runner

`tests/v5_integration.rs` (V5CORE-9 writes) iterates scripts under
`tests/v5/*/` (excluding `harness/` and `fixtures/`) and runs each as
one `#[test]` with a tier tag. Tiers from v5/README.md:

- **Tier 1 (offline):** default `cargo test --test v5_integration`.
- **Tier 2 (live forge):** `cargo test --test v5_integration --features tier2`.
- **Tier 3 (orchestrated):** `cargo test --test v5_integration --features tier3`.

Each script self-declares its tier via a magic comment on line 2:
`# tier: 1` (default if omitted).

### Fixtures

Shared YAML fixtures at `tests/v5/fixtures/<name>/`. Each fixture is a
drop-in replacement for `$HF_CONFIG`:

```
tests/v5/fixtures/<name>/
├── config.yaml
├── orgs/<org>.yaml
├── workspaces/<ws>.yaml
└── secrets.yaml (optional)
```

Fixtures are committed alongside tickets that introduce them. A ticket
that needs a new fixture owns it; no shared "grab-bag" fixture.

---

## §anti-patterns — what tickets MUST NOT do

Read the ticketing skill's "Capabilities, not implementations" and apply
these project-specific reinforcements:

- **Never** name a Rust module or file path in a ticket body. The layout
  is the implementer's.
- **Never** write a `#[derive(...)]` list in a ticket. Derive sets are
  implementation.
- **Never** pin JSON-RPC method names in stone unless the CLI shape is
  the contract (for root-level public methods it is).
- **Never** describe "internal state" — only the wire-observable
  behavior matters for acceptance.
- **Never** let acceptance criteria reference a source line, function,
  or struct; they reference the test script's observables.
