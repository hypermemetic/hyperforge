# WORK-4: Multi-Workspace Resolution

blocked_by: [WORK-2]
unlocks: [WORK-5]

## Scope

Core plumbing: given `--path` and/or `--workspace` args, resolve to a list of workspaces to operate on.

## Function

```rust
pub fn resolve_workspaces(
    registry: &WorkspaceRegistry,
    path: Option<&str>,
    workspace: Option<&str>,
) -> Result<ResolvedWorkspaces, String>
```

## Tests

### `test_resolve_by_name`
Register workspaces "alpha" and "beta". Call `resolve_workspaces(None, Some("alpha"))`.
Assert result has 1 entry named "alpha".

### `test_resolve_by_name_not_found`
Register workspace "alpha". Call `resolve_workspaces(None, Some("gamma"))`.
Assert Err containing "Workspace 'gamma' not found".

### `test_resolve_by_path_registered`
Register "alpha" with path `/tmp/alpha`. Call `resolve_workspaces(Some("/tmp/alpha"), None)`.
Assert result has 1 entry named "alpha" with path `/tmp/alpha`.

### `test_resolve_by_path_unregistered`
Registry is empty. Call `resolve_workspaces(Some("/tmp/mystery"), None)`.
Assert result has 1 entry with an ephemeral name derived from the path basename ("mystery").
Assert the entry's path is `/tmp/mystery`.

### `test_resolve_all`
Register "alpha", "beta", "gamma". Call `resolve_workspaces(None, None)`.
Assert result has 3 entries.

### `test_resolve_all_empty_registry`
Registry is empty. Call `resolve_workspaces(None, None)`.
Assert Err containing "No workspaces registered".

### `test_resolve_ambiguous`
Call `resolve_workspaces(Some("/tmp/alpha"), Some("beta"))`.
Assert Err containing "Cannot specify both --path and --workspace".

### `test_resolve_by_path_trailing_slash`
Register "alpha" with path `/tmp/alpha`. Call `resolve_workspaces(Some("/tmp/alpha/"), None)`.
Assert it resolves to "alpha" (trailing slash normalized).

### `test_resolve_by_path_dot_expansion`
Register "alpha" with the CWD path. Call `resolve_workspaces(Some("."), None)`.
Assert it resolves to "alpha" (dot expanded to CWD). Note: this test must set CWD to the registered path.

### `test_ephemeral_entry_infers_org`
No registered workspaces. Create a tempdir at `/tmp/myws/somerepo/.hyperforge/config.toml` with `org = "myorg"`.
Call `resolve_workspaces(Some("/tmp/myws"), None)`.
Assert the ephemeral entry has `org = "myorg"` (inferred from child repo config).
