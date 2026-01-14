//! Storage port - interface for persisting desired and observed state.
//!
//! This port abstracts over different storage backends (filesystem, database,
//! etc.), allowing the domain to persist state without knowing the storage
//! implementation.

use async_trait::async_trait;
use std::path::PathBuf;
use thiserror::Error;

use crate::domain::{DesiredRepo, ObservedRepo};

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Organization not found in storage
    #[error("Organization not found: {0}")]
    OrgNotFound(String),

    /// Failed to read from storage
    #[error("Failed to read from storage: {message}")]
    ReadError { message: String, path: Option<PathBuf> },

    /// Failed to write to storage
    #[error("Failed to write to storage: {message}")]
    WriteError { message: String, path: Option<PathBuf> },

    /// Failed to parse stored data
    #[error("Failed to parse stored data: {message}")]
    ParseError { message: String, path: Option<PathBuf> },

    /// Failed to serialize data for storage
    #[error("Failed to serialize data: {message}")]
    SerializeError { message: String },

    /// Storage path doesn't exist or isn't accessible
    #[error("Storage path not accessible: {path}")]
    PathNotAccessible { path: PathBuf },

    /// Permission denied accessing storage
    #[error("Permission denied accessing storage: {path}")]
    PermissionDenied { path: PathBuf },

    /// Data integrity error (e.g., corrupted file, invalid state)
    #[error("Data integrity error: {message}")]
    IntegrityError { message: String },

    /// Generic I/O error
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

impl StorageError {
    /// Create a read error with a path
    pub fn read_error(message: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self::ReadError {
            message: message.into(),
            path,
        }
    }

    /// Create a write error with a path
    pub fn write_error(message: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self::WriteError {
            message: message.into(),
            path,
        }
    }

    /// Create a parse error with a path
    pub fn parse_error(message: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self::ParseError {
            message: message.into(),
            path,
        }
    }

    /// Create a serialize error
    pub fn serialize_error(message: impl Into<String>) -> Self {
        Self::SerializeError {
            message: message.into(),
        }
    }

    /// Create an integrity error
    pub fn integrity_error(message: impl Into<String>) -> Self {
        Self::IntegrityError {
            message: message.into(),
        }
    }

    /// Get the path associated with this error, if any
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            StorageError::ReadError { path, .. } => path.as_ref(),
            StorageError::WriteError { path, .. } => path.as_ref(),
            StorageError::ParseError { path, .. } => path.as_ref(),
            StorageError::PathNotAccessible { path } => Some(path),
            StorageError::PermissionDenied { path } => Some(path),
            _ => None,
        }
    }

    /// Check if this error indicates the data doesn't exist yet
    pub fn is_not_found(&self) -> bool {
        matches!(self, StorageError::OrgNotFound(_))
    }
}

/// Port for persisting desired and observed repository state.
///
/// This trait abstracts storage operations, allowing the domain to save
/// and load state without knowing whether it's using the filesystem,
/// a database, or some other backend.
///
/// # State Types
///
/// - **Desired state**: What the user wants - loaded from config, modified
///   through commands, saved back to config.
/// - **Synced state**: What was last successfully applied to forges - used
///   to detect drift and compute diffs.
///
/// # Object Safety
///
/// This trait is object-safe and can be used as `dyn StoragePort`.
///
/// # Example
///
/// ```ignore
/// async fn compute_changes(
///     storage: &dyn StoragePort,
///     org: &str,
/// ) -> Result<Vec<Change>, StorageError> {
///     let desired = storage.load_desired(org).await?;
///     let synced = storage.load_synced(org).await?;
///     Ok(diff(&desired, &synced))
/// }
/// ```
#[async_trait]
pub trait StoragePort: Send + Sync {
    /// Load desired repository state for an organization.
    ///
    /// Returns the list of repositories as declared in configuration.
    /// This represents user intent, not current reality.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name
    ///
    /// # Returns
    ///
    /// Empty Vec if the org exists but has no repos configured.
    ///
    /// # Errors
    ///
    /// * `StorageError::OrgNotFound` - Organization not configured
    /// * `StorageError::ParseError` - Config file is malformed
    /// * `StorageError::ReadError` - Failed to read storage
    async fn load_desired(&self, org: &str) -> Result<Vec<DesiredRepo>, StorageError>;

    /// Save desired repository state for an organization.
    ///
    /// Persists the desired state, replacing any existing configuration
    /// for this organization.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name
    /// * `repos` - The desired repositories to save
    ///
    /// # Errors
    ///
    /// * `StorageError::WriteError` - Failed to write storage
    /// * `StorageError::SerializeError` - Failed to serialize data
    /// * `StorageError::PermissionDenied` - No write permission
    async fn save_desired(&self, org: &str, repos: &[DesiredRepo]) -> Result<(), StorageError>;

    /// Load synced (last-known-applied) state for an organization.
    ///
    /// Returns the state that was last successfully synced to forges.
    /// This is used to detect drift and compute what needs to change.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name
    ///
    /// # Returns
    ///
    /// Empty Vec if nothing has been synced yet.
    ///
    /// # Errors
    ///
    /// * `StorageError::OrgNotFound` - Organization not found
    /// * `StorageError::ParseError` - Synced state is corrupted
    /// * `StorageError::ReadError` - Failed to read storage
    async fn load_synced(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError>;

    /// Save synced state for an organization.
    ///
    /// Records the current observed state as the last-known-good state.
    /// Called after successfully applying changes to forges.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name
    /// * `repos` - The observed state to save
    ///
    /// # Errors
    ///
    /// * `StorageError::WriteError` - Failed to write storage
    /// * `StorageError::SerializeError` - Failed to serialize data
    /// * `StorageError::PermissionDenied` - No write permission
    async fn save_synced(&self, org: &str, repos: &[ObservedRepo]) -> Result<(), StorageError>;

    /// List all configured organizations.
    ///
    /// Returns the names of all organizations that have been configured
    /// in storage.
    ///
    /// # Errors
    ///
    /// * `StorageError::ReadError` - Failed to read storage
    async fn list_orgs(&self) -> Result<Vec<String>, StorageError>;

    /// Check if an organization exists in storage.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization name to check
    ///
    /// # Errors
    ///
    /// * `StorageError::ReadError` - Failed to read storage
    async fn org_exists(&self, org: &str) -> Result<bool, StorageError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify StorageError can be constructed
    #[test]
    fn test_storage_error_constructors() {
        let err = StorageError::read_error("file not found", Some(PathBuf::from("/tmp/test")));
        assert!(err.path().is_some());

        let err = StorageError::OrgNotFound("myorg".to_string());
        assert!(err.is_not_found());
        assert!(err.path().is_none());

        let err = StorageError::serialize_error("invalid utf8");
        assert!(!err.is_not_found());
    }

    // Verify StoragePort is object-safe
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn StoragePort) {}
}
