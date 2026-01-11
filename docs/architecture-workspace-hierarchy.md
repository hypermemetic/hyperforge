# Workspace Hierarchy Architecture

**Status:** Proposal
**Date:** 2026-01-09

## Current State

The current structure is org-centric:
```
hyperforge/
├── org/
│   ├── import      # imports repos for ONE org
│   ├── <org>/
│   │   └── repos/
│   │       ├── diff
│   │       ├── sync
│   │       ├── clone
│   │       └── clone_all
├── workspace/
│   ├── list        # list bindings
│   ├── show        # resolve cwd -> org
│   ├── bind        # bind path to org
│   └── unbind
└── forge/
    └── (direct forge operations)
```

**Problems:**
1. Workspace is an afterthought, not the entry point
2. No `workspace import` or `workspace sync`
3. Org-level operations require knowing the org name
4. No token lifecycle management
5. Forge sync (mirroring) is implicit, not explicit

---

## Proposed Hierarchy

```
workspace (local dir)
    │
    ├── bound to [org1, org2, ...]
    │
    └── org
         │
         ├── owner: "hypermemetic"
         ├── forges: [github, codeberg]
         ├── sync: true  # mirror across forges
         │
         └── forge
              │
              ├── key: "hypermemetic" (SSH key name)
              ├── token: keychain ref
              ├── token_status: valid | expired | missing
              │
              └── repos: [substrate, hyperforge, ...]
```

### Dependency Flow

```
workspace -> [org] -> [forge] -> [repo]
     │          │         │
     │          │         └── token validation
     │          │
     │          └── sync policy (mirror or not)
     │
     └── import/sync operations cascade down
```

---

## Data Model Changes

### config.yaml (proposed)

```yaml
workspaces:
  ~/dev/controlflow/hypermemetic:
    orgs: [hypermemetic]           # list of orgs, not single
  ~/dev/controlflow/juggernautlabs:
    orgs: [juggernautlabs]

organizations:
  hypermemetic:
    owner: hypermemetic
    forges:
      github:
        key: hypermemetic          # SSH key name -> github-hypermemetic
        sync: true                 # mirror to this forge
      codeberg:
        key: hypermemetic
        sync: true
    origin: codeberg               # primary/source of truth
    default_visibility: public

  juggernautlabs:
    owner: juggernautlabs
    forges:
      github:
        key: juggernautlabs
        sync: true                 # only one forge, sync is no-op
    origin: github
    default_visibility: public
```

### Token State (tracked separately)

```yaml
# ~/.config/hyperforge/tokens.yaml (or in keychain metadata)
tokens:
  hypermemetic:
    github:
      status: valid
      last_checked: 2026-01-09T10:00:00Z
    codeberg:
      status: expired
      last_checked: 2026-01-09T09:00:00Z
      error: "401 Unauthorized"
```

---

## Command Hierarchy (Proposed)

### Workspace Commands (Entry Point)

```bash
# From any directory
synapse plexus hyperforge workspace import
# 1. Resolves cwd -> workspace -> [orgs]
# 2. For each org, imports repos from all forges
# 3. Validates tokens, marks expired ones
# 4. Creates repos.yaml with _synced state

synapse plexus hyperforge workspace sync
# 1. Resolves cwd -> workspace -> [orgs]
# 2. For each org with sync:true forges
# 3. Ensures repos exist on all forges
# 4. Optionally: git push to all remotes

synapse plexus hyperforge workspace diff
# Shows diff for all orgs in workspace

synapse plexus hyperforge workspace clone_all
# Clones all repos for all orgs to workspace dir
```

### Org Commands (Granular)

```bash
# Explicit org operations (same as today, still useful)
synapse plexus hyperforge org hypermemetic repos diff
synapse plexus hyperforge org hypermemetic repos sync
```

### Forge Commands (Direct)

```bash
# Direct forge operations
synapse plexus hyperforge forge github auth      # check token
synapse plexus hyperforge forge github repos     # list repos
synapse plexus hyperforge forge github refresh   # refresh token
```

---

## Code Structure (Proposed)

```
hyperforge/src/
├── types/
│   ├── mod.rs
│   ├── workspace.rs      # Workspace, WorkspaceBinding
│   ├── org.rs            # Org, OrgConfig (with forge configs)
│   ├── forge.rs          # Forge enum, ForgeConfig, TokenStatus
│   ├── repo.rs           # RepoConfig, RepoSummary
│   └── secret.rs
│
├── storage/
│   ├── mod.rs
│   ├── paths.rs          # HyperforgePaths
│   ├── config.rs         # GlobalConfig (load/save)
│   ├── org_storage.rs    # OrgStorage (repos.yaml per org)
│   ├── token_storage.rs  # TokenStorage (track token status)
│   └── workspace_storage.rs  # NEW: workspace-level state
│
├── bridge/
│   ├── mod.rs
│   ├── keychain.rs       # KeychainBridge
│   ├── pulumi.rs         # PulumiBridge
│   ├── git_remote.rs     # GitRemoteBridge
│   ├── github.rs         # NEW: GitHub API client
│   └── codeberg.rs       # NEW: Codeberg API client
│
├── activations/
│   ├── mod.rs
│   ├── workspace/        # NEW: expanded
│   │   ├── mod.rs
│   │   ├── activation.rs # import, sync, diff, clone_all
│   │   └── events.rs     # WorkspaceEvent types
│   ├── org/
│   │   ├── mod.rs
│   │   ├── activation.rs
│   │   ├── child_router.rs
│   │   └── events.rs     # OrgEvent types
│   ├── repos/
│   │   ├── mod.rs
│   │   ├── activation.rs
│   │   └── events.rs     # RepoEvent types
│   ├── forge/
│   │   ├── mod.rs
│   │   ├── activation.rs # auth, repos, refresh
│   │   └── events.rs     # ForgeEvent types
│   └── secrets/
│       └── ...
│
└── events.rs             # Re-exports all events (or remove in favor of per-activation)
```

---

## Key Abstractions

### 1. ForgeClient Trait

```rust
#[async_trait]
trait ForgeClient {
    async fn authenticate(&self, token: &str) -> Result<AuthStatus, ForgeError>;
    async fn list_repos(&self, owner: &str) -> Result<Vec<ForgeRepo>, ForgeError>;
    async fn create_repo(&self, name: &str, config: &RepoConfig) -> Result<ForgeRepo, ForgeError>;
    async fn delete_repo(&self, name: &str) -> Result<(), ForgeError>;
}

impl ForgeClient for GitHubClient { ... }
impl ForgeClient for CodebergClient { ... }
```

### 2. TokenManager

```rust
impl TokenManager {
    async fn get_token(&self, org: &str, forge: &Forge) -> Result<String, TokenError>;
    async fn validate_token(&self, org: &str, forge: &Forge) -> TokenStatus;
    async fn mark_expired(&self, org: &str, forge: &Forge, error: &str);
    async fn refresh_token(&self, org: &str, forge: &Forge) -> Result<String, TokenError>;
}
```

### 3. SyncPolicy

```rust
enum SyncPolicy {
    Mirror,      // All forges must have same repos
    Primary,     // Only origin forge is authoritative
    Manual,      // No automatic sync
}

impl OrgConfig {
    fn sync_policy(&self) -> SyncPolicy;
    fn should_sync(&self, forge: &Forge) -> bool;
}
```

---

## Workflow Examples

### 1. New Machine Setup

```bash
cd ~/dev/controlflow/hypermemetic

# Bind workspace to org
synapse plexus hyperforge workspace bind --org hypermemetic

# Import all repos (validates tokens, creates repos.yaml)
synapse plexus hyperforge workspace import

# Clone everything
synapse plexus hyperforge workspace clone_all

# Result: all repos cloned with correct remotes
```

### 2. Daily Workflow

```bash
cd ~/dev/controlflow/hypermemetic

# Check status
synapse plexus hyperforge workspace diff

# If out of sync (new repo on forge, or local changes)
synapse plexus hyperforge workspace sync --yes
```

### 3. Token Expired

```bash
# Operation fails with 401
synapse plexus hyperforge workspace import
# Error: Token expired for hypermemetic/codeberg

# Check token status
synapse plexus hyperforge forge codeberg auth --org hypermemetic
# Status: expired, last_error: "401 Unauthorized"

# Refresh (interactive or via env var)
synapse plexus hyperforge forge codeberg refresh --org hypermemetic
```

---

## Migration Path

### Phase 1: Restructure Code
1. Split `events.rs` into per-activation event files
2. Create `bridge/github.rs` and `bridge/codeberg.rs`
3. Add `TokenStorage` for tracking token status

### Phase 2: Expand Workspace
1. Add `workspace import` (delegates to org import)
2. Add `workspace sync` (delegates to org sync)
3. Add `workspace diff` (aggregates org diffs)
4. Add `workspace clone_all` (delegates to org clone_all)

### Phase 3: Token Lifecycle
1. Wrap all API calls with token validation
2. On 401, mark token expired in IAC
3. Add `forge auth` and `forge refresh` commands

### Phase 4: Sync Policy
1. Add `sync: true/false` per forge in config
2. Implement mirror logic in `repos sync`
3. Add git remote sync (push to all remotes)

---

## Open Questions

1. **Workspace with multiple orgs** - Should `workspace import` import all orgs, or require explicit selection?

2. **Token refresh flow** - Interactive prompt vs environment variable vs OAuth flow?

3. **Git content sync** - Should `workspace sync` also `git push` to all remotes, or just ensure repos exist on forges?

4. **Conflict resolution** - If forges are out of sync (different commits), which is authoritative?

---

## Summary

The key insight is: **workspace is the user's entry point**, not org.

```
Current:  org -> repos -> forge
Proposed: workspace -> org -> forge -> repos
```

This inverts the mental model to match how developers work:
1. I'm in a directory (workspace)
2. That workspace has projects (orgs)
3. Those projects are mirrored to services (forges)
4. Each service hosts repositories (repos)

The workspace becomes the orchestration layer that cascades operations down the hierarchy.
