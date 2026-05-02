# MFORGE-3: Per-Provider Credential Resolution in ops::repo

blocked_by: [MFORGE-2]
unlocks: [MFORGE-5, MFORGE-6]

## Scope

Replace org-wide `token_ref_for(org)` / `default_token_ref_for(org)` with
provider-scoped variants. Update all forge-call wrappers to accept explicit provider.

## Method

### New functions (`src/v5/ops/repo.rs`)

```rust
pub fn token_ref_for_provider(org: &OrgConfig, provider: ProviderKind) -> Option<&str>
pub fn default_token_ref_for_provider(provider: ProviderKind) -> String
pub fn ssh_key_for_provider(org: &OrgConfig, provider: ProviderKind) -> Option<&str>
```

### Deprecate old functions

`token_ref_for(org)` → calls `token_ref_for_provider(org, org.primary_provider()?)`.
`default_token_ref_for(org)` → calls `default_token_ref_for_provider(org.primary_provider()?)`.

### Update forge-call wrappers

Every wrapper (`exists_on_forge`, `create_on_forge`, `delete_on_forge`,
`list_on_forge`, `write_metadata_on_forge`, `privatize_on_forge`) now takes
explicit `provider: ProviderKind` and derives auth from that provider's credential block.

### Update sync_one

`sync_one` currently resolves credentials once per repo. Change to per-remote:
derive provider from remote URL → look up per-provider credentials → build ForgeAuth.

## Tests

### `test_token_ref_for_provider_github`
Org with `forges: {github: {credentials: [{key: "secrets://gh", type: token}]}}`.
Assert `token_ref_for_provider(org, Github)` returns `Some("secrets://gh")`.
Assert `token_ref_for_provider(org, Codeberg)` returns `None`.

### `test_default_token_ref_for_provider`
Assert `default_token_ref_for_provider(Github)` = `"secrets://github/_default/token"`.
Assert `default_token_ref_for_provider(Codeberg)` = `"secrets://codeberg/_default/token"`.

### `test_sync_one_per_provider_creds`
Org with github (token A) + codeberg (token B). Repo with both remotes.
Assert ForgeAuth for github remote uses token A, ForgeAuth for codeberg remote uses token B.
