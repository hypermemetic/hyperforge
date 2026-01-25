# Hyperforge (LFORGE2)

Multi-forge repository management with repo-local configuration and git-native workflows.

## Overview

Hyperforge is a declarative repository management system that syncs repositories across GitHub, Codeberg, and other forges. LFORGE2 is a complete redesign focusing on:

- **Repo-local configuration**: Each repo has `.hyperforge/config.toml` defining its forge settings
- **Git-native**: Uses standard git config and remotes, no SSH host aliases
- **Workspace emergence**: Workspaces are discovered from directory structure, not predefined
- **Direct API calls**: No Pulumi - uses SymmetricSyncService with ForgePort adapters

## Architecture

Hyperforge is built on the hub-core activation system:

```
┌─────────────────────────────────────────────────────────┐
│  DynamicHub (namespace: "lforge")                       │
│  ├─ Provides .call() routing for CLI tools              │
│  └─ Registers activations dynamically                   │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│  HyperforgeHub (namespace: "hyperforge")                │
│  ├─ status()  - Show version and status                 │
│  ├─ version() - Version information                     │
│  └─ [future methods for repo management]                │
└─────────────────────────────────────────────────────────┘
```

**Transport Layer**: hub-transport provides:
- WebSocket server (JSON-RPC)
- stdio mode (MCP-compatible)
- MCP HTTP server (optional)

**Calling Pattern**: `synapse -p <port> lforge hyperforge <method>`
- `lforge` = DynamicHub namespace (the backend)
- `hyperforge` = activation namespace (the plugin)
- `<method>` = method to call (status, version, etc.)

## Installation

### Building from Source

```bash
# Clone the repository
git clone <repo-url>
cd hyperforge-lforge2

# Build
cargo build --release

# The binary will be at target/release/hyperforge
```

### Running the Server

```bash
# WebSocket mode (default port 4446)
./target/release/hyperforge

# Custom port
./target/release/hyperforge --port 8080

# With MCP HTTP server (on port + 1)
./target/release/hyperforge --mcp

# stdio mode (for MCP integration)
./target/release/hyperforge --stdio
```

## Usage

### Standalone Server

Start the server:

```bash
./target/release/hyperforge
```

Output:
```
LFORGE2 initialized
  Namespace: lforge
  Activation: hyperforge
  Version: 2.0.0
  Description: Multi-forge repository management

LFORGE2 started
  WebSocket: ws://127.0.0.1:4446

Usage:
  synapse -p 4446 lforge hyperforge status
  synapse -p 4446 lforge hyperforge version
```

### Using with Synapse

Check status:
```bash
synapse -p 4446 lforge hyperforge status
```

Returns:
```json
{
  "type": "status",
  "version": "2.0.0",
  "description": "Multi-forge repository management (LFORGE2)"
}
```

Get version:
```bash
synapse -p 4446 lforge hyperforge version
```

Returns:
```json
{
  "type": "info",
  "message": "hyperforge 2.0.0 (LFORGE2 - repo-local, git-native)"
}
```

### Plugin Mode

Hyperforge can be registered as a plugin in other DynamicHub-based systems:

```rust
use hub_core::plexus::DynamicHub;
use hyperforge::HyperforgeHub;
use std::sync::Arc;

// Register hyperforge in your hub
let hub = Arc::new(
    DynamicHub::new("myapp")
        .register(HyperforgeHub::new())
);

// Now callable via: myapp.hyperforge.status
let stream = hub.route("hyperforge.status", serde_json::json!({})).await?;
```

## Development

### Running Tests

```bash
# All tests
cargo test

# Integration tests only
cargo test --test integration_test

# Specific test
cargo test test_hyperforge_as_plugin
```

### Test Coverage

- `test_hyperforge_as_plugin`: Verifies DynamicHub integration and routing
- `test_hyperforge_version_method`: Tests version method returns correct data
- `test_dynamic_hub_lists_hyperforge`: Validates activation listing/introspection

### Architecture Patterns

**hub-macro**: Generates Activation trait implementation from `#[hub_methods]` attribute:

```rust
#[hub_methods(
    namespace = "hyperforge",
    version = "2.0.0",
    description = "Multi-forge repository management",
    crate_path = "hub_core"
)]
impl HyperforgeHub {
    #[hub_method(description = "Show status")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        // Implementation
    }
}
```

**DynamicHub wrapper**: Even for single-activation servers, DynamicHub provides:
- `.call()` routing method expected by synapse
- Activation listing/introspection
- Schema generation
- Consistent plugin interface

**Event-based API**: All methods return `Stream<Item = HyperforgeEvent>`:
- Enables progressive updates
- Natural for long-running operations
- Compatible with JSON-RPC streaming

## Design Principles

### 1. Repo-Local Configuration

Each repository has `.hyperforge/config.toml`:

```toml
[forge]
origin = "github"           # Primary forge
mirrors = ["codeberg"]      # Optional mirrors
visibility = "public"

[metadata]
description = "My awesome project"
topics = ["rust", "cli"]
```

No global workspace configuration - settings travel with the repo.

### 2. Git-Native Approach

Use standard git features:

```bash
# Configure forge credentials via git config
git config forge.github.token <token>
git config forge.codeberg.token <token>

# Remotes are standard git remotes
git remote -v
# origin   git@github.com:user/repo.git (fetch)
# codeberg git@codeberg.org:user/repo.git (fetch)
```

No SSH host aliases or custom git protocols.

### 3. Emergent Workspaces

Workspaces are discovered, not configured:

```
~/dev/projects/
  ├─ tool1/.hyperforge/     # Workspace member
  ├─ tool2/.hyperforge/     # Workspace member
  └─ library1/.hyperforge/  # Workspace member
```

Run `hyperforge workspace sync --path ~/dev/projects` to sync all repos in that directory tree.

### 4. Origin-Based Sync

Each repo has one **origin** forge (source of truth) and optional **mirrors**:

- **Import**: Discover repos from forges → add to local state
- **Sync**: Apply local state → forges
  - Ensure repo exists on origin
  - Mirror to configured mirrors
  - Delete from forges if marked for removal

### 5. Direct API Integration

No Pulumi subprocess - direct Rust API calls via ForgePort adapters:

```rust
pub trait ForgePort {
    async fn list_repos(&self, org: &str) -> Result<Vec<Repo>>;
    async fn create_repo(&self, org: &str, repo: &Repo) -> Result<()>;
    async fn update_repo(&self, org: &str, repo: &Repo) -> Result<()>;
    async fn delete_repo(&self, org: &str, name: &str) -> Result<()>;
}
```

Implementations: `GitHubAdapter`, `CodebergAdapter`, `LocalForge` (repo-local state).

## Roadmap

### Current (v2.0.0)

- ✅ Hub-transport integration
- ✅ Basic activation with status/version
- ✅ Plugin mode support
- ✅ Integration tests

### In Progress (LFORGE2-* tickets)

- [ ] LFORGE2-1: Core module structure
- [ ] LFORGE2-2: LocalForge implementing ForgePort
- [ ] LFORGE2-3: Repo type with origin/mirrors
- [ ] LFORGE2-4: SymmetricSyncService with origin logic
- [ ] LFORGE2-5: LocalForge ↔ repos.yaml persistence
- [ ] LFORGE2-6: Wire activations to sync service

### Future

- Multi-org support
- Workspace commands (diff, sync, import, clone_all)
- Repo management (create, update, remove)
- Package management (npm, cargo, etc.)

## License

AGPL-3.0-only

## Related Projects

- **hub-core**: Activation system core infrastructure
- **hub-transport**: Transport layer (WebSocket, stdio, MCP HTTP)
- **hub-macro**: Procedural macros for activation generation
- **synapse**: CLI client for hub-based systems
