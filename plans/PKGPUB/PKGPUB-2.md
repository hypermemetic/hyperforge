# PKGPUB-2: Registry Version Comparison (package_diff)

blocked_by: []
unlocks: [PKGPUB-4, PKGPUB-6]

## Scope

Add a new `build.registry_diff` method (or replace `package_diff` semantics) that
compares local package versions against their published registry versions.

## Method

### New method: `build.registry_diff`

Params:
- `name: String` — workspace name
- `filter: Option<Vec<String>>` — glob patterns to include/exclude

For each workspace member:
1. Detect build system (Cargo.toml, package.json, cabal)
2. Parse local version from manifest
3. Query registry for published version:
   - crates.io: `GET https://crates.io/api/v1/crates/{name}`
   - hackage: `GET https://hackage.haskell.org/package/{name}/preferred`
   - npm: `GET https://registry.npmjs.org/{name}`
4. Compare and emit status

### Event: `RegistryDiffEntry`

```rust
RegistryDiffEntry {
    package_name: String,
    build_system: String,       // "cargo" | "cabal" | "node"
    local_version: String,
    published_version: Option<String>,
    registry: String,           // "crates_io" | "hackage" | "npm"
    status: String,             // "up_to_date" | "ahead" | "stale" | "drifted" | "unpublished"
    changed_files: Option<Vec<String>>,  // for drifted: which files differ
}
```

### Status logic
- `unpublished` — not on registry at all
- `ahead` — local version > published
- `stale` — local version < published (local is behind)
- `up_to_date` — same version, same content
- `drifted` — same version, different content (needs bump before publish)

### Registry queries
Use `reqwest` (already a dep) for HTTP. No auth needed for public reads.
For crates.io drift detection: download published crate via
`GET https://crates.io/api/v1/crates/{name}/{version}/download` and compare.

## Tests

### `test_registry_diff_cargo_ahead`
Mock workspace with Cargo.toml version 0.5.0. Mock registry returns 0.4.0.
Assert status = "ahead".

### `test_registry_diff_unpublished`
Package not on registry. Assert status = "unpublished".

### `test_registry_diff_up_to_date`
Same version on both sides. Assert status = "up_to_date".

### `test_registry_diff_cabal`
Cabal project with .cabal file. Query hackage. Assert correct parsing.

### `test_registry_diff_node`
package.json project. Query npm. Assert correct parsing.
