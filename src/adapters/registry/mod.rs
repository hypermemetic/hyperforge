//! Container registry adapters for listing, pushing, and deleting images.
//!
//! Separate from `ForgePort` — different domain (OCI/Docker), different auth scopes,
//! different API surfaces.

pub mod codeberg;
pub mod github;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A container package (image) in a registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub tag_count: usize,
    pub latest_tag: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

/// A container image tag/version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageTag {
    pub tag: String,
    pub digest: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

/// Errors from container registry operations
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error: {0}")]
    ApiError(String),
}

pub type RegistryResult<T> = Result<T, RegistryError>;

/// Trait for container registry operations
#[async_trait]
pub trait RegistryPort: Send + Sync {
    /// List all container packages for an org
    async fn list_packages(&self, org: &str) -> RegistryResult<Vec<PackageInfo>>;

    /// List image tags for a specific package
    async fn list_images(&self, org: &str, repo: &str) -> RegistryResult<Vec<ImageTag>>;

    /// Delete a specific image tag
    async fn delete_image(&self, org: &str, repo: &str, tag: &str) -> RegistryResult<()>;

    /// Registry host (e.g. "ghcr.io")
    fn registry_host(&self) -> &str;
}
