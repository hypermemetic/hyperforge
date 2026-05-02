# MFORGE-8: Wire Shape Migration for OrgSummary/OrgDetail

blocked_by: [MFORGE-4]
unlocks: [MFORGE-9]

## Scope

Update OrgsEvent wire shape for forward compatibility. Old consumers expecting
`provider: ProviderKind` still work; new consumers read `providers: Vec<ProviderKind>`.

## Method

### OrgSummary
Add `providers: Vec<ProviderKind>` field.
Keep `provider: ProviderKind` as deprecated (populated from primary_provider).

### OrgDetail
Add `forges: BTreeMap<String, ForgeProviderBlock>` field.
Keep `provider` + `credentials` as deprecated (primary forge's data).

### Deprecation timeline
`provider` field on OrgSummary/OrgDetail removed in v6.

## Tests

### `test_org_summary_has_providers`
Multi-forge org summary event contains `providers: ["codeberg", "github"]` (sorted).

### `test_org_summary_backward_compat`
Single-forge org summary still has `provider: "github"`.

### `test_org_detail_has_forges_map`
Multi-forge org detail contains `forges: {github: {credentials: [...]}, codeberg: {credentials: [...]}}`.

### `test_org_detail_backward_compat`
Single-forge org detail still has `provider` and `credentials` at top level.
