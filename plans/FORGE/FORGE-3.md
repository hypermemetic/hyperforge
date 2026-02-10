# FORGE-3: Add `OrgDefaults` struct

blocked_by: []
unlocks: [FORGE-6]

## Scope

Add `OrgDefaults` struct for org-level default configuration, stored at `~/.config/hyperforge/orgs/{org}/defaults.yaml`.

## File

`src/types/mod.rs`

## Changes

### New struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgDefaults {
    #[serde(default)]
    pub forges: Vec<Forge>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub protected: bool,
}
```

### Purpose

When `repos_create` is called without `--forges`, fall back to org defaults. This eliminates the need to specify forges on every create call.

Example `defaults.yaml`:
```yaml
forges:
  - github
  - codeberg
visibility: public
protected: false
```

### Acceptance criteria

- Struct is `Serialize + Deserialize`
- All fields have `#[serde(default)]` for partial YAML files
- Exported from `types` module
