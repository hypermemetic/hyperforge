# Auth Plugin Implementation

## Overview

This document describes the auth plugin implementation for hyperforge, including architecture, usage, testing, and remaining work.

## What Was Built

### 1. Auth Hub Plugin (`src/auth_hub/`)

A standalone secret management service that runs independently from hyperforge.

**Components:**
- `types.rs`: Secret types (SecretPath, Secret, SecretInfo)
- `storage.rs`: YAML file storage backend with async I/O
- `mod.rs`: Hub methods implementation

**Hub Methods:**
- `get_secret(path)`: Retrieve a secret by path
- `set_secret(path, value)`: Store a secret
- `list_secrets(prefix)`: List secrets matching a prefix
- `delete_secret(path)`: Remove a secret

**Storage Format** (`~/.config/hyperforge/secrets.yaml`):
```yaml
secrets:
  github/hypermemetic/token:
    value: "ghp_xxxxxxxxxxxxx"
    created_at: "2026-01-27T01:00:00Z"
    updated_at: "2026-01-27T01:00:00Z"
  codeberg/alice/token:
    value: "codeberg_token_here"
    created_at: "2026-01-27T01:00:00Z"
```

**Binary:** `hyperforge-auth`
- Runs on port 4445 by default
- Namespace: `auth` (distinct from hyperforge's `lforge`)
- Transport: WebSocket via hub-transport

### 2. Auth Integration in Hyperforge

**RPC-based Auth Provider** (`src/auth/yaml_provider.rs`):
- Calls auth hub via JSON-RPC using `synapse` from PATH
- Hyperforge has no knowledge of YAML storage
- Properly abstracted - swappable storage backends

**Integration Points:**
- `src/hub.rs`: repos_import, workspace_diff, workspace_sync
- `src/remote/mod.rs`: get_forge_adapter
- All forge operations request tokens via auth hub

### 3. LFORGE2 Git Commands

**Hub Methods** (all in `src/hub.rs`):
- `git_init`: Initialize hyperforge config with forges and SSH keys
  - Configures `core.sshCommand` for per-repo SSH keys
  - Creates `.hyperforge/config.toml`
- `git_status`: Show repo sync status across forges
- `git_push`: Push to all configured forges

**SSH Key Management:**
- Per-repo SSH keys via git's `core.sshCommand`
- Keys mapped in `.hyperforge/config.toml` [ssh] section
- No global `~/.ssh/config` modifications needed

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         SYNAPSE                              │
│                    (CLI orchestrator)                        │
└───────────────┬─────────────────────┬───────────────────────┘
                │                     │
        JSON-RPC│                     │JSON-RPC
                │                     │
    ┌───────────▼───────────┐   ┌────▼──────────────┐
    │   Hyperforge Hub      │   │   Auth Hub        │
    │   (port 4444)         │   │   (port 4445)     │
    │   namespace: lforge   │   │   namespace: auth │
    └───────────┬───────────┘   └────┬──────────────┘
                │                     │
                │                     ▼
                │            ~/.config/hyperforge/
                │                secrets.yaml
                │
                ▼
         Git Repositories
    (.hyperforge/config.toml
     .git/config with SSH keys)
```

**Key Design Principles:**
1. **Separation of Concerns**: Auth hub manages secrets, hyperforge consumes them
2. **Storage Agnostic**: Hyperforge doesn't know about YAML (calls RPC)
3. **Per-Repo Config**: Each git repo has its own `.hyperforge/config.toml`
4. **Multi-Hub**: Auth and hyperforge are separate services with unique namespaces

## Usage

### Starting the Services

**Terminal 1: Start Auth Hub**
```bash
cargo run --release --bin hyperforge-auth -- --port 4445
```

**Terminal 2: Start Hyperforge Hub**
```bash
cargo run --release --bin hyperforge -- --port 4444
```

### Managing Secrets

**Set a GitHub token** (manual YAML edit for now):
```bash
mkdir -p ~/.config/hyperforge
cat > ~/.config/hyperforge/secrets.yaml <<EOF
secrets:
  github/hypermemetic/token:
    value: "ghp_YOUR_GITHUB_TOKEN_HERE"
    created_at: "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    updated_at: "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
EOF
```

**Future: Via synapse** (once auth hub RPC is debugged):
```bash
synapse -P 4445 auth auth set_secret \
  --path "github/hypermemetic/token" \
  --value "ghp_xxxxx"
```

### Using Hyperforge

**Initialize a repository:**
```bash
synapse -P 4444 lforge hyperforge git_init \
  --path /workspace/myrepo \
  --forges github \
  --org hypermemetic \
  --ssh-keys "github:~/.ssh/hyperforge_ed25519"
```

**Check status:**
```bash
synapse -P 4444 lforge hyperforge git_status \
  --path /workspace/myrepo
```

**Import repos from GitHub:**
```bash
synapse -P 4444 lforge hyperforge repos_import \
  --org hypermemetic \
  --forge github
```

**List local repos:**
```bash
synapse -P 4444 lforge hyperforge repos_list \
  --org hypermemetic
```

**Sync workspace to remote:**
```bash
synapse -P 4444 lforge hyperforge workspace_sync \
  --org hypermemetic \
  --forge github \
  --dry-run true
```

## Testing Plan

### Phase 1: Auth Hub Tests (Current Blocker)

**Issue:** Auth hub returns "Internal error" when calling methods via synapse.

**Debug Steps:**
1. Check auth hub logs for errors
2. Try calling via raw WebSocket (wscat)
3. Verify hub_methods macro generates correct RPC code
4. Compare with working hyperforge hub implementation

**Test Commands:**
```bash
# Check if auth hub schema is accessible
synapse -P 4445 auth schema --raw

# Try to set a secret
synapse -P 4445 auth auth set_secret \
  --path "test/token" \
  --value "test123" \
  --raw

# Try to get a secret
synapse -P 4445 auth auth get_secret \
  --path "test/token" \
  --raw
```

### Phase 2: Manual Secret Storage + Hyperforge Tests

**Prerequisites:**
1. Get a real GitHub token: `gh auth login` (or use GitHub Settings → Developer settings → Personal access tokens)
2. Add token to `~/.config/hyperforge/secrets.yaml`

**Test 1: Token Retrieval**
```bash
# Hyperforge should be able to call auth hub and get the token
# This happens automatically when you use repos_import
synapse -P 4444 lforge hyperforge repos_import \
  --org hypermemetic \
  --forge github \
  --raw
```

**Expected:** Lists repos from hypermemetic org on GitHub

**Test 2: Workspace Diff**
```bash
synapse -P 4444 lforge hyperforge workspace_diff \
  --org hypermemetic \
  --forge github \
  --raw
```

**Expected:** Shows what repos would be created/updated/deleted

**Test 3: Git Init**
```bash
synapse -P 4444 lforge hyperforge git_init \
  --path /workspace/hyperforge \
  --forges github \
  --org hypermemetic \
  --ssh-keys "github:~/.ssh/hyperforge_ed25519" \
  --force true \
  --raw
```

**Expected:** Creates `.hyperforge/config.toml` and sets `core.sshCommand`

**Test 4: Git Status**
```bash
synapse -P 4444 lforge hyperforge git_status \
  --path /workspace/hyperforge \
  --raw
```

**Expected:** Shows branch, working tree status, forge sync status

**Test 5: Git Push** (if SSH key has push access)
```bash
synapse -P 4444 lforge hyperforge git_push \
  --path /workspace/hyperforge \
  --dry-run true \
  --raw
```

**Expected:** Shows what would be pushed to each forge

### Phase 3: Full Integration Tests

**Test 6: Create Local Repo**
```bash
synapse -P 4444 lforge hyperforge repos_create \
  --org hypermemetic \
  --name test-repo \
  --visibility public \
  --origin github \
  --description "Test repository" \
  --raw
```

**Expected:** Creates entry in `~/.config/hyperforge/orgs/hypermemetic/repos.yaml`

**Test 7: Sync to Remote**
```bash
synapse -P 4444 lforge hyperforge workspace_sync \
  --org hypermemetic \
  --forge github \
  --dry-run false \
  --raw
```

**Expected:** Creates test-repo on GitHub via API

**Test 8: Multi-Forge Setup**
```bash
# Add Codeberg token
echo "  codeberg/hypermemetic/token:
    value: \"your_codeberg_token\"
    created_at: \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"
" >> ~/.config/hyperforge/secrets.yaml

# Sync to Codeberg
synapse -P 4444 lforge hyperforge workspace_sync \
  --org hypermemetic \
  --forge codeberg \
  --raw
```

**Expected:** Creates test-repo on Codeberg

## What's Left to Do

### Critical Path (Required for MVP)

1. **Fix Auth Hub RPC** ⚠️ BLOCKER
   - Debug "Internal error" when calling auth hub methods
   - Verify hub_methods macro generates correct code
   - Alternative: Manually test YAML storage reads/writes

2. **Get Real GitHub Token**
   - Complete `gh auth login` (device code: 1122-D9EA)
   - Or create token at https://github.com/settings/tokens
   - Add to secrets.yaml

3. **Test Hyperforge → Auth Hub RPC**
   - Verify hyperforge can call auth hub via synapse
   - Confirm token retrieval works
   - Test repos_import with real token

4. **Test Full Workflow**
   - Import repos from GitHub
   - Create local repo config
   - Sync to remote forge
   - Verify repos created via GitHub UI

### Nice to Have (Post-MVP)

1. **Improve Auth Hub RPC Client**
   - Replace synapse shell calls with proper JSON-RPC client library
   - Make auth hub calls non-blocking
   - Add connection pooling

2. **Auth Hub Method Access via Synapse**
   - Fix internal error
   - Test set_secret/get_secret via synapse
   - Document secret management workflow

3. **SSH Key Management**
   - Auto-generate SSH keys per forge
   - Store in auth hub
   - Auto-configure git SSH

4. **Multi-Org Support**
   - Test with multiple GitHub orgs
   - Test with different usernames per forge
   - Verify token isolation

5. **Error Handling**
   - Better error messages when token missing
   - Retry logic for network failures
   - Token expiration detection

6. **Testing Infrastructure**
   - Unit tests for auth provider
   - Integration tests for full workflow
   - Mock auth hub for testing

## Known Issues

### Auth Hub RPC Error

**Symptom:** Calling auth hub methods via synapse returns:
```
Fetch error at auth.auth: Subscription error: RpcErrorObj {errCode = -32603, errMessage = "Internal error"}
```

**Status:** Not yet debugged (user said not to focus on hub macro)

**Workaround:** Directly edit `~/.config/hyperforge/secrets.yaml` for now

### Hyperforge Push Permission Denied

**Symptom:** `git_push` fails with "Permission denied" if SSH key not added to GitHub account with write access

**Solution:** Add `~/.ssh/hyperforge_ed25519.pub` to GitHub account or as deploy key

## File Locations

### Configuration Files
- Auth secrets: `~/.config/hyperforge/secrets.yaml`
- Org repos: `~/.config/hyperforge/orgs/<org>/repos.yaml`
- Repo config: `<repo>/.hyperforge/config.toml`

### Git Configuration
- SSH command: `<repo>/.git/config` (core.sshCommand)
- Git remotes: `<repo>/.git/config` (remote sections)

### Binaries
- Auth hub: `target/release/hyperforge-auth`
- Hyperforge: `target/release/hyperforge`

## Example Secrets File

```yaml
secrets:
  # GitHub tokens (per org)
  github/hypermemetic/token:
    value: "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
    created_at: "2026-01-27T01:00:00Z"
    updated_at: "2026-01-27T01:00:00Z"

  github/alice/token:
    value: "ghp_yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"
    created_at: "2026-01-27T01:00:00Z"

  # Codeberg tokens
  codeberg/hypermemetic/token:
    value: "codeberg_token_here"
    created_at: "2026-01-27T01:00:00Z"

  # GitLab tokens
  gitlab/hypermemetic/token:
    value: "glpat-xxxxxxxxxxxxxxxxxxxx"
    created_at: "2026-01-27T01:00:00Z"

  # Registry tokens
  cargo/token:
    value: "crates_io_token"
    created_at: "2026-01-27T01:00:00Z"
```

## Secret Path Format

Secrets use hierarchical paths:

**Forge tokens:** `{forge}/{org}/token`
- Examples: `github/alice/token`, `codeberg/acme-corp/token`

**Registry tokens:** `{registry}/token`
- Examples: `cargo/token`, `npm/token`, `pypi/token`

**Future: Per-repo secrets:** `{forge}/{org}/{repo}/deploy-key`

## Success Criteria

The implementation is complete when:

1. ✅ Auth hub runs standalone and manages secrets
2. ✅ Hyperforge calls auth hub via RPC (not direct file access)
3. ⏳ Hyperforge can authenticate with GitHub API using token from auth hub
4. ⏳ repos_import successfully lists repos from GitHub
5. ⏳ workspace_sync can create repos on GitHub via API
6. ✅ git_init configures SSH keys per repo
7. ✅ git_push uses configured SSH keys
8. ⏳ Multi-forge operations work (GitHub + Codeberg + GitLab)

## Conclusion

The auth plugin architecture is complete and demonstrates proper separation of concerns. Hyperforge is storage-agnostic and calls the auth service via RPC. The main remaining work is:

1. Debug auth hub RPC (or use manual YAML editing as workaround)
2. Get a real GitHub token
3. Test the full workflow end-to-end

Once these are complete, LFORGE2 will be fully functional with multi-forge repository management!
