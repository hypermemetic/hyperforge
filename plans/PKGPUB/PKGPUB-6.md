# PKGPUB-6: Auto-Bump Drifted Packages

blocked_by: [PKGPUB-2]
unlocks: []

## Scope

When `registry_diff` detects a drifted package (same version locally and on
registry but different source content), auto-bump the version before publishing.

## Method

### Detection
`registry_diff` (PKGPUB-2) already identifies `status: "drifted"` with `changed_files`.

### Auto-bump logic
In `publish_workspace` (PKGPUB-4) flow:
1. If package status is `drifted` → auto patch bump
2. Bump version in manifest (Cargo.toml / package.json / .cabal)
3. Commit with message `chore: bump {name} {old} → {new} (drift)`
4. Emit `PublishStep { action: "auto_bump" }`
5. Proceed to publish

### Standalone method: `build.fix_drift`

Params:
- `name: String` — workspace name
- `filter: Option<Vec<String>>` — glob patterns
- `dry_run: Option<bool>` — default true

Identifies all drifted packages and bumps them without publishing.
Useful for pre-publish cleanup.

## Tests

### `test_auto_bump_drifted_cargo`
Cargo package at 0.2.1 locally and on crates.io, but `src/lib.rs` differs.
Assert bumps to 0.2.2, commits.

### `test_auto_bump_drifted_cabal`
Same pattern for cabal. Assert `.cabal` version bumped.

### `test_auto_bump_dry_run`
Dry-run shows what would bump without writing.

### `test_no_bump_if_ahead`
Package is ahead (local > published). Assert no auto-bump.
