# AUTH-6: Pre-flight Validation in Pipelines

blocked_by: [AUTH-3, AUTH-5]
unlocks: []

## Scope

Add credential pre-flight checks to `build release`, `build release_all`, `workspace sync`, and `workspace push_all` so they fail fast with a clear message instead of failing mid-pipeline.

## Implementation

### Shared pre-flight function

```rust
async fn preflight_auth_check(
    repos: &[DiscoveredRepo],
    forges: &[String],
    channels: &[DistChannel],
    auth: &YamlAuthProvider,
) -> Vec<HyperforgeEvent>
```

Returns events for any missing/invalid credentials. If the vec is non-empty, the pipeline should abort before doing any work.

### Where to add pre-flight

| Command | Check before |
|---------|-------------|
| `build release` | Phase: compile (after discovering repos, before building) |
| `build release_all` | Phase: first repo (after workspace discovery, before any compilation) |
| `workspace sync` | Phase 6 (after discovery, before diff API calls) |
| `workspace push_all` | Before parallel push |

### Behavior

- If pre-flight fails, emit error events listing missing credentials + the `auth setup` command to fix them
- Add `--skip_auth_check` flag to bypass (for offline/cached operations)
- Pre-flight runs the same validation as `auth check` but only for the credentials that will actually be used

### Example failure

```
Pre-flight auth check failed:

  ✗ github/hypermemetic/packages_token — MISSING
    Needed by: build release (forge-release channel)
    Fix: synapse -P 44105 secrets auth set_secret --secret_key "github/hypermemetic/packages_token" --value "$(pbpaste)"
    Create at: https://github.com/settings/tokens/new?scopes=read:packages,write:packages

  ✗ crates-io/token — MISSING
    Needed by: build release (crates-io channel)
    Fix: synapse -P 44105 secrets auth set_secret --secret_key "crates-io/token" --value "$(pbpaste)"
    Create at: https://crates.io/me

Aborting. Run `auth setup --org hypermemetic` to configure missing credentials.
```

## Acceptance Criteria

- [ ] `build release` fails before compiling if tokens are missing
- [ ] `workspace sync` fails before API calls if tokens are missing
- [ ] Error messages include the exact command to fix each missing credential
- [ ] `--skip_auth_check` bypasses pre-flight
- [ ] Pre-flight only checks credentials needed for the specific operation
