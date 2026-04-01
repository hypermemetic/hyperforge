# DIST-8: Binstall Metadata Injection

blocked_by: [DIST-4]
unlocks: []

## Scope

Inject `[package.metadata.binstall]` into Cargo.toml files so `cargo binstall` can discover pre-built binaries without relying on default URL guessing.

## What binstall needs

In `Cargo.toml`:

```toml
[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/v{ version }/{ name }-{ target }-v{ version }{ archive-suffix }"
bin-dir = "{ name }-{ target }-v{ version }/{ bin }{ binary-ext }"
pkg-fmt = "tgz"
```

This tells binstall exactly where to find the archive and how the binary is laid out inside.

## Method

`build binstall_init` — writes or updates binstall metadata in Cargo.toml.

### Params
- `path` — repo or workspace path
- `include` / `exclude` — repo filters
- `forge` — which forge hosts releases (default: github, determines the `{ repo }` URL)
- `dry_run` — preview changes

### Flow

1. Discover repos with Cargo.toml
2. For each, check if `[package.metadata.binstall]` already exists
3. If not, inject it using `toml_edit` (preserves formatting)
4. The `pkg-url` template uses the repo's configured forge URL

### Forge URL mapping

- GitHub: `https://github.com/{org}/{repo}`
- Codeberg: `https://codeberg.org/{org}/{repo}`

The template uses binstall's `{ repo }` variable which resolves from the `[package]` repository field in Cargo.toml.

## Acceptance Criteria

- [ ] Injects binstall metadata into Cargo.toml without clobbering existing content
- [ ] Uses `toml_edit` to preserve formatting and comments
- [ ] Skips repos that already have binstall metadata
- [ ] Works in workspace mode with filters
- [ ] Dry-run shows what would be added
