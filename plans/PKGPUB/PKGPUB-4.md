# PKGPUB-4: Workspace-Wide Publish with Dependency Ordering

blocked_by: [PKGPUB-2, PKGPUB-3]
unlocks: [PKGPUB-7]

## Scope

New `build.publish_workspace` method (or overhaul `release_all`) that publishes
all workspace packages in dependency order with proper error handling.

## Method

### New method: `build.publish_workspace`

Params:
- `name: String` — workspace name
- `include: Option<Vec<String>>` — glob filters
- `exclude: Option<Vec<String>>` — glob filters
- `bump: Option<String>` — "patch" | "minor" | "major" (default: "patch")
- `execute: Option<bool>` — actually publish (default: false = dry-run)
- `no_tag: Option<bool>` — skip git tags
- `no_commit: Option<bool>` — skip auto-commit after bumps

### Flow
1. Discover workspace members + parse manifests
2. Run `registry_diff` (PKGPUB-2) to identify which packages need publishing
3. Filter by `include`/`exclude`
4. Build dependency graph (PKGPUB-3) for filtered packages
5. Add transitive deps: if B is selected and depends on A, auto-add A
6. For each tier (leaves first):
   - Bump version if needed (drifted → auto patch, ahead → skip bump)
   - `cargo publish` / `npm publish` / `cabal upload`
   - Wait for registry propagation (crates.io needs ~30s)
   - Emit per-package event
7. Emit summary

### Events

```rust
PublishStep {
    package_name: String,
    version: String,
    registry: String,
    action: String,  // "publish" | "auto_bump" | "skip" | "failed"
    error: Option<String>,
}

PublishSummary {
    total: u32,
    published: u32,
    failed: u32,
    skipped: u32,
    auto_bumped: u32,
    tags_created: u32,
}
```

### Error handling
- First failure in a tier aborts remaining tiers (D4 spirit)
- Already-published packages in earlier tiers are reported
- Dry-run shows the full plan without executing

## Tests

### `test_publish_workspace_dry_run`
Workspace with A → B. Dry-run. Assert plan shows B first, then A. No actual publish.

### `test_publish_workspace_filters`
Workspace with A, B, C. `--include "A"`. A depends on B.
Assert both B and A in plan (transitive). C excluded.

### `test_publish_workspace_skip_up_to_date`
Package already at same version on registry. Assert status = "skip".

### `test_publish_workspace_auto_bump_drifted`
Drifted package (same version, changed files). Assert auto_bump action.
