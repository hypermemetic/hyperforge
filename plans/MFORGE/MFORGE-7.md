# MFORGE-7: Auth Check, Requirements, and SSH Key Helpers

blocked_by: [MFORGE-2]
unlocks: [MFORGE-8]

## Scope

Update `auth_check`, `auth_requirements`, and SSH key helpers in hub.rs
to iterate per-forge credential blocks.

## Method

### auth_check
Currently iterates `org_cfg.forge.credentials` and probes against `org_cfg.forge.provider`.
Change to: iterate `org_cfg.forges`, for each `(provider, block)` probe that provider's
credentials against that provider's endpoint.

### auth_requirements
Same iteration change. Emit one `AuthRequirement` per forge per credential type.

### config_set_ssh_key
Add `--forge` parameter. Target the specific provider's credential block.
If omitted on single-forge org → use that forge. Multi-forge without `--forge` → error.

### config_show_ssh_key
Add optional `--forge` filter. Without it, show SSH keys for all forges.

## Tests

### `test_auth_check_multi_forge`
Org with github (valid token) + codeberg (valid token).
Assert two `AuthCheckResult` events, one per provider, each reporting success.

### `test_auth_check_one_bad_cred`
Org with github (valid) + codeberg (invalid token).
Assert github passes, codeberg fails. Both reported.

### `test_auth_requirements_multi_forge`
Dual-forge org. Assert requirements emitted for each forge's credential slots.

### `test_ssh_key_set_per_forge`
Dual-forge org. `config_set_ssh_key --org foo --forge codeberg --key /path/to/key`.
Assert codeberg forge has SSH key. Github forge unchanged.

### `test_ssh_key_show_all`
Dual-forge org with SSH keys on both. `config_show_ssh_key --org foo`.
Assert two results, one per forge.
