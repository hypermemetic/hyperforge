//! ForgePort trait - unified interface for forge operations
//!
//! This trait provides a common interface for interacting with code forges
//! (GitHub, Codeberg, etc.) and the local forge (in-memory state).

use async_trait::async_trait;
use thiserror::Error;

use crate::types::Repo;

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

    /// Check if a repository exists
    async fn repo_exists(&self, org: &str, name: &str) -> ForgeResult<bool> {
        match self.get_repo(org, name).await {
            Ok(_) => Ok(true),
            Err(ForgeError::RepoNotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}
