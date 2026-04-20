# Hyperforge

Multi-forge repository management with declarative configuration.

## Overview

Hyperforge syncs repositories across GitHub, Codeberg, and GitLab using direct API calls. It maintains a local state (LocalForge) that tracks your repos and their forge configurations.

**Key features:**
- **Declarative config**: Define repos in YAML, sync to forges
- **Multi-forge**: Origin + mirrors pattern (e.g., GitHub origin, Codeberg mirror)
- **Direct API**: No Pulumi — uses `ForgePort` adapters per forge
- **Workspace operations**: Discover, diff, sync, verify across all repos
- **Build pipeline**: Unify cross-repo manifests, publish to crates.io/Homebrew/binstall, bump versions

## Binaries

Hyperforge ships three binaries (see `Cargo.toml`):

| Binary | Default port | Role |
|---|---|---|
| `hyperforge` | 44104 | Main hub: `repo.*`, `workspace.*`, `build.*`, auth/org/config |
| `hyperforge-auth` | 4445 | Secrets sidecar (YAML-backed secret store, namespace: `secrets`) |
| `hyperforge-ssh` | — | SSH key management CLI |

`hyperforge` auto-starts `hyperforge-auth` as a sidecar unless `--no-auth-sidecar` is passed.

## Quick Start

```bash
# List repos for an org
synapse substrate hyperforge repo list --org hypermemetic

# Import existing repos from GitHub
synapse substrate hyperforge repo import --forge github --org hypermemetic

# Preview what would change
synapse substrate hyperforge workspace diff --org hypermemetic --forge github

# Apply changes
synapse substrate hyperforge workspace sync --org hypermemetic --forge github
```

## Commands

Hyperforge routes into three sub-activations (`repo`, `workspace`, `build`) plus root-level auth/config/org methods. All examples use `synapse substrate hyperforge …` — replace with `synapse -P 44104 lforge hyperforge …` if running standalone.

### Root: Auth, Orgs, Config

```bash
# Service status / version
synapse substrate hyperforge status

# Reload config from disk
synapse substrate hyperforge reload

# Orgs
synapse substrate hyperforge orgs_list
synapse substrate hyperforge orgs_add    --org <org> --ssh.github ~/.ssh/gh --workspace_path /path/to/ws
synapse substrate hyperforge orgs_update --org <org> --ssh.codeberg ~/.ssh/cb   # default merge; add --replace true to replace
synapse substrate hyperforge orgs_delete --org <org>

# Config
synapse substrate hyperforge config_show
synapse substrate hyperforge config_set_ssh_key --org <org> --forge github --key <path>

# Auth
synapse substrate hyperforge auth_requirements --org <org>
synapse substrate hyperforge auth_setup           # guided token setup
synapse substrate hyperforge auth_check --org <org>

# Onboarding entrypoint
synapse substrate hyperforge begin
```

### `repo.*` — Single-Repo Operations

Registry CRUD + single-repo git:

```bash
# Registry (LocalForge records)
synapse substrate hyperforge repo list   --org <org>
synapse substrate hyperforge repo create --org <org> --name my-tool \
  --origin github --visibility public --mirrors codeberg
synapse substrate hyperforge repo update --org <org> --name my-tool --visibility private
synapse substrate hyperforge repo delete --org <org> --name my-tool
synapse substrate hyperforge repo purge  --org <org> --name my-tool   # remove from all forges + local
synapse substrate hyperforge repo rename --org <org> --name my-tool --new-name better-tool
synapse substrate hyperforge repo set_archived       --org <org> --name my-tool --archived true
synapse substrate hyperforge repo set_default_branch --org <org> --name my-tool --branch main
synapse substrate hyperforge repo import --forge github --org <org>

# Single-repo git
synapse substrate hyperforge repo init   --path /path/to/repo --org <org> --forges "github,codeberg"
synapse substrate hyperforge repo status --path /path/to/repo
synapse substrate hyperforge repo push   --path /path/to/repo
synapse substrate hyperforge repo clone  --org <org> --name my-tool --dest /path/to/checkout
synapse substrate hyperforge repo sync   --path /path/to/repo   # pull from origin, push to mirrors

# Inspection
synapse substrate hyperforge repo dirty       --path /path/to/repo
synapse substrate hyperforge repo size        --path /path/to/repo
synapse substrate hyperforge repo loc         --path /path/to/repo
synapse substrate hyperforge repo large_files --path /path/to/repo
```

### `workspace.*` — Multi-Repo Orchestration

Operates over every repo in a workspace directory:

```bash
synapse substrate hyperforge workspace discover --path /path/to/workspace
synapse substrate hyperforge workspace init     --path /path/to/workspace --org <org>
synapse substrate hyperforge workspace check    --path /path/to/workspace
synapse substrate hyperforge workspace diff     --path /path/to/workspace --org <org> --forge github
synapse substrate hyperforge workspace sync     --path /path/to/workspace --org <org> --forge github
synapse substrate hyperforge workspace verify   --org <org>
synapse substrate hyperforge workspace push_all --path /path/to/workspace
synapse substrate hyperforge workspace clone    --org <org> --dest /path/to/workspace
synapse substrate hyperforge workspace move_repos            --from /old --to /new
synapse substrate hyperforge workspace set_default_branch    --org <org> --branch main
synapse substrate hyperforge workspace check_default_branch  --org <org>
```

`workspace sync` is the main workhorse — it discovers, registers, imports remote-only repos, diffs, creates missing, updates metadata, and pushes. See the [Workspace Sync Guide](docs/workspace-sync-guide.md).

### `build.*` — Build, Release, Distribution

Cross-repo Cargo/manifest ops, binary distribution, version bumping:

```bash
# Manifest unification + analysis
synapse substrate hyperforge build unify                  --path /path/to/workspace
synapse substrate hyperforge build analyze                --path /path/to/workspace
synapse substrate hyperforge build detect_name_mismatches --path /path/to/workspace
synapse substrate hyperforge build package_diff           --path /path/to/workspace
synapse substrate hyperforge build validate               --path /path/to/workspace

# Release pipeline
synapse substrate hyperforge build bump        --path /path/to/repo --level patch
synapse substrate hyperforge build publish     --path /path/to/workspace
synapse substrate hyperforge build release     --path /path/to/repo
synapse substrate hyperforge build release_all --path /path/to/workspace

# Execution / runtime
synapse substrate hyperforge build run  --path /path/to/repo
synapse substrate hyperforge build exec --path /path/to/repo -- <args>

# Distribution channels
synapse substrate hyperforge build init_configs   --path /path/to/workspace
synapse substrate hyperforge build binstall_init  --path /path/to/repo
synapse substrate hyperforge build brew_formula   --path /path/to/repo
synapse substrate hyperforge build dist_init      --path /path/to/repo
synapse substrate hyperforge build dist_show      --path /path/to/repo

# Hygiene / inspection across workspace
synapse substrate hyperforge build gitignore_sync --path /path/to/workspace
synapse substrate hyperforge build large_files    --path /path/to/workspace
synapse substrate hyperforge build repo_sizes     --path /path/to/workspace
synapse substrate hyperforge build loc            --path /path/to/workspace
synapse substrate hyperforge build dirty          --path /path/to/workspace
```

## Configuration

Hyperforge has two layers of config: **global** (per-machine) and **per-repo**.

### Global Config

```
~/.config/hyperforge/
├── config.yaml           # Global config, org definitions
├── secrets.yaml          # Managed by hyperforge-auth (do not hand-edit when sidecar running)
└── orgs/
    └── <org>/
        └── repos.yaml    # LocalForge repo records for this org
```

**`config.yaml`:**

```yaml
default_org: hypermemetic
secret_provider: keychain

organizations:
  hypermemetic:
    owner: hypermemetic
    owner_type: user        # user | org — affects API endpoints
    ssh_key: hypermemetic
    origin: github
    forges:
      - github
      - codeberg
    default_visibility: public
```

**`orgs/<org>/repos.yaml`** (LocalForge state — usually edited via `repo.*` methods, not by hand):

```yaml
repos:
  - name: my-tool
    origin: github
    visibility: public
    description: "My awesome tool"
    mirrors:
      - codeberg
    protected: false
```

### Per-Repo Config

`repo init` creates a `.hyperforge/config.toml` inside the git repo:

```toml
repo_name = "hyperforge"
org = "hypermemetic"
forges = ["github", "codeberg"]
visibility = "public"
default_branch = "main"

[ssh]
github = "/home/user/.ssh/hypermemetic"

[forge.github]
# per-forge overrides

[ci]
# CI config

[dist]
# distribution config (binstall / homebrew / etc.)
```

SSH keys are wired per-repo via git's `core.sshCommand` (no global `~/.ssh/config` edits).

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  synapse (CLI client) / substrate (Plexus RPC server)           │
│                                                                  │
│  └─ hyperforge (port 44104) ────── hyperforge-auth (port 4445)  │
│     ├─ HyperforgeHub (root)              ns: secrets            │
│     │  ├─ RepoHub      (repo.*)                                 │
│     │  ├─ WorkspaceHub (workspace.*)                            │
│     │  └─ BuildHub     (build.*)                                │
│     ├─ LocalForge  (YAML persistence)                           │
│     └─ ForgePort adapters                                       │
│        ├─ GitHubAdapter                                         │
│        ├─ CodebergAdapter (Gitea-compatible)                    │
│        └─ GitLabAdapter                                         │
└─────────────────────────────────────────────────────────────────┘
```

`HyperforgeHub` is a Plexus `Activation` + `ChildRouter`: root-level methods (auth/orgs/config/status) live on the hub itself; `repo.*`, `workspace.*`, `build.*` are routed to child activations.

### `ForgePort` Trait

```rust
#[async_trait]
pub trait ForgePort {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>>;
    async fn list_repos_incremental(&self, org: &str, etag: Option<String>)
        -> ForgeResult<(Vec<Repo>, Option<String>)>;
    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo>;
    async fn repo_exists(&self, org: &str, name: &str) -> ForgeResult<bool>;
    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;
    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;
    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()>;
    async fn rename_repo(&self, org: &str, old: &str, new: &str) -> ForgeResult<()>;
    async fn set_default_branch(&self, org: &str, name: &str, branch: &str) -> ForgeResult<()>;
    async fn set_archived(&self, org: &str, name: &str, archived: bool) -> ForgeResult<()>;
}
```

See `src/adapters/forge_port.rs`.

### Authentication

Tokens are stored in `hyperforge-auth` (namespace `secrets`) and retrieved on demand:

```bash
# Set a token
synapse secrets auth set_secret \
  --secret_key "github/<org>/token" \
  --value "<token>"

# Paths used by hyperforge:
#   github/<org>/token
#   codeberg/<org>/token
#   gitlab/<org>/token
```

Guided setup via `auth_setup` walks you through this interactively.

## Sync Model

### Origin + Mirrors

Each repo has one **origin** forge and optional **mirrors**:

- **Origin**: source of truth
- **Mirrors**: synced from origin

### Diff

`workspace diff` compares LocalForge state against a remote forge:

| Status | Meaning |
|--------|---------|
| `in_sync` | Local and remote match |
| `to_create` | Exists locally, not on remote |
| `to_update` | Metadata differs |
| `to_delete` | Marked for deletion locally |

### Sync

`workspace sync` applies local state to remote: creates missing repos, updates metadata, deletes marked repos, and pushes git content. Full eight-phase pipeline in [docs/workspace-sync-guide.md](docs/workspace-sync-guide.md).

## Guides & Architecture Docs

- [Workspace Sync Guide](docs/workspace-sync-guide.md) — 8-phase `workspace sync` pipeline
- [Context Passing](docs/architecture-context-passing.md) — how hubs share context
- [Workspace Hierarchy](docs/architecture-workspace-hierarchy.md) — proposed workspace model
- [Declarative IaC Status](docs/declarative-iac-status.md) — declarative workflow state
- [SSH Config Migration](docs/ssh-config-migration.md) — per-repo SSH keys
- [Testing Declarative IaC](docs/testing-declarative-iac.md)
- [Testing Status](docs/testing-status.md)
- [Container Session](docs/CONTAINER-SESSION.md)
- Architecture proposals: [`docs/architecture/`](docs/architecture/)

See [`plans/`](plans/) for epic-level roadmap (AUTH, DIST, WORK, CONF, CLEANUP).

## Development

### Building

```bash
cargo build --release
```

### Testing

```bash
cargo test
cargo test --test integration_test
cargo test --test build_hub_test
```

### Running Standalone

```bash
# Main hub (port 44104 by default)
./target/release/hyperforge

# Custom port, no sidecar
./target/release/hyperforge --port 8080 --no-auth-sidecar

# Stdio mode (for MCP)
./target/release/hyperforge --stdio

# Auth sidecar standalone
./target/release/hyperforge-auth --port 4445
```

## Related Projects

- **plexus-core**: Activation system infrastructure
- **plexus-transport**: WebSocket / stdio transport
- **plexus-macros**: Procedural macros for activations
- **synapse**: CLI client for Plexus RPC servers
- **substrate**: Reference Plexus RPC server with built-in activations

## License

MIT (see `Cargo.toml`)
