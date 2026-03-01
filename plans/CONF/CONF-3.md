# CONF-3: Implement `materialize` Config Projection

**blocked_by**: [CONF-2]
**unlocks**: [CONF-5, CONF-6]

## Scope

Create a single function that projects LocalForge state to disk: writes `.hyperforge/config.toml` and sets git remotes. This replaces all ad-hoc config writes scattered across init, clone, move_repos, and sync.

## Design

### The Function

```rust
/// Project a RepoRecord onto disk at the given path.
///
/// Writes .hyperforge/config.toml from the record's fields,
/// and reconciles git remotes to match the record's forges.
pub fn materialize(
    org: &str,
    record: &RepoRecord,
    repo_path: &Path,
    opts: MaterializeOpts,
) -> Result<MaterializeReport, MaterializeError>
```

### Location

New file: `src/commands/materialize.rs` (or extend `src/commands/mod.rs`).

### What It Does

**Step 1: Write `.hyperforge/config.toml`**
- Build `HyperforgeConfig` from `RepoRecord` fields:
  - `org` = parameter
  - `repo_name` = `record.name` (only if differs from dir name)
  - `forges` = `record.forges`
  - `visibility` = `record.visibility`
  - `description` = `record.description`
  - `ssh` = `record.ssh`
  - `forge_config` = `record.forge_config`
  - `default_branch` = `record.default_branch`
  - `ci` = `record.ci`
- Call `config.save(repo_path)` (creates `.hyperforge/` dir if needed)

**Step 2: Reconcile git remotes** (if `.git` exists)
- List current remotes via `Git::list_remotes(repo_path)`
- Compute desired remotes from `record.forges` + `record.origin` + `record.mirrors`:
  - Origin forge → remote named by forge (e.g., "github")
  - Mirror forges → remotes named by forge (e.g., "codeberg", "gitlab")
  - URL = `build_remote_url(forge, org_for_forge, record.name)` where `org_for_forge` comes from `record.forge_config[forge].org` or falls back to `org`
- For each desired remote:
  - If remote exists with wrong URL → `Git::set_remote_url()`
  - If remote doesn't exist → `Git::add_remote()`
- For remotes that exist but aren't desired: leave them (don't delete user-added remotes)

**Step 3 (optional): Install hooks**
- If `opts.hooks` is true, install pre-push hook
- If `opts.ssh_wrapper` is true, configure SSH wrapper

### Options

```rust
pub struct MaterializeOpts {
    /// Write config file (default: true)
    pub config: bool,
    /// Reconcile git remotes (default: true)
    pub remotes: bool,
    /// Install pre-push hook (default: false)
    pub hooks: bool,
    /// Configure SSH wrapper (default: false)
    pub ssh_wrapper: bool,
    /// Dry run — report what would change without writing
    pub dry_run: bool,
}

impl Default for MaterializeOpts {
    fn default() -> Self {
        Self {
            config: true,
            remotes: true,
            hooks: false,
            ssh_wrapper: false,
            dry_run: false,
        }
    }
}
```

### Report

```rust
pub struct MaterializeReport {
    pub config_written: bool,
    pub remotes_added: Vec<String>,
    pub remotes_updated: Vec<String>,
    pub hooks_installed: bool,
}
```

## Acceptance Criteria

- [ ] `materialize(org, record, path, opts)` creates/updates `.hyperforge/config.toml` from record fields
- [ ] Git remotes are reconciled to match record's forge list
- [ ] Existing user-added remotes are not deleted
- [ ] `MaterializeOpts::dry_run` reports changes without writing
- [ ] Works on repos with no `.git` (config-only materialization)
- [ ] Works on repos with `.git` but no `.hyperforge/` (creates dir)
- [ ] Round-trip: `materialize(record) → load config → merge_from_config` yields equivalent state
- [ ] Unit tests for config generation and remote reconciliation

## Notes

- This function is synchronous (git ops + file I/O, no async needed)
- Hook installation logic can be extracted from existing `init.rs` code
- SSH wrapper logic can be extracted from existing `init.rs` code
- `HyperforgeConfig` remains the serialization format for `.toml` files — materialize builds one from a record, doesn't bypass it
