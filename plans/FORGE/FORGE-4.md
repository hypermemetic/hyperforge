# FORGE-4: Update adapter `to_repo()` methods

blocked_by: [FORGE-2]
unlocks: [FORGE-6]

## Scope

Update all forge adapter `to_repo()` conversions and `local_forge.rs` tests to use the new `forges: Vec<Forge>` field.

## Files

- `src/adapters/github.rs`
- `src/adapters/codeberg.rs`
- `src/adapters/gitlab.rs`
- `src/adapters/local_forge.rs`

## Changes

### GitHub adapter (`github.rs`)

In `to_repo()`, change struct literal from:
```rust
origin: Forge::GitHub,
mirrors: Vec::new(),
```
To:
```rust
forges: vec![Forge::GitHub],
```

### Codeberg adapter (`codeberg.rs`)

Same pattern — `Forge::Codeberg`.

### GitLab adapter (`gitlab.rs`)

Same pattern — `Forge::GitLab`.

### LocalForge (`local_forge.rs`)

Update any test fixtures that construct `Repo` with `origin`/`mirrors` to use the new constructor or `forges` field.

## Acceptance criteria

- All adapters construct `Repo` with `forges: vec![Forge::X]`
- No references to `origin` or `mirrors` remain in adapter code
- LocalForge tests compile and pass
