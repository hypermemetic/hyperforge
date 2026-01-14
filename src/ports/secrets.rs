//! Secrets port - interface for secure credential storage.
//!
//! This port abstracts over different secrets backends (keychain, environment
//! variables, vault, etc.), allowing the domain to retrieve credentials
//! without knowing the storage mechanism.

use async_trait::async_trait;
use thiserror::Error;

/// Errors that can occur during secrets operations.
#[derive(Debug, Error)]
pub enum SecretsError {
    /// Secret not found
    #[error("Secret not found: {key}")]
    NotFound { key: String },

    /// Failed to access secrets store
    #[error("Failed to access secrets store: {message}")]
    AccessError { message: String },

    /// Permission denied accessing secrets
    #[error("Permission denied accessing secret: {key}")]
    PermissionDenied { key: String },

    /// Secret value is invalid (e.g., not valid UTF-8)
    #[error("Invalid secret value for {key}: {message}")]
    InvalidValue { key: String, message: String },

    /// Failed to store secret
    #[error("Failed to store secret {key}: {message}")]
    StoreError { key: String, message: String },

    /// Failed to delete secret
    #[error("Failed to delete secret {key}: {message}")]
    DeleteError { key: String, message: String },

    /// Backend-specific error
    #[error("Secrets backend error: {message}")]
    BackendError { message: String },
}

impl SecretsError {
    /// Create a not found error
    pub fn not_found(key: impl Into<String>) -> Self {
        Self::NotFound { key: key.into() }
    }

    /// Create an access error
    pub fn access_error(message: impl Into<String>) -> Self {
        Self::AccessError {
            message: message.into(),
        }
    }

    /// Create a permission denied error
    pub fn permission_denied(key: impl Into<String>) -> Self {
        Self::PermissionDenied { key: key.into() }
    }

    /// Create an invalid value error
    pub fn invalid_value(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidValue {
            key: key.into(),
            message: message.into(),
        }
    }

    /// Create a store error
    pub fn store_error(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::StoreError {
            key: key.into(),
            message: message.into(),
        }
    }

    /// Create a delete error
    pub fn delete_error(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::DeleteError {
            key: key.into(),
            message: message.into(),
        }
    }

    /// Create a backend error
    pub fn backend_error(message: impl Into<String>) -> Self {
        Self::BackendError {
            message: message.into(),
        }
    }

    /// Check if this error indicates the secret doesn't exist
    pub fn is_not_found(&self) -> bool {
        matches!(self, SecretsError::NotFound { .. })
    }

    /// Get the key associated with this error, if any
    pub fn key(&self) -> Option<&str> {
        match self {
            SecretsError::NotFound { key } => Some(key),
            SecretsError::PermissionDenied { key } => Some(key),
            SecretsError::InvalidValue { key, .. } => Some(key),
            SecretsError::StoreError { key, .. } => Some(key),
            SecretsError::DeleteError { key, .. } => Some(key),
            _ => None,
        }
    }
}

/// Port for secure credential storage and retrieval.
///
/// This trait abstracts secrets management, allowing the domain to retrieve
/// API tokens and other credentials without knowing whether they come from
/// the system keychain, environment variables, a vault, or elsewhere.
///
/// # Key Naming Convention
///
/// Keys follow a hierarchical naming convention:
/// - `hyperforge/{service}/api_token` - API tokens for forges
/// - `hyperforge/{service}/ssh_key` - SSH private keys
/// - `hyperforge/org/{org}/secret` - Organization-specific secrets
///
/// # Object Safety
///
/// This trait is object-safe and can be used as `dyn SecretsPort`.
///
/// # Example
///
/// ```ignore
/// async fn get_github_token(secrets: &dyn SecretsPort) -> Result<String, SecretsError> {
///     secrets.get("hyperforge/github/api_token").await
/// }
/// ```
#[async_trait]
pub trait SecretsPort: Send + Sync {
    /// Retrieve a secret by key.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key (e.g., "hyperforge/github/api_token")
    ///
    /// # Errors
    ///
    /// * `SecretsError::NotFound` - Secret doesn't exist
    /// * `SecretsError::PermissionDenied` - No permission to read
    /// * `SecretsError::InvalidValue` - Secret exists but is invalid
    async fn get(&self, key: &str) -> Result<String, SecretsError>;

    /// Store a secret.
    ///
    /// Creates or updates the secret at the given key.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key
    /// * `value` - The secret value
    ///
    /// # Errors
    ///
    /// * `SecretsError::PermissionDenied` - No permission to write
    /// * `SecretsError::StoreError` - Failed to store
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretsError>;

    /// Delete a secret.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to delete
    ///
    /// # Errors
    ///
    /// * `SecretsError::NotFound` - Secret doesn't exist
    /// * `SecretsError::PermissionDenied` - No permission to delete
    /// * `SecretsError::DeleteError` - Failed to delete
    async fn delete(&self, key: &str) -> Result<(), SecretsError>;

    /// Check if a secret exists.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to check
    ///
    /// # Errors
    ///
    /// * `SecretsError::AccessError` - Failed to check existence
    async fn exists(&self, key: &str) -> Result<bool, SecretsError>;

    /// List all secret keys matching a prefix.
    ///
    /// # Arguments
    ///
    /// * `prefix` - The key prefix to match (e.g., "hyperforge/github/")
    ///
    /// # Returns
    ///
    /// List of matching keys (not values).
    ///
    /// # Errors
    ///
    /// * `SecretsError::AccessError` - Failed to list keys
    async fn list(&self, prefix: &str) -> Result<Vec<String>, SecretsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify SecretsError can be constructed
    #[test]
    fn test_secrets_error_constructors() {
        let err = SecretsError::not_found("github_token");
        assert!(err.is_not_found());
        assert_eq!(err.key(), Some("github_token"));

        let err = SecretsError::access_error("keychain locked");
        assert!(!err.is_not_found());
        assert!(err.key().is_none());

        let err = SecretsError::invalid_value("token", "not utf-8");
        assert!(!err.is_not_found());
        assert_eq!(err.key(), Some("token"));
    }

    // Verify SecretsPort is object-safe
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn SecretsPort) {}
}
