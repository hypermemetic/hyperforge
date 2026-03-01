# CONF-2: Expand RepoRecord to Absorb Per-Repo Config Fields

**blocked_by**: []
**unlocks**: [CONF-3, CONF-4]

## Scope

`RepoRecord` currently tracks lifecycle metadata (present_on, dismissed, privatized_on, etc.) but delegates forge selection, SSH keys, CI config, and default branch to per-repo `.hyperforge/config.toml`. This ticket merges those fields into `RepoRecord` so LocalForge can be the single source of truth.

## Current State

**RepoRecord** (`src/types/repo.rs`):
```
name, description, visibility, origin, mirrors,
present_on, privatized_on, dismissed, deleted_from, deleted_at,
previous_names, managed, protected, staged_for_deletion,
default_branch
```

**HyperforgeConfig** (`src/config/mod.rs`) — fields NOT in RepoRecord:
```
forges: Vec<String>          — which forges to sync to
ssh: HashMap<String, String> — SSH key path per forge
forge_config: HashMap<String, ForgeConfig>  — per-forge overrides (org, remote name)
ci: Option<CiConfig>         — validation config (dockerfile, build, test, etc.)
default_branch: Option<String> — already in RepoRecord
repo_name: Option<String>    — redundant with RepoRecord.name
org: Option<String>          — implicit from LocalForge org key
```

## Changes

### 1. Add fields to `RepoRecord` (`src/types/repo.rs`)

```rust
pub struct RepoRecord {
    // ... existing fields ...

    /// Local clone path (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<PathBuf>,

    /// Which forges this repo should sync to (e.g. ["github", "codeberg"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forges: Vec<String>,

    /// SSH key paths per forge
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ssh: HashMap<String, String>,

    /// Per-forge config overrides (org override, remote name override)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub forge_config: HashMap<String, ForgeConfig>,

    /// CI/validation configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiConfig>,
}
```

### 2. Update `RepoRecord::from_repo()` and `RepoRecord::to_repo()`

`from_repo()` initializes new fields to defaults (empty vecs/maps, None). The existing conversion path remains backward-compatible.

### 3. Add `RepoRecord::merge_from_config(config: &HyperforgeConfig)`

Absorbs fields from a per-repo config into the record. Used during discovery/import to pull config data into LocalForge:

```rust
impl RepoRecord {
    pub fn merge_from_config(&mut self, config: &HyperforgeConfig) {
        if self.forges.is_empty() {
            self.forges = config.forges.clone();
        }
        if self.ssh.is_empty() {
            self.ssh = config.ssh.clone();
        }
        if self.forge_config.is_empty() {
            self.forge_config = config.forge_config.clone();
        }
        if self.ci.is_none() {
            self.ci = config.ci.clone();
        }
        if self.default_branch.is_none() {
            self.default_branch = config.default_branch.clone();
        }
        if self.description.is_none() {
            self.description = config.description.clone();
        }
    }
}
```

### 4. Update YAML serialization

`repos.yaml` gains new optional fields per record. Old YAML files without these fields deserialize with defaults (empty vecs, None) — fully backward-compatible via `#[serde(default)]`.

### 5. Move `ForgeConfig` and `CiConfig` to `src/types/` (or re-export)

These types are currently defined in `src/config/mod.rs`. They need to be importable by `RepoRecord` without creating a circular dependency. Move them to `src/types/config.rs` or make `src/types/mod.rs` re-export them.

## Acceptance Criteria

- [ ] `RepoRecord` has `local_path`, `forges`, `ssh`, `forge_config`, `ci` fields
- [ ] `merge_from_config()` absorbs HyperforgeConfig fields
- [ ] Old repos.yaml files without new fields deserialize without error
- [ ] New repos.yaml files serialize the new fields when non-empty
- [ ] `ForgeConfig` and `CiConfig` are importable from `src/types/`
- [ ] `cargo build --release` succeeds
- [ ] Existing tests pass

## Notes

- `HyperforgeConfig` struct and its `load()`/`save()` methods stay alive for now — CONF-3 uses them for the materialize step, and CONF-7 removes direct writes
- The `org` and `repo_name` fields of HyperforgeConfig are NOT added to RepoRecord because they're implicit (org = LocalForge key, repo_name = record key)
