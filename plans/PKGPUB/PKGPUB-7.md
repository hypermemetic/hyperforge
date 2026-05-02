# PKGPUB-7: Workspace Publish Filters

blocked_by: [PKGPUB-4]
unlocks: []

## Scope

Add `--include` / `--exclude` glob filters to `publish_workspace` and
`registry_diff`, matching v4's filtering behavior.

## Method

### Filter params

Both `publish_workspace` and `registry_diff` gain:
- `include: Option<Vec<String>>` — glob patterns, repo must match at least one
- `exclude: Option<Vec<String>>` — glob patterns, exclude wins over include

### Glob matching
Port v4's `glob_match` from `hubs/utils.rs`:
- `*` matches any characters
- `plexus-*` matches `plexus-core`, `plexus-macros`, etc.
- Exact match if no wildcard

### Transitive inclusion
When a filtered package depends on an excluded package, the excluded dep
is auto-included in the publish plan (with a note in the event).

## Tests

### `test_include_filter`
Workspace with A, B, C. `--include "A"`. Assert only A in results.

### `test_exclude_filter`
`--exclude "plexus-*"`. Assert plexus packages excluded.

### `test_exclude_wins`
`--include "*" --exclude "test-*"`. Assert test packages excluded.

### `test_transitive_include`
A depends on B. `--include "A" --exclude "B"`. Assert B auto-included
with note about transitive dependency.
