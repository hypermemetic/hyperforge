# MFORGE-2: Schema + Backward-Compatible Deserialization

blocked_by: []
unlocks: [MFORGE-3, MFORGE-4, MFORGE-6]

## Scope

Replace `OrgConfig.forge: ForgeBlock` with `OrgConfig.forges: BTreeMap<ProviderKind, ForgeProviderBlock>`.
Old YAML (`forge:` key) must deserialize transparently; new YAML serializes as `forges:`.

## Method

### Type changes (`src/v5/config.rs`)

New struct:
```rust
pub struct ForgeProviderBlock {
    pub credentials: Vec<CredentialEntry>,
}
```

`OrgConfig` changes:
```rust
pub struct OrgConfig {
    pub name: OrgName,
    pub forges: BTreeMap<ProviderKind, ForgeProviderBlock>,  // was: forge: ForgeBlock
    pub repos: Vec<OrgRepo>,
}
```

### Custom deserialization

Accept either:
- `forge: { provider: github, credentials: [...] }` → single-entry map `{ Github: { credentials } }`
- `forges: { github: { credentials: [...] }, codeberg: { credentials: [...] } }` → direct

Both present → error (ambiguous).
Neither present → error (missing field).

### Convenience methods on OrgConfig

```rust
fn providers(&self) -> impl Iterator<Item = ProviderKind>
fn primary_provider(&self) -> Option<ProviderKind>  // first key
fn credentials_for(&self, provider: ProviderKind) -> &[CredentialEntry]
fn all_credentials(&self) -> impl Iterator<Item = (ProviderKind, &CredentialEntry)>
```

## Tests

### `test_round_trip_legacy_org`
Deserialize old `forge:` YAML, re-serialize, re-parse. Assert data preserved,
output uses `forges:` key.

### `test_round_trip_multi_forge_org`
Deserialize new `forges:` YAML with github+codeberg, round-trip. Assert both
providers present with correct credentials.

### `test_legacy_single_forge_to_new`
Load old format. Assert `forges` map has exactly one entry. Assert provider
and credentials match original.

### `test_both_forge_and_forges_errors`
YAML with both `forge:` and `forges:` keys → deserialization error.

### `test_neither_forge_nor_forges_errors`
YAML with neither key → deserialization error.

### `test_existing_fixtures_still_load`
All fixture org YAMLs in `tests/v5/fixtures/` load without error.

### `test_convenience_methods`
Multi-forge org: `providers()` returns both, `primary_provider()` returns first,
`credentials_for(Github)` returns github creds only, `credentials_for(Codeberg)`
returns codeberg creds only.
