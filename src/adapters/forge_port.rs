//! ForgePort trait - unified interface for forge operations
//!
//! This trait provides a common interface for interacting with code forges
//! (GitHub, Codeberg, etc.) and the local forge (in-memory state).

use async_trait::async_trait;
use thiserror::Error;

use crate::types::Repo;

/// Result of a conditional list operation (ETag-based)
#[derive(Debug)]
pub struct ListResult {
    /// Repos if modified, None if not modified (304)
    pub repos: Option<Vec<Repo>>,
    /// ETag from response for future conditional requests
    pub etag: Option<String>,
    /// Whether the data was modified since last check
    pub modified: bool,
}

/// Errors that can occur when interacting with a forge
#[derive(Debug, Error)]
pub enum ForgeError {
    /// Repository not found
    #[error("Repository not found: {name}")]
    RepoNotFound { name: String },

    /// Repository already exists
    #[error("Repository already exists: {name}")]
    RepoAlreadyExists { name: String },

    /// Authentication failed
    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    /// Network/connection error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// API error
    #[error("API error: {0}")]
    ApiError(String),

    /// I/O error (for LocalForge file operations)
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Serialization/deserialization error
    #[error("Serialization error: {0}")]
    SerdeError(String),
}

/// Result type for forge operations
pub type ForgeResult<T> = std::result::Result<T, ForgeError>;

/// Unified interface for forge operations
///
/// This trait is implemented by:
/// - LocalForge: In-memory state backed by repos.yaml
/// - GitHubAdapter: GitHub API client
/// - CodebergAdapter: Codeberg API client
#[async_trait]
pub trait ForgePort: Send + Sync {
    /// List all repositories for the given organization
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>>;

    /// Get a specific repository
    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo>;

    /// Create a new repository
    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;

    /// Update an existing repository
    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()>;

    /// Delete a repository
    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()>;

    /// Rename a repository
    async fn rename_repo(&self, org: &str, old_name: &str, new_name: &str) -> ForgeResult<()>;

    /// Set the default branch for a repository
    async fn set_default_branch(&self, org: &str, name: &str, branch: &str) -> ForgeResult<()>;

    /// Check if a repository exists
    async fn repo_exists(&self, org: &str, name: &str) -> ForgeResult<bool> {
        match self.get_repo(org, name).await {
            Ok(_) => Ok(true),
            Err(ForgeError::RepoNotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// List repos with conditional request support (ETag)
    ///
    /// If an `etag` from a previous response is provided, the forge may return
    /// a 304 Not Modified response, avoiding the cost of re-transferring and
    /// re-parsing the full repo list.
    ///
    /// Default implementation calls `list_repos()` and always returns `modified: true`.
    async fn list_repos_incremental(
        &self, org: &str, etag: Option<String>,
    ) -> ForgeResult<ListResult> {
        let _ = etag; // unused in default impl
        let repos = self.list_repos(org).await?;
        Ok(ListResult {
            repos: Some(repos),
            etag: None,
            modified: true,
        })
    }
}
