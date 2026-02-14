# Hyperforge

Multi-forge repository management with declarative configuration.

## Overview

Hyperforge syncs repositories across GitHub, Codeberg, and GitLab using direct API calls. It maintains a local state (LocalForge) that tracks your repos and their forge configurations.

**Key features:**
- **Declarative config**: Define repos in YAML, sync to forges
- **Multi-forge**: Origin + mirrors pattern (e.g., GitHub origin, Codeberg mirror)
- **Direct API**: No Pulumi - uses ForgePort adapters for each forge
- **Workspace operations**: Diff, sync, verify across all repos

## Quick Start

```bash
# List repos for an org
synapse substrate hyperforge repos_list --org hypermemetic

# Import existing repos from GitHub
synapse substrate hyperforge repos_import --forge github --org hypermemetic

# Check what would change
synapse substrate hyperforge workspace_diff --org hypermemetic --forge github

# Apply changes
synapse substrate hyperforge workspace_sync --org hypermemetic --forge github
```

## Commands

### Repo Management

```bash
# List repos in LocalForge
synapse substrate hyperforge repos_list --org <org>

# Create a new repo
synapse substrate hyperforge repos_create \
  --org <org> \
  --name my-tool \
  --origin github \
  --visibility public \
  --mirrors codeberg

# Update repo settings
synapse substrate hyperforge repos_update \
  --org <org> \
  --name my-tool \
  --visibility private

# Delete repo from LocalForge
synapse substrate hyperforge repos_delete --org <org> --name my-tool

# Import repos from a forge
synapse substrate hyperforge repos_import --forge github --org <org>
```

### Workspace Operations

```bash
# Diff local state vs remote forge
synapse substrate hyperforge workspace_diff --org <org> --forge github

# Sync local state to remote forge
synapse substrate hyperforge workspace_sync --org <org> --forge github

# Verify workspace configuration
synapse substrate hyperforge workspace_verify --org <org>
```

### Git Operations

```bash
# Initialize a repo for multi-forge sync
synapse substrate hyperforge git_init \
  --path /path/to/repo \
  --org <org> \
  --forges "github,codeberg"

# Push to all configured forges
synapse substrate hyperforge git_push --path /path/to/repo

# Check git status
synapse substrate hyperforge git_status --path /path/to/repo
```

### Meta

```bash
# Show status
synapse substrate hyperforge status

# Show version
synapse substrate hyperforge version
```

## Configuration

Config lives in `~/.config/hyperforge/`:

```
~/.config/hyperforge/
├── config.yaml           # Global config, org definitions
└── orgs/
    └── <org>/
        └── repos.yaml    # Repo configurations for this org
```

### Global Config (`config.yaml`)

```yaml
default_org: hypermemetic
secret_provider: keychain

organizations:
  hypermemetic:
    owner: hypermemetic
    owner_type: user        # user | org (affects API endpoints)
    ssh_key: hypermemetic
    origin: github
    forges:
      - github
      - codeberg
    default_visibility: public
```

### Repo Config (`orgs/<org>/repos.yaml`)

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

## Architecture

Hyperforge is a Plexus RPC activation that integrates with substrate:

```
┌─────────────────────────────────────────┐
│  substrate (Plexus RPC server)          │
│  └─ hyperforge activation               │
│     ├─ HyperforgeHub (methods)          │
│     ├─ LocalForge (local state)         │
│     └─ ForgePort adapters               │
│        ├─ GitHubAdapter                 │
│        ├─ CodebergAdapter               │
│        └─ GitLabAdapter                 │
└─────────────────────────────────────────┘
```

### ForgePort Trait

```rust
#[async_trait]
pub trait ForgePort {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>>;
    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo>;
    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;
    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;
    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()>;
}
```

### Authentication

Tokens are retrieved from the secrets auth hub:

```bash
# Set a token
synapse secrets auth set_secret \
  --secret-key "github/<org>/token" \
  --value "<token>"

# Tokens are stored at:
# - github/<org>/token
# - codeberg/<org>/token
# - gitlab/<org>/token
```

## Sync Model

### Origin + Mirrors

Each repo has one **origin** forge and optional **mirrors**:

- **Origin**: Source of truth, where repo is primarily managed
- **Mirrors**: Read-only copies synced from origin

### Diff Operations

`workspace_diff` compares LocalForge state against a remote forge:

| Status | Meaning |
|--------|---------|
| `in_sync` | Local and remote match |
| `to_create` | Exists locally, not on remote |
| `to_update` | Metadata differs |
| `to_delete` | Marked for deletion locally |

### Sync Operations

`workspace_sync` applies local state to remote:

1. Creates repos that exist locally but not remotely
2. Updates repos where metadata differs
3. Deletes repos marked for removal

## Guides

- **[Workspace Sync Guide](docs/workspace-sync-guide.md)** — How to push an entire workspace to remote forges, create missing repos, and keep everything in sync.

## Known Issues

### User vs Org Accounts

Codeberg/GitHub have different API endpoints for user accounts vs organizations. If sync fails with "org does not exist", add `owner_type: user` to your org config.

See `docs/architecture/16676594318971935743_owner-type-enum.md` for the planned fix.

## Development

### Building

```bash
cargo build --release
```

### Testing

```bash
cargo test
cargo test --test integration_test
```

### Running Standalone

```bash
# As standalone server (port 4446)
./target/release/hyperforge

# With custom port
./target/release/hyperforge --port 8080
```

## License

AGPL-3.0-only

## Related Projects

- **plexus-core** (hub-core): Activation system infrastructure
- **plexus-transport** (hub-transport): WebSocket/stdio transport
- **plexus-macros** (hub-macro): Procedural macros for activations
- **synapse**: CLI client for Plexus RPC servers
- **substrate**: Reference Plexus RPC server with built-in activations
