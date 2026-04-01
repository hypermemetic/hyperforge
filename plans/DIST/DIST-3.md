# DIST-3: Binary Target Detection

blocked_by: []
unlocks: [DIST-5]

## Scope

Extract binary/executable target names from Cargo.toml and .cabal files so the build system knows what to compile and archive.

## Rust (Cargo)

Parse `Cargo.toml` for:
- Explicit `[[bin]]` sections → `name` field
- Implicit binary: if `src/main.rs` exists and no `[[bin]]` sections, package name is the binary
- Workspace members: recurse into workspace members for their binaries

Add to `src/build_system/cargo.rs`:
```rust
fn cargo_binary_targets(path: &Path) -> Vec<BinaryTarget> { ... }
```

## Haskell (Cabal)

Parse `.cabal` file for:
- `executable {name}` stanzas → name is the binary
- Multiple executables per package (e.g. synapse has just `synapse`)

Add to `src/build_system/cabal.rs`:
```rust
fn cabal_binary_targets(path: &Path) -> Vec<BinaryTarget> { ... }
```

## Unified Type

```rust
struct BinaryTarget {
    name: String,
    build_system: BuildSystemKind,
    /// Path to the repo containing this binary
    repo_path: PathBuf,
}
```

Add to `src/build_system/mod.rs`:
```rust
fn binary_targets(path: &Path) -> Vec<BinaryTarget> { ... }
```

Dispatches to cargo or cabal based on detected build system.

## Acceptance Criteria

- [ ] `cargo_binary_targets` finds explicit `[[bin]]` targets
- [ ] `cargo_binary_targets` finds implicit binary (src/main.rs)
- [ ] `cabal_binary_targets` finds executable stanzas
- [ ] `binary_targets` auto-detects and dispatches
- [ ] Tests for each case including multi-binary packages (hyperforge has 3: hyperforge, hyperforge-auth, hyperforge-ssh)
