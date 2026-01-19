# Workspace Sync Invariants

**Status**: Proposed
**Date**: 2026-01-17

## Problem

The current `repos sync` command only syncs repos to remote forges (API operations). It doesn't enforce local workspace state:

- Git remotes may be missing or stale
- New forges added to config don't propagate to existing local repos
- No single command ensures "workspace is fully consistent"

## Proposal

Enhance `repos sync` to enforce **all workspace-level invariants** in a single command.

## Invariants to Enforce

### 1. Forge State (existing)
Each repo exists on its configured forges with correct properties.

```
For each repo in config:
  For each forge in repo.forges:
    - Repo exists on forge
    - Visibility matches config
    - Description matches config
```

### 2. Git Remotes (new)
Each local git repo has remotes for all configured forges.

```
For each repo in config:
  If local git repo exists at workspace/{repo}:
    For each forge in repo.forges:
      - Remote named {forge} exists
      - Remote URL is git@{forge}-{org}:{owner}/{repo}.git
```

### 3. Synced State (existing)
Tracking state reflects actual forge state after sync.

```
For each successful forge operation:
  - _synced[forge] updated with URL and timestamp
```

### 4. SSH Config (optional, future)
SSH host aliases configured for org's forges.

```
For each forge in org.forges:
  - ~/.ssh/config has Host {forge}-{org}
  - IdentityFile points to org's SSH key
```

## Implementation

### Phase 1: Add Remote Sync to `repos sync`

Modify `WorkspaceService::process_org_sync()` to include a remote enforcement step after forge sync.

```rust
pub async fn process_org_sync(...) -> Vec<WorkspaceEvent> {
    // ... existing forge sync ...

    // NEW: Enforce git remotes for all repos
    if auto_yes {
        for event in self.enforce_git_remotes(&config, org_name, workspace_path).await {
            events.push(event);
        }
    }

    events
}

async fn enforce_git_remotes(
    &self,
    config: &GlobalConfig,
    org_name: &str,
    workspace_path: &PathBuf,
) -> Vec<WorkspaceEvent> {
    let mut events = vec![];
    let org_config = config.get_org(org_name)?;
    let storage = self.org_storage(org_name);
    let repos_config = storage.load_repos().await?;

    for (repo_name, repo_config) in &repos_config.repos {
        let repo_path = workspace_path.join(repo_name);

        // Skip if local repo doesn't exist
        if !repo_path.join(".git").exists() {
            continue;
        }

        let forges = repos_config.effective_forges(repo_config);
        let git_bridge = GitRemoteBridge::new(
            repo_path,
            org_name.to_string(),
            org_config.owner.clone(),
        );

        match git_bridge.setup_forge_remotes(&forges, repo_name).await {
            Ok(added) if !added.is_empty() => {
                events.push(WorkspaceEvent::RemotesUpdated {
                    repo_name: repo_name.clone(),
                    remotes_added: added,
                });
            }
            Err(e) => {
                events.push(WorkspaceEvent::Error {
                    message: format!("{}: failed to setup remotes: {}", repo_name, e),
                });
            }
            _ => {} // No changes needed
        }
    }

    events
}
```

### Phase 2: Add Stale Remote Removal (optional)

Remove remotes for forges no longer in config.

```rust
async fn enforce_git_remotes(...) {
    // ... add missing remotes ...

    // Remove stale remotes
    let current_remotes = git_bridge.list_remotes().await?;
    let configured_forges: HashSet<_> = forges.iter()
        .map(|f| f.to_string().to_lowercase())
        .collect();

    for (remote_name, _) in current_remotes {
        // Only manage forge remotes (github, codeberg, gitlab)
        if is_forge_remote(&remote_name) && !configured_forges.contains(&remote_name) {
            git_bridge.remove_remote(&remote_name).await?;
            events.push(WorkspaceEvent::RemoteRemoved {
                repo_name: repo_name.clone(),
                remote_name,
            });
        }
    }
}
```

### Phase 3: Dry-run Support

Preview mode should show what remotes would be added/removed.

```bash
$ synapse plexus hyperforge org hypermemetic repos sync

# Output includes:
repo_name: substrate
remotes_to_add: [github=git@github-hypermemetic:hypermemetic/substrate.git]
type: remote_preview
```

## New Events

```rust
// In workspace/events.rs

/// Git remotes updated for a repo
RemotesUpdated {
    repo_name: String,
    remotes_added: Vec<String>,
},

/// Git remote removed from a repo
RemoteRemoved {
    repo_name: String,
    remote_name: String,
},

/// Preview of remote changes (dry-run)
RemotePreview {
    repo_name: String,
    remotes_to_add: Vec<String>,
    remotes_to_remove: Vec<String>,
},
```

## Command Behavior

### `repos sync` (dry-run, default)

Shows what would change:
1. Forge operations (create/update/delete repos)
2. Remote operations (add/remove git remotes)

### `repos sync --yes`

Applies all changes:
1. Execute forge operations
2. Update git remotes for all local repos
3. Update synced state

## Example Output

```bash
$ synapse plexus hyperforge org hypermemetic repos sync --yes

org_name: hypermemetic
type: org_sync_started

# Forge sync
line: Syncing to github...
repos: [substrate: in_sync, synapse: in_sync, ...]
type: forge_synced

line: Syncing to codeberg...
repos: [substrate: in_sync, synapse: in_sync, ...]
type: forge_synced

# Remote sync (NEW)
line: Enforcing git remotes...
type: sync_output

repo_name: substrate
remotes_added: []
type: remotes_updated

repo_name: new-repo
remotes_added: [github=git@github-hypermemetic:hypermemetic/new-repo.git]
type: remotes_updated

type: org_sync_complete
forge_changes: 0
remote_changes: 1
```

## Edge Cases

### Repo not cloned locally
Skip remote enforcement. User must clone first.

### Remote exists with wrong URL
Update URL via `git remote set-url` (already implemented in GitRemoteBridge).

### Non-forge remotes (e.g., "origin", "upstream")
Don't touch. Only manage remotes named after forges (github, codeberg, gitlab).

### Repo removed from config
Don't remove local repo or remotes. User must manually clean up.

## Migration

No migration needed. Enhancement is additive:
- Existing syncs continue to work
- Remote enforcement adds new behavior
- Users get automatic remote management on next sync

## Testing

1. **Unit tests**: Mock git commands, verify correct remotes added
2. **Integration tests**:
   - Create repo with missing remote, run sync, verify remote added
   - Add forge to config, run sync, verify new remote added
   - Remove forge from config, run sync with stale removal, verify remote removed

## Future Enhancements

### SSH Config Enforcement
```bash
# Ensure SSH config has correct host aliases
$ synapse plexus hyperforge org hypermemetic repos sync --yes

line: Enforcing SSH config...
ssh_hosts_added: [github-hypermemetic, codeberg-hypermemetic]
```

### Workspace Health Check
```bash
# Check all invariants without making changes
$ synapse plexus hyperforge workspace check --path ~/dev/hypermemetic

invariants:
  forge_state: ok
  git_remotes: 2 repos missing remotes
  ssh_config: ok
  synced_state: ok
```

## Summary

| Invariant | Current | Proposed |
|-----------|---------|----------|
| Forge repos exist | ✅ `repos sync` | ✅ No change |
| Forge properties match | ✅ `repos sync` | ✅ No change |
| Git remotes configured | ❌ Manual | ✅ `repos sync` |
| Synced state tracking | ✅ `repos sync` | ✅ No change |
| SSH config | ❌ Manual | 🔮 Future |

**Single command to ensure workspace consistency:**
```bash
synapse plexus hyperforge org hypermemetic repos sync --yes
```
