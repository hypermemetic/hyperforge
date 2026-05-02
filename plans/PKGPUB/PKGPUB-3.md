# PKGPUB-3: Dependency Graph Topological Sort

blocked_by: []
unlocks: [PKGPUB-4]

## Scope

Build a workspace dependency graph from manifests and produce a topological
publish order. v4 had this in `build_system/dep_graph.rs` — port the logic
to v5's `build/manifest.rs`.

## Method

### Extend manifest parsing

`PackageManifest` already has `deps: Vec<(String, String)>` (name, version).
Add a function:

```rust
pub fn build_publish_order(manifests: &[PackageManifest]) -> Result<Vec<Vec<&PackageManifest>>, String>
```

Returns tiers (each tier can publish in parallel):
- Tier 0: packages with no in-workspace deps
- Tier 1: packages whose deps are all in tier 0
- etc.

### Algorithm
1. Build adjacency map: for each package, find which of its deps are also workspace members
2. Kahn's algorithm for topological sort
3. Group into tiers by topological level
4. Detect cycles → return error with cycle members

### Integration point
`build.release_all` currently iterates in declaration order. After this ticket,
it uses `build_publish_order` to determine sequence.

## Tests

### `test_topo_sort_linear`
A → B → C. Assert order: [C], [B], [A].

### `test_topo_sort_diamond`
A → B, A → C, B → D, C → D. Assert: [D], [B, C], [A].

### `test_topo_sort_independent`
A, B, C with no cross-deps. Assert single tier: [A, B, C].

### `test_topo_sort_cycle`
A → B → A. Assert error containing both package names.

### `test_topo_sort_mixed_workspace_external`
A depends on B (workspace) and reqwest (external). Only B is in the graph.
Assert A after B, reqwest ignored.
