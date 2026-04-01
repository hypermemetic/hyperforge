//! Release adapters for creating, uploading, and managing forge releases.
//!
//! Separate from ForgePort — different domain (release artifacts), different API
//! surfaces (upload endpoints, asset management).

pub mod codeberg;
pub mod github;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A release on a forge (GitHub Release, Codeberg/Gitea Release, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub id: u64,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
    pub created_at: DateTime<Utc>,
    pub assets: Vec<AssetInfo>,
}

/// An asset attached to a release (binary, archive, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    pub content_type: String,
    pub download_url: String,
    pub created_at: DateTime<Utc>,
}

/// Errors from release operations
#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error: {0}")]
    ApiError(String),
}

pub type ReleaseResult<T> = Result<T, ReleaseError>;

/// Trait for forge release operations
#[async_trait]
pub trait ReleasePort: Send + Sync {
    /// Create a new release on the forge
    async fn create_release(
        &self,
        org: &str,
        repo: &str,
        tag: &str,
        name: &str,
        body: &str,
        draft: bool,
        prerelease: bool,
    ) -> ReleaseResult<ReleaseInfo>;

    /// Upload a binary asset to an existing release
    async fn upload_asset(
        &self,
        org: &str,
        repo: &str,
        release_id: u64,
        filename: &str,
        content_type: &str,
        data: Vec<u8>,
    ) -> ReleaseResult<AssetInfo>;

    /// List all releases for a repository
    async fn list_releases(&self, org: &str, repo: &str) -> ReleaseResult<Vec<ReleaseInfo>>;

    /// Get a specific release by its git tag
    async fn get_release_by_tag(
        &self,
        org: &str,
        repo: &str,
        tag: &str,
    ) -> ReleaseResult<Option<ReleaseInfo>>;

    /// Delete a release by ID
    async fn delete_release(&self, org: &str, repo: &str, release_id: u64) -> ReleaseResult<()>;

    /// List assets attached to a release
    async fn list_assets(
        &self,
        org: &str,
        repo: &str,
        release_id: u64,
    ) -> ReleaseResult<Vec<AssetInfo>>;

    /// Delete a specific asset by ID
    async fn delete_asset(&self, org: &str, repo: &str, asset_id: u64) -> ReleaseResult<()>;
}
