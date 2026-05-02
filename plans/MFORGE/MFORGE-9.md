# MFORGE-9: E2E Integration Tests

blocked_by: [MFORGE-5, MFORGE-6, MFORGE-7, MFORGE-8]
unlocks: []

## Scope

End-to-end validation that all MFORGE pieces compose correctly.
New fixture + tier-1 test exercising the complete multi-forge lifecycle.

## Method

### New fixture
`tests/v5/fixtures/multi_forge_org/` containing:
- `config.yaml` with provider_map for github.com and codeberg.org
- `orgs/hypermemetic.yaml` with two forges, two repos with mixed remotes
- `secrets.yaml` with two tokens (test values)

### Test script
`tests/v5/MFORGE/MFORGE-9.sh`

## Tests

### `test_e2e_multi_forge_lifecycle`
1. Load fixture
2. `orgs list` → shows multi-forge org with both providers
3. `orgs get` → shows per-forge credentials
4. `repos list` → shows repos
5. `repos sync` on dual-remote repo → uses correct per-forge credentials
6. `orgs set_credential --forge codeberg` → targets correct block
7. Verify round-trip: save + reload → data preserved

### `test_e2e_legacy_org_unchanged`
Load existing `minimal_org` fixture. All existing test assertions pass.
No behavioral regression.

### `test_e2e_migration_round_trip`
1. Load old-format org yaml (single `forge:` key)
2. Call `orgs get` → verify correct providers
3. Call `orgs update --add_forge codeberg` → verify second forge added
4. Save → verify new format on disk
5. Reload → verify semantics preserved

### `test_e2e_bootstrap_multi_forge`
1. `orgs bootstrap --name testorg --provider github --token <test>`
2. `orgs bootstrap --name testorg --provider codeberg --token <test>`
3. `orgs get --org testorg` → both forges present, both credentials present
4. `repos import --org testorg` → imports from both forges
