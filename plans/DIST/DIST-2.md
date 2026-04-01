# DIST-2: ReleasePort Trait + Forge Adapters

blocked_by: []
unlocks: [DIST-4, DIST-6]

## Scope

Create a `ReleasePort` trait for forge release operations (parallel to `ForgePort` for repos and `RegistryPort` for container images), with GitHub and Codeberg implementations.

## Types

```rust
struct ReleaseInfo {
    id: u64,
    tag_name: String,
    name: String,
    body: String,
    draft: bool,
    prerelease: bool,
    created_at: DateTime<Utc>,
    assets: Vec<AssetInfo>,
}

struct AssetInfo {
    id: u64,
    name: String,
    size_bytes: u64,
    content_type: String,
    download_url: String,
    created_at: DateTime<Utc>,
}
```

## Trait

```rust
trait ReleasePort: Send + Sync {
    async fn create_release(org, repo, tag, name, body, draft, prerelease) -> ReleaseInfo;
    async fn upload_asset(org, repo, release_id, filename, content_type, data: Vec<u8>) -> AssetInfo;
    async fn list_releases(org, repo) -> Vec<ReleaseInfo>;
    async fn get_release_by_tag(org, repo, tag) -> Option<ReleaseInfo>;
    async fn delete_release(org, repo, release_id) -> ();
    async fn list_assets(org, repo, release_id) -> Vec<AssetInfo>;
    async fn delete_asset(org, repo, asset_id) -> ();
}
```

## GitHub Adapter

- Auth: reuses existing `github/{org}/token` (needs `repo` scope for releases)
- Create: `POST /repos/{owner}/{repo}/releases` → JSON body
- Upload: `POST {upload_url}?name={filename}` → raw binary body, `Content-Type: application/octet-stream`
- List: `GET /repos/{owner}/{repo}/releases`
- Get by tag: `GET /repos/{owner}/{repo}/releases/tags/{tag}`

## Codeberg Adapter

- Auth: reuses existing `codeberg/{org}/token`
- Create: `POST /api/v1/repos/{owner}/{repo}/releases` → JSON body
- Upload: `POST /api/v1/repos/{owner}/{repo}/releases/{id}/assets?name={filename}` → **multipart/form-data** (different from GitHub)
- List: `GET /api/v1/repos/{owner}/{repo}/releases`

**Key difference**: GitHub uses raw binary upload body; Codeberg/Gitea uses multipart form. The adapter layer abstracts this.

## File Layout

```
src/adapters/releases/
    mod.rs       — ReleasePort trait, ReleaseInfo, AssetInfo, ReleaseError
    github.rs    — GitHub Releases API adapter
    codeberg.rs  — Codeberg/Gitea Releases API adapter
```

## Dependencies

- `reqwest` multipart feature needed for Codeberg uploads (add to Cargo.toml features)

## Acceptance Criteria

- [ ] ReleasePort trait compiles with all 7 methods
- [ ] GitHub adapter can create a release, upload an asset, list releases
- [ ] Codeberg adapter can create a release, upload an asset (multipart), list releases
- [ ] Unit tests for response parsing
- [ ] Auth follows the existing `{forge}/{org}/token` convention
