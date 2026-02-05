# SSH Config Migration: Host Aliases → Per-Repo Git Config

## Summary

Hyperforge has migrated from managing `~/.ssh/config` host aliases to using per-repo git configuration with `core.sshCommand`. This document describes the changes and the current state.

## Previous Approach (Deprecated)

The old approach used SSH host aliases in `~/.ssh/config`:

```
# ~/.ssh/config (managed by SshConfigBridge)
Host github-hypermemetic
    HostName github.com
    User git
    IdentityFile ~/.ssh/hypermemetic
    IdentitiesOnly yes

Host codeberg-hypermemetic
    HostName codeberg.org
    User git
    IdentityFile ~/.ssh/hypermemetic
    IdentitiesOnly yes
```

Remote URLs used these aliases:
```
git@github-hypermemetic:hypermemetic/hyperforge.git
```

**Problems with this approach:**
- Required modifying global `~/.ssh/config`
- Potential conflicts with user's existing SSH config
- Complex alias management across multiple orgs
- URLs were non-standard (couldn't copy/paste from GitHub)

## Current Approach

The new approach uses per-repo git configuration:

```bash
# Set in each repo's .git/config
git config hyperforge.org hypermemetic
git config core.sshCommand "hyperforge-ssh"
```

Remote URLs use plain format:
```
git@github.com:hypermemetic/hyperforge.git
```

The `hyperforge-ssh` wrapper script:
1. Reads `hyperforge.org` from the repo's git config
2. Looks up the SSH key from `~/.config/hyperforge/config.yaml`
3. Executes `ssh -i <key_path>` with the correct identity

**Benefits:**
- No global SSH config modifications
- Standard URLs (copy/paste from GitHub works)
- Per-repo isolation (different orgs can use different keys)
- Works with worktrees (inherits config from main repo)

## Removed Code

### Files Deleted
- `src/bridge/ssh_config.rs` - The `SshConfigBridge` implementation

### Code Removed from Existing Files

**`src/bridge/mod.rs`:**
- Removed: `mod ssh_config;`
- Removed: `pub use ssh_config::SshConfigBridge;`
- Added: Comment explaining the migration

**`src/activations/org/activation.rs`:**
- Removed: `use crate::bridge::SshConfigBridge;`
- Removed: SSH config update logic in `create()` method

**`src/activations/org/events.rs`:**
- Removed: `SshConfigUpdated` event variant

## New Code

### GitRemoteBridge (`src/bridge/git_remote.rs`)

- `build_remote_url()` - Now returns plain URLs (`git@github.com:...`)
- `ensure_ssh_config()` - Sets `hyperforge.org` and `core.sshCommand` on a repo

### WorkspaceService (`src/activations/workspace/service.rs`)

- `enforce_ssh_config()` - Applies SSH config to all repos in a workspace

### Commands

- `workspace sync --enforce_ssh` - Apply SSH config during sync
- `org <org> repos enforce_ssh` - Apply SSH config to all org repos

## Migration Commands

To migrate existing repos to the new approach:

```bash
# For a specific workspace
synapse plexus hyperforge workspace sync --path /path/to/workspace --enforce_ssh true --yes true

# For all repos in an org
synapse plexus hyperforge org hypermemetic repos enforce_ssh
```

## Verification

Check a repo is correctly configured:
```bash
cd /path/to/repo
git config hyperforge.org          # Should show org name
git config core.sshCommand         # Should show "hyperforge-ssh"
git remote -v                      # Should show plain URLs
```

## Related Files

- `~/.config/hyperforge/config.yaml` - Org configs with SSH key paths
- `hyperforge-ssh` - Wrapper script (in PATH, typically `~/.local/bin/`)
