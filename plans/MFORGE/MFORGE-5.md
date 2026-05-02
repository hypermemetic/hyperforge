# MFORGE-5: Repos sync/push/delete Per-Provider Dispatch

blocked_by: [MFORGE-3]
unlocks: [MFORGE-8]

## Scope

Update all ReposHub methods that call forge adapters to resolve credentials
from the per-provider block. Every call site that builds ForgeAuth must
derive provider from the remote and look up that provider's credentials.

## Method

Every call pattern:
```rust
let token_ref = token_ref_for(org_cfg);
let fallback = default_token_ref_for(org_cfg);
```
Becomes:
```rust
let provider = derive_provider(remote, &provider_map)?;
let token_ref = token_ref_for_provider(org_cfg, provider);
let fallback = default_token_ref_for_provider(provider);
```

### Affected methods
- `repos.sync` — per-remote drift check
- `repos.push` — per-remote metadata write
- `repos.delete` — per-remote privatize
- `repos.purge` — per-remote hard delete
- `repos.add` (with `create_remote=true`) — per-remote create
- `repos.clone` — canonical remote credential
- `repos.push_refs` — git push credential
- `repos.rename` — per-remote rename
- `repos.set_archived` — per-remote metadata write
- `repos.set_default_branch` — per-remote metadata write

### SSH key helpers
`ssh_key_for_org` in repos.rs and `ssh_key_path_for_org` in workspaces.rs become
provider-aware: derive provider from remote → look up per-provider SSH key.

## Tests

### `test_sync_multi_forge_correct_creds`
Org with github (token A) + codeberg (token B). Repo with both remotes.
Sync calls github adapter with token A, codeberg adapter with token B.
Assert two `sync_diff` events, each with correct provider.

### `test_push_multi_forge_sequential`
Push metadata to repo on both forges. Assert each remote gets its own credential.
Assert `push_summary` counts both.

### `test_delete_multi_forge_privatizes_both`
Soft-delete repo. Assert `forge_privatized` for both github and codeberg,
each using its own credential.

### `test_clone_picks_provider_cred`
Clone repo. Assert clone uses credential for the canonical remote's derived provider.
