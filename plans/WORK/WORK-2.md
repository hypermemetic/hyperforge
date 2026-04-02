# WORK-2: Workspace Registry

blocked_by: []
unlocks: [WORK-3, WORK-4, WORK-6]

## Scope

Create the workspace registry — a config file at `~/.config/hyperforge/workspaces.toml` that maps workspace names to paths, orgs, and forge lists.

## Types

In `src/config/workspace.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub org: String,
    #[serde(default)]
    pub forges: Vec<Forge>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceRegistry {
    #[serde(flatten)]
    pub workspaces: HashMap<String, WorkspaceEntry>,
}
```

## Tests

### `test_registry_roundtrip`
Create a WorkspaceRegistry with two entries, save to a tempdir, load it back. Assert the loaded registry has the same two entries with identical fields.

### `test_registry_empty_file`
Load from a tempdir with an empty `workspaces.toml`. Assert the registry has 0 entries and no panic.

### `test_registry_missing_file`
Load from a tempdir with no `workspaces.toml`. Assert the registry has 0 entries and no panic.

### `test_register_new`
Start with empty registry. Register "hypermemetic" with path `/tmp/hm`, org `hypermemetic`, forges `[GitHub, Codeberg]`. Assert `get("hypermemetic")` returns the entry. Assert `all().len() == 1`.

### `test_register_overwrite`
Register "hypermemetic" twice with different paths. Assert the second registration wins. Assert `all().len() == 1`.

### `test_unregister`
Register two workspaces. Unregister one. Assert the removed entry is returned. Assert `all().len() == 1`. Assert `get("removed_name")` returns None.

### `test_unregister_missing`
Unregister a name that doesn't exist. Assert None is returned. Assert registry unchanged.

### `test_find_by_path`
Register two workspaces with different paths. Call `find_by_path` with one of the paths. Assert it returns the matching (name, entry). Call with a path that doesn't match. Assert None.

### `test_find_by_path_canonicalization`
Register with path `/tmp/hm`. Call `find_by_path` with `/tmp/hm/`. Assert it matches (trailing slash tolerance).

### `test_toml_format`
Create a registry with one entry, save it, read the raw TOML string. Assert it contains `[hypermemetic]`, `path = "..."`, `org = "hypermemetic"`, `forges = ["github", "codeberg"]`.

## Integration with HyperforgeState

Add `workspace_registry: Arc<RwLock<WorkspaceRegistry>>` to `HyperforgeState`. Load from `config_dir` on startup.

### `test_state_loads_registry`
Create a tempdir with a valid workspaces.toml. Construct HyperforgeState pointing at it. Assert registry is populated.
