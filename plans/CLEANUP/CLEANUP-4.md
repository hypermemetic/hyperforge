# CLEANUP-4: Fix plexus-transport missing example files

blocked_by: []
unlocks: [CLEANUP-8]
difficulty: trivial
cascades_to: []

## Problem

Two `[[example]]` sections in Cargo.toml reference files that were never created:
- `examples/jsexec_server.rs` — does not exist
- `examples/full_plexus.rs` — does not exist

These were declared in the initial commit (920b540, Jan 24) but the actual files were never written.
`cargo test` tries to compile examples and fails with "No such file or directory".

## Fix

**Option A (recommended — fastest):** Delete the two `[[example]]` blocks from Cargo.toml.

```toml
# DELETE these two blocks:
[[example]]
name = "jsexec_server"
path = "examples/jsexec_server.rs"

[[example]]
name = "full_plexus"
path = "examples/full_plexus.rs"
```

**Option B (if examples are wanted):** Create the example files. README has code patterns at lines 40-96.
