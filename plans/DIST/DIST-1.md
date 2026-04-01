# DIST-1: Multi-Channel Binary Distribution

## Goal

Enable hyperforge to build, package, and distribute pre-built binaries across multiple channels — forge releases (cargo binstall), Homebrew, and future package managers — for both Rust and Haskell projects in the workspace.

## Context

Today hyperforge can publish **source packages** (crates.io via `package_publish`, Hackage via `package_publish`) and **container images** (ghcr.io/codeberg via `ImagesHub`). But there's no way to distribute **pre-built binaries**.

Users want to run `cargo binstall plexus-substrate` or `brew install hypermemetic/tap/synapse` and get a binary without compiling from source. This requires:

1. Cross-compiling for multiple target triples
2. Packaging binaries with the right naming conventions
3. Creating forge releases and uploading assets
4. Generating package manager metadata (Homebrew formulas, binstall config)

### Language-specific considerations

| Language | Source publish | Binary install | Cross-compile |
|----------|--------------|----------------|---------------|
| **Rust** | crates.io | cargo binstall (forge releases) | `cross` crate or `cargo build --target` |
| **Haskell** | Hackage | Homebrew, direct download | Native only (cross-GHC is hard); Docker for Linux targets |

Haskell has no `cargo binstall` equivalent. The practical channels for Haskell binaries are Homebrew (macOS/Linux) and direct download from forge releases.

## Dependency DAG

```
DIST-2 (ReleasePort trait + forge adapters)
  │
  ├──► DIST-4 (ReleasesHub — create/upload/list/delete releases)
  │
  └──► DIST-6 (build release — all-in-one orchestrator)

DIST-3 (Binary target detection)
  │
  └──► DIST-5 (Cross-compile + package engine)
         │
         └──► DIST-6 (build release — all-in-one orchestrator)

DIST-7 (Homebrew formula generation) ◄── DIST-4
DIST-8 (Binstall metadata injection) ◄── DIST-4
DIST-9 (Workspace-wide release) ◄── DIST-6
```

## Phases

### Phase 1: Foundation (DIST-2, DIST-3) — parallelizable
- ReleasePort trait with GitHub and Codeberg adapters
- Binary target extraction from Cargo.toml / .cabal files

### Phase 2: Forge Releases (DIST-4) — depends on DIST-2
- ReleasesHub subactivation under RepoHub (list, create, upload, delete)
- First usable milestone: manual upload of pre-built binaries

### Phase 3: Cross-Compilation (DIST-5) — depends on DIST-3
- Target triple type system
- Compilation via native cargo, cross, or Docker
- Archive packaging with binstall-compatible naming
- Haskell: native target + static linking

### Phase 4: All-in-One (DIST-6) — depends on DIST-4 + DIST-5
- `build release` command: compile → package → create release → upload
- Streaming progress events
- Second usable milestone: one command to release

### Phase 5: Package Managers (DIST-7, DIST-8) — depends on DIST-4
- Homebrew formula generation from release assets
- `[package.metadata.binstall]` injection into Cargo.toml
- Future: Nix flake generation

### Phase 6: Workspace Scale (DIST-9) — depends on DIST-6
- Release all packages in dependency order
- Parallel cross-compilation across repos

## Success Criteria

- `synapse lforge hyperforge build release --path . --tag v4.1.0 --targets x86_64-unknown-linux-gnu,aarch64-apple-darwin` builds and uploads binaries
- `cargo binstall hyperforge` installs from the release
- `brew install hypermemetic/tap/synapse` installs the Haskell binary
- Both GitHub and Codeberg releases are populated
