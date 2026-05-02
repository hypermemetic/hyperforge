# PKGPUB-5: Hackage Publish Support

blocked_by: []
unlocks: []

## Scope

Add hackage as a publish channel for cabal projects. v5 currently supports
crates.io, npm, pypi but not hackage.

## Method

### Extend `build.publish` channel enum

Add `"hackage"` to the channel list. When channel = hackage:

```bash
# Build source distribution
cabal sdist

# Upload to hackage
cabal upload --publish dist-newstyle/sdist/{package}-{version}.tar.gz
```

### Auth
Hackage token from `secrets://hackage/token` via SecretResolver.
Passed as `--token` flag or via `HACKAGE_TOKEN` env var.

### Cabal version detection
Parse `.cabal` file for `version:` field. Already handled by v4's
`build_system/cabal.rs` — port the version parsing logic.

### Integration with registry_diff (PKGPUB-2)
Add hackage query: `GET https://hackage.haskell.org/package/{name}/preferred`
Response is JSON with version list. Compare local `.cabal` version against latest.

## Tests

### `test_hackage_publish_dry_run`
Cabal project with .cabal file. `publish --channel hackage --org foo --name bar`.
Dry-run. Assert command would be `cabal sdist && cabal upload ...`.

### `test_hackage_version_parse`
Parse version from `.cabal` file. Assert correct semver extraction.

### `test_hackage_registry_query`
Mock hackage API response. Assert version comparison works.

### `test_hackage_auth_resolution`
Assert hackage token resolved from `secrets://hackage/token`.
