# MFORGE-6: Multi-Forge Import and Bootstrap

blocked_by: [MFORGE-3, MFORGE-4]
unlocks: [MFORGE-8]

## Scope

Update `repos.import` to resolve credentials per-forge. Enable import from
all forges in a multi-forge org. Update `orgs.bootstrap` to add forge entries
without clobbering existing ones.

## Method

### repos.import
- `--forge` specified → use that provider's credential block (already supported, just credential source changes)
- `--forge` omitted on multi-forge org → iterate all forges, import from each, deduplicate by repo name (first-wins by forge order)
- Dedup: if repo name already exists in org, add the new forge's remote to existing entry rather than skipping

### orgs.bootstrap
- Takes single `--provider` (unchanged interface)
- If org already exists: adds the new provider's forge block without removing existing forges
- If org doesn't exist: creates single-forge org (unchanged)
- Credential is written to `secrets://{provider}/{org}/token` (unchanged)
- Import phase: imports from the bootstrapped provider only (not all forges)

## Tests

### `test_import_uses_per_provider_cred`
Dual-forge org. `repos import --org foo --forge codeberg`.
Assert import uses codeberg's credential, not github's.

### `test_import_all_forges`
Dual-forge org with github (3 repos) + codeberg (2 repos, 1 overlapping).
`repos import --org foo` (no --forge). Assert 4 total repos imported (3 + 2 - 1 dedup).
Assert overlapping repo has remotes from both forges.

### `test_bootstrap_creates_single_forge`
`orgs bootstrap --name neworg --provider github --token gh-token://`.
Assert `forges: {github: {credentials: [...]}}`.

### `test_bootstrap_adds_forge_to_existing`
Org exists with github. `orgs bootstrap --name foo --provider codeberg --token <cb-token>`.
Assert org now has `forges: {github: {...}, codeberg: {...}}`.
Assert github credentials unchanged.
Assert codeberg credential added.
Assert import runs against codeberg only.

### `test_bootstrap_idempotent`
Bootstrap github twice. Assert org still has one github forge entry.
Assert credential updated (not duplicated).
