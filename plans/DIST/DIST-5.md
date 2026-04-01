# DIST-5: Cross-Compile + Package Engine

blocked_by: [DIST-3]
unlocks: [DIST-6]

## Scope

Build binaries for multiple target triples and package them into archives with binstall-compatible naming. Supports Rust (via cargo/cross) and Haskell (native + static linking).

## Target Triple Type

```rust
enum ArchiveFormat { TarGz, Zip, TarXz }

struct TargetTriple {
    triple: String,           // e.g. "x86_64-unknown-linux-gnu"
    archive_format: ArchiveFormat,
}

impl TargetTriple {
    fn is_windows(&self) -> bool;
    fn is_native(&self) -> bool;  // matches current host
    fn binary_extension(&self) -> &str;  // "" or ".exe"
}
```

### Predefined target sets

```rust
const COMMON_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

const LINUX_TARGETS: &[&str] = &[...];
const APPLE_TARGETS: &[&str] = &[...];
```

## Compilation Strategy

### Rust
1. If target == host: `cargo build --release --target {triple}`
2. If target != host: check if `cross` is installed → `cross build --release --target {triple}`
3. If `cross` not available: error with install instructions
4. Binary location: `target/{triple}/release/{binary_name}`

### Haskell
1. Native target only (cross-GHC is impractical)
2. `cabal build -O2` for optimized build
3. Optional: `--enable-executable-static` for static linking on Linux (requires musl)
4. Binary location: parsed from `cabal list-bin {executable}`
5. Future: Docker-based Linux builds when running on macOS

## Archive Packaging

### binstall naming convention

```
{name}-{target}-v{version}.tar.gz     (unix)
{name}-{target}-v{version}.zip        (windows)
```

### Archive internal structure

```
{name}-{target}-v{version}/
    {binary1}
    {binary2}
    ...
```

Multiple binaries from the same package go in one archive (e.g. hyperforge, hyperforge-auth, hyperforge-ssh).

### Implementation

```rust
struct CompileResult {
    target: TargetTriple,
    binaries: Vec<PathBuf>,      // compiled binary paths
    archive_path: Option<PathBuf>, // packaged archive
    success: bool,
    error: Option<String>,
}

async fn compile_and_package(
    repo_path: &Path,
    build_system: BuildSystemKind,
    targets: &[TargetTriple],
    binary_names: &[String],
    version: &str,
    output_dir: &Path,
) -> Vec<CompileResult>
```

## File Layout

```
src/build_system/
    cross_compile.rs  — TargetTriple, compile_and_package, archive creation
```

## Dependencies

- `flate2` — gzip compression for .tar.gz
- `tar` — already in deps
- `zip` — for Windows .zip archives (optional, can defer)

## Acceptance Criteria

- [ ] TargetTriple type with predefined sets and host detection
- [ ] Rust native compilation produces binary at expected path
- [ ] Rust cross-compilation via `cross` works for linux targets
- [ ] Haskell native compilation finds binary via `cabal list-bin`
- [ ] Archive created with correct binstall naming
- [ ] Multiple binaries packaged into single archive
- [ ] CompileResult reports success/failure per target
