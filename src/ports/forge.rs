//! Forge port - interface for forge (GitHub, Codeberg, etc.) operations.
//!
//! This port abstracts over different forge implementations, allowing the
//! domain to work with repositories without knowing which forge backend
//! is being used.

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{DesiredRepo, ObservedRepo, RepoIdentity};
use crate::types::Forge;

/// Format the retry_after_secs for display in error messages.
fn format_retry_after(secs: Option<u64>) -> String {
    match secs {
        Some(s) => format!(": retry after {}s", s),
        None => String::new(),
    }
}

/// Errors that can occur during forge operations.
#[derive(Debug, Error)]
pub enum ForgeError {
    /// Authentication failed (invalid token, expired, etc.)
    #[error("Authentication failed for {forge}: {message}")]
    AuthenticationFailed { forge: Forge, message: String },

    /// Rate limited by the forge API
    #[error("Rate limited by {forge}{}", format_retry_after(*.retry_after_secs))]
    RateLimited {
        forge: Forge,
        retry_after_secs: Option<u64>,
    },

    /// Repository not found
    #[error("Repository not found: {0}")]
    RepoNotFound(RepoIdentity),

    /// Organization not found
    #[error("Organization not found on {forge}: {org}")]
    OrgNotFound { forge: Forge, org: String },

    /// Permission denied for the operation
    #[error("Permission denied on {forge}: {message}")]
    PermissionDenied { forge: Forge, message: String },

    /// Validation error (e.g., invalid repo name)
    #[error("Validation error: {message}")]
    ValidationError { message: String },

    /// Network or connectivity error
    #[error("Network error communicating with {forge}: {message}")]
    NetworkError { forge: Forge, message: String },

    /// The forge returned an unexpected response
    #[error("Unexpected response from {forge}: {message}")]
    UnexpectedResponse { forge: Forge, message: String },

    /// Repository already exists (on create)
    #[error("Repository already exists: {0}")]
    RepoAlreadyExists(RepoIdentity),

    /// Generic forge API error
    #[error("Forge API error on {forge}: {message}")]
    ApiError { forge: Forge, message: String },
}

impl ForgeError {
    /// Create an authentication failed error
    pub fn auth_failed(forge: Forge, message: impl Into<String>) -> Self {
        Self::AuthenticationFailed {
            forge,
            message: message.into(),
        }
    }

    /// Create a rate limited error
    pub fn rate_limited(forge: Forge, retry_after_secs: Option<u64>) -> Self {
        Self::RateLimited {
            forge,
            retry_after_secs,
        }
    }

    /// Create a permission denied error
    pub fn permission_denied(forge: Forge, message: impl Into<String>) -> Self {
        Self::PermissionDenied {
            forge,
            message: message.into(),
        }
    }

    /// Create a network error
    pub fn network_error(forge: Forge, message: impl Into<String>) -> Self {
        Self::NetworkError {
            forge,
            message: message.into(),
        }
    }

    /// Create an API error
    pub fn api_error(forge: Forge, message: impl Into<String>) -> Self {
        Self::ApiError {
            forge,
            message: message.into(),
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ForgeError::RateLimited { .. } | ForgeError::NetworkError { .. }
        )
    }

    /// Get the forge associated with this error, if any
    pub fn forge(&self) -> Option<&Forge> {
        match self {
            ForgeError::AuthenticationFailed { forge, .. } => Some(forge),
            ForgeError::RateLimited { forge, .. } => Some(forge),
            ForgeError::OrgNotFound { forge, .. } => Some(forge),
            ForgeError::PermissionDenied { forge, .. } => Some(forge),
            ForgeError::NetworkError { forge, .. } => Some(forge),
            ForgeError::UnexpectedResponse { forge, .. } => Some(forge),
            ForgeError::ApiError { forge, .. } => Some(forge),
            ForgeError::RepoNotFound(_) => None,
            ForgeError::ValidationError { .. } => None,
            ForgeError::RepoAlreadyExists(_) => None,
        }
    }
}

/// Port for interacting with code forges (GitHub, Codeberg, GitLab, etc.).
///
/// This trait abstracts the operations needed to manage repositories on
/// external forges. Implementations handle the specifics of each forge's
/// API while exposing a consistent interface.
///
/// # Object Safety
///
/// This trait is object-safe and can be used as `dyn ForgePort`.
///
/// # Example
///
/// ```ignore
/// async fn sync_repo(
///     forge: &dyn ForgePort,
///     desired: &DesiredRepo,
/// ) -> Result<ObservedRepo, ForgeError> {
///     if forge.repo_exists(&desired.identity).await? {
///         forge.update_repo(desired).await
///     } else {
///         forge.create_repo(desired).await
///     }
/// }
/// ```
#[async_trait]
pub trait ForgePort: Send + Sync {
    /// Returns which forge this port communicates with.
    fn forge_type(&self) -> Forge;

    /// List all repositories in an organization.
    ///
    /// Returns observed state for all repos visible to the authenticated user.
    /// Private repos are only included if the token has sufficient permissions.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name to list repos from
    ///
    /// # Errors
    ///
    /// * `ForgeError::AuthenticationFailed` - Invalid or expired token
    /// * `ForgeError::OrgNotFound` - Organization doesn't exist
    /// * `ForgeError::RateLimited` - API rate limit exceeded
    async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError>;

    /// Create a new repository on the forge.
    ///
    /// Creates the repository with the settings specified in the desired state.
    /// Returns the observed state after creation.
    ///
    /// # Arguments
    ///
    /// * `repo` - The desired repository configuration
    ///
    /// # Errors
    ///
    /// * `ForgeError::RepoAlreadyExists` - Repository already exists
    /// * `ForgeError::ValidationError` - Invalid repo name or settings
    /// * `ForgeError::PermissionDenied` - No permission to create repos
    async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError>;

    /// Update an existing repository on the forge.
    ///
    /// Updates the repository settings to match the desired state.
    /// Returns the observed state after update.
    ///
    /// # Arguments
    ///
    /// * `repo` - The desired repository configuration
    ///
    /// # Errors
    ///
    /// * `ForgeError::RepoNotFound` - Repository doesn't exist
    /// * `ForgeError::ValidationError` - Invalid settings
    /// * `ForgeError::PermissionDenied` - No permission to update
    async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError>;

    /// Delete a repository from the forge.
    ///
    /// This is a destructive operation that removes the repository and all
    /// its data from the forge.
    ///
    /// # Arguments
    ///
    /// * `identity` - The repository to delete
    ///
    /// # Errors
    ///
    /// * `ForgeError::RepoNotFound` - Repository doesn't exist
    /// * `ForgeError::PermissionDenied` - No permission to delete
    async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError>;

    /// Check if a repository exists on the forge.
    ///
    /// Returns true if the repository exists and is accessible to the
    /// authenticated user.
    ///
    /// # Arguments
    ///
    /// * `identity` - The repository to check
    ///
    /// # Errors
    ///
    /// * `ForgeError::AuthenticationFailed` - Invalid or expired token
    async fn repo_exists(&self, identity: &RepoIdentity) -> Result<bool, ForgeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify ForgeError can be constructed
    #[test]
    fn test_forge_error_constructors() {
        let err = ForgeError::auth_failed(Forge::GitHub, "bad token");
        assert!(matches!(err, ForgeError::AuthenticationFailed { .. }));
        assert_eq!(err.forge(), Some(&Forge::GitHub));

        let err = ForgeError::rate_limited(Forge::Codeberg, Some(60));
        assert!(err.is_retryable());

        let err = ForgeError::network_error(Forge::GitLab, "timeout");
        assert!(err.is_retryable());

        let err = ForgeError::RepoNotFound(RepoIdentity::new("org", "repo"));
        assert!(!err.is_retryable());
        assert!(err.forge().is_none());
    }

    // Verify ForgePort is object-safe
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn ForgePort) {}
}
