# FORGE-5: Update `symmetric_sync.rs` — `SyncOp::RemoteOnly`, filter

blocked_by: [FORGE-2]
unlocks: [FORGE-6]

## Scope

Update the sync service to use the flat `forges` field and relabel the `Delete` sync operation as `RemoteOnly`.

## File

`src/services/symmetric_sync.rs`

## Changes

### `SyncOp` enum

Rename `Delete` to `RemoteOnly`:

```rust
pub enum SyncOp {
    Create,      // in local, not on remote
    Update,      // differs
    RemoteOnly,  // was: Delete — on remote, not in local
    InSync,
}
```

### `SyncDiff` helper methods

Rename `to_delete()` to `remote_only()`.

### `sync_with_origins()` filter

Change from:
```rust
r.origin == forge_type || r.mirrors.contains(&forge_type)
```
To:
```rust
r.forges.contains(&forge_type)
```

### Display / label updates

Anywhere the operation is displayed as `"delete"`, change to `"remote_only"`.

## Acceptance criteria

- `SyncOp::RemoteOnly` variant exists, `Delete` is gone
- `remote_only()` method replaces `to_delete()`
- Filter uses `r.forges.contains(&forge_type)`
- No references to `origin` or `mirrors` remain in sync code
