# DIST-10: Distribution Config as Source of Truth

blocked_by: [DIST-6]
unlocks: []

## Scope

Add per-repo distribution configuration to `.hyperforge/config.toml` and `RepoRecord` so `build release` and `build release_all` know which channels and targets each repo publishes through — without CLI flags.

## The Problem

Today, distribution targets are entirely CLI-driven:
```bash
build release --targets x86_64-linux-gnu --forge github
build brew_formula --tap_path /path/to/tap
build binstall_init --forge github
```

Every invocation requires the user to remember which repos go where. There's no source of truth for "hyperforge publishes to crates.io, brew, and forge releases on GitHub+Codeberg with these 4 targets."

## Config Schema

### Per-repo in `.hyperforge/config.toml`:

```toml
[dist]
# Which distribution channels this repo publishes through
channels = ["forge-release", "crates-io", "brew", "ghcr"]

# Target triples for binary releases (forge-release channel)
targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-apple-darwin", "aarch64-apple-darwin"]

# Homebrew tap repo (required if "brew" is in channels)
brew_tap = "hypermemetic/homebrew-tap"

# Custom brew tap path on disk (optional, for local formula generation)
brew_tap_path = "/Users/shmendez/dev/controlflow/hypermemetic/homebrew-tap"
```

### Haskell example:

```toml
[dist]
channels = ["forge-release", "hackage", "brew"]
targets = ["native"]
brew_tap = "hypermemetic/homebrew-tap"
```

### Supported channels:

| Channel | What it does | Rust | Haskell |
|---------|-------------|------|---------|
| `forge-release` | Create release + upload binaries on configured forges | yes | yes |
| `crates-io` | `cargo publish` (existing package_publish) | yes | no |
| `hackage` | `cabal upload` (existing package_publish) | no | yes |
| `brew` | Generate Homebrew formula from release assets | yes | yes |
| `ghcr` | Build + push container image | yes | yes |
| `binstall` | Inject [package.metadata.binstall] into Cargo.toml | yes | no |

## Type Changes

### In `src/types/config.rs` (or new `src/types/dist.rs`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistConfig {
    #[serde(default)]
    pub channels: Vec<DistChannel>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brew_tap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brew_tap_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DistChannel {
    ForgeRelease,
    CratesIo,
    Hackage,
    Brew,
    Ghcr,
    Binstall,
}
```

### In `HyperforgeConfig` (`src/config/mod.rs`):

Add `dist: Option<DistConfig>` field (backward compatible — existing configs without `[dist]` deserialize to None).

### In `RepoRecord` (`src/types/repo.rs`):

Add `dist: Option<DistConfig>` field, merged during `materialize()` like other config fields.

## Behavior Changes

### `build release` / `build release_all`:
- If repo has `[dist]` config, use it for channels and targets
- CLI flags override config (e.g. `--targets` overrides `dist.targets`)
- If no `[dist]` and no CLI flags, skip the repo (no more implicit defaults)

### `build brew_formula`:
- If repo has `brew_tap` in dist config, use it
- If `brew_tap_path` is set, write formula there automatically

### New: `build dist_init`:
- Interactive-ish command to populate `[dist]` in `.hyperforge/config.toml`
- Detects build system and suggests appropriate channels
- Detects available targets from installed toolchains

### New: `build dist_show`:
- Show distribution config for a repo or workspace
- Shows which channels, targets, and tap configs are set

## Acceptance Criteria

- [ ] `DistConfig` and `DistChannel` types compile and serialize correctly
- [ ] `.hyperforge/config.toml` with `[dist]` section round-trips
- [ ] `build release` reads dist config when available
- [ ] CLI flags override dist config
- [ ] Repos without `[dist]` are skipped by release_all (unless --force)
- [ ] `build dist_show` displays current config
- [ ] `build dist_init` populates config for a repo
- [ ] Existing tests still pass (backward compat)
