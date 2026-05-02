# MFORGE-4: OrgsHub CRUD Adaptation

blocked_by: [MFORGE-2]
unlocks: [MFORGE-5, MFORGE-7]

## Scope

Update all OrgsHub methods to work with multi-forge shape. Add `--forge` targeting
for credential operations.

## Method

### orgs.create
Accept `provider` (single forge, backward compat) or `providers` (multi-forge).
Creates `forges` map with empty credential blocks for each provider.

### orgs.update
Add `--add_forge <provider>` and `--remove_forge <provider>` params.
Keep `--provider` for single-forge update (patches primary).

### orgs.set_credential
Add required `--forge` param to target which forge's credential list.
Backward compat: if omitted and org has exactly one forge, use it.
If omitted and org has multiple forges → error `ambiguous_forge`.

### orgs.remove_credential
Same `--forge` param as set_credential.

### orgs.list / orgs.get
Events updated per MFORGE-7.

### orgs.bootstrap
Takes `--provider` (single forge). Creates or extends the org's forge map.
If org already exists with other forges, adds the new provider without removing existing.

## Tests

### `test_create_single_forge`
`orgs create --name foo --provider github` → `forges: {github: {credentials: []}}`.

### `test_add_forge_via_update`
Create github-only org. `orgs update --org foo --add_forge codeberg`.
Assert org now has both forges.

### `test_remove_forge_via_update`
Dual-forge org. `orgs update --org foo --remove_forge codeberg`.
Assert org has only github. Assert codeberg credentials removed.

### `test_set_credential_targets_forge`
Dual-forge org. `orgs set_credential --org foo --forge codeberg --key secrets://cb --credential_type token`.
Assert added only to codeberg's block. Github block unchanged.

### `test_set_credential_single_forge_fallback`
Single-forge org. `orgs set_credential --org foo --key ... --credential_type token` (no --forge).
Assert succeeds (unambiguous).

### `test_set_credential_multi_forge_no_target_errors`
Dual-forge org. `orgs set_credential --org foo --key ... --credential_type token` (no --forge).
Assert error: `ambiguous_forge`.

### `test_bootstrap_adds_to_existing`
Org already has github. `orgs bootstrap --name foo --provider codeberg --token ...`.
Assert org now has both forges. Github credentials preserved.
