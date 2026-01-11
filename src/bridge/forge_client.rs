//! ForgeClient trait and supporting types for unified forge API access
//!
//! This module provides a unified interface for interacting with different code forges
//! (GitHub, Codeberg, etc.) through a common trait. Each forge implements the `ForgeClient`
//! trait with forge-specific API handling.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

use crate::types::{Forge, Visibility};

/// Errors that can occur when interacting with a forge API
#[derive(Debug, Error)]
pub enum ForgeError {
    /// Authentication failed (401)
    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    /// Token is expired or invalid - cached from previous 401 response
    /// This error is returned by the ValidatedForgeClient when the token
    /// is known to be expired before making an API call (fail-fast).
    #[error("Token expired for {forge}: {message}")]
    TokenExpired { forge: Forge, message: String },

    /// Forbidden - insufficient permissions (403)
    #[error("Forbidden: {message}")]
    Forbidden { message: String },

    /// Rate limited - retry after duration (429)
    #[error("Rate limited: retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },

    /// Repository not found
    #[error("Repository not found: {name}")]
    RepoNotFound { name: String },

    /// Repository already exists
    #[error("Repository already exists: {name}")]
    RepoAlreadyExists { name: String },

    /// Network/connection error
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// API error with status code and message
    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },

    /// Server error (5xx)
    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },
}

/// Result type for forge operations
pub type ForgeResult<T> = std::result::Result<T, ForgeError>;

/// Authentication status returned by `authenticate`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatus {
    /// Whether authentication succeeded
    pub authenticated: bool,
    /// The authenticated username
    pub username: String,
    /// OAuth scopes granted to the token
    pub scopes: Vec<String>,
}

/// Repository information returned by forge APIs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeRepo {
    /// Repository name (without owner prefix)
    pub name: String,
    /// Full name including owner (e.g., "owner/repo")
    pub full_name: String,
    /// Repository description
    pub description: Option<String>,
    /// Repository visibility
    pub visibility: Visibility,
    /// HTTPS clone URL
    pub clone_url: String,
    /// SSH clone URL
    pub ssh_url: String,
}

/// Configuration for creating a new repository
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCreateConfig {
    /// Repository description
    pub description: Option<String>,
    /// Repository visibility
    pub visibility: Visibility,
    /// Whether to auto-initialize with README
    pub auto_init: bool,
}

impl Default for RepoCreateConfig {
    fn default() -> Self {
        Self {
            description: None,
            visibility: Visibility::Public,
            auto_init: false,
        }
    }
}

/// Unified trait for interacting with code forges (GitHub, Codeberg, etc.)
///
/// This trait provides a common interface for forge operations, allowing the
/// application to work with different forges without forge-specific code in
/// the business logic layer.
#[async_trait]
pub trait ForgeClient: Send + Sync {
    /// Get the forge type this client is for
    fn forge(&self) -> Forge;

    /// Authenticate with the forge using the provided token
    ///
    /// # Arguments
    /// * `token` - The API token to authenticate with
    ///
    /// # Returns
    /// * `Ok(AuthStatus)` - Authentication succeeded with user info
    /// * `Err(ForgeError::AuthenticationFailed)` - Token is invalid
    async fn authenticate(&self, token: &str) -> ForgeResult<AuthStatus>;

    /// List all repositories for the given owner
    ///
    /// # Arguments
    /// * `owner` - The owner (user or organization) to list repos for
    /// * `token` - The API token to use
    ///
    /// # Returns
    /// * `Ok(Vec<ForgeRepo>)` - List of repositories
    /// * `Err(ForgeError)` - API or network error
    async fn list_repos(&self, owner: &str, token: &str) -> ForgeResult<Vec<ForgeRepo>>;

    /// Create a new repository
    ///
    /// # Arguments
    /// * `name` - The repository name
    /// * `config` - Repository configuration
    /// * `token` - The API token to use
    ///
    /// # Returns
    /// * `Ok(ForgeRepo)` - The created repository
    /// * `Err(ForgeError::RepoAlreadyExists)` - Repository already exists
    async fn create_repo(
        &self,
        name: &str,
        config: &RepoCreateConfig,
        token: &str,
    ) -> ForgeResult<ForgeRepo>;

    /// Delete a repository
    ///
    /// # Arguments
    /// * `owner` - The repository owner
    /// * `name` - The repository name
    /// * `token` - The API token to use
    ///
    /// # Returns
    /// * `Ok(())` - Repository deleted
    /// * `Err(ForgeError::RepoNotFound)` - Repository doesn't exist
    async fn delete_repo(&self, owner: &str, name: &str, token: &str) -> ForgeResult<()>;
}

/// Factory function to create a forge client for the given forge type
pub fn create_client(forge: Forge) -> Box<dyn ForgeClient> {
    match forge {
        Forge::GitHub => Box::new(super::github::GitHubClient::new()),
        Forge::Codeberg => Box::new(super::codeberg::CodebergClient::new()),
        Forge::GitLab => {
            // GitLab support is not yet implemented
            // For now, return a placeholder that will error on all operations
            unimplemented!("GitLab client not yet implemented")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_repo_create_config_default() {
        let config = RepoCreateConfig::default();
        assert!(config.description.is_none());
        assert!(!config.auto_init);
        assert!(matches!(config.visibility, Visibility::Public));
    }

    // =========================================================================
    // ForgeError tests
    // =========================================================================

    #[test]
    fn test_error_authentication_failed() {
        let err = ForgeError::AuthenticationFailed {
            message: "Invalid token".to_string(),
        };
        assert!(err.to_string().contains("Authentication failed"));
        assert!(err.to_string().contains("Invalid token"));
    }

    #[test]
    fn test_error_token_expired() {
        let err = ForgeError::TokenExpired {
            forge: Forge::GitHub,
            message: "Token revoked".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Token expired"));
        assert!(msg.contains("github"));
        assert!(msg.contains("Token revoked"));
    }

    #[test]
    fn test_error_forbidden() {
        let err = ForgeError::Forbidden {
            message: "Insufficient permissions".to_string(),
        };
        assert!(err.to_string().contains("Forbidden"));
        assert!(err.to_string().contains("Insufficient permissions"));
    }

    #[test]
    fn test_error_rate_limited() {
        let err = ForgeError::RateLimited {
            retry_after: Duration::from_secs(60),
        };
        let msg = err.to_string();
        assert!(msg.contains("Rate limited"));
        assert!(msg.contains("60"));
    }

    #[test]
    fn test_error_repo_not_found() {
        let err = ForgeError::RepoNotFound {
            name: "my-repo".to_string(),
        };
        assert!(err.to_string().contains("not found"));
        assert!(err.to_string().contains("my-repo"));
    }

    #[test]
    fn test_error_repo_already_exists() {
        let err = ForgeError::RepoAlreadyExists {
            name: "existing-repo".to_string(),
        };
        assert!(err.to_string().contains("already exists"));
        assert!(err.to_string().contains("existing-repo"));
    }

    #[test]
    fn test_error_api_error() {
        let err = ForgeError::ApiError {
            status: 400,
            message: "Bad request".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("API error"));
        assert!(msg.contains("400"));
        assert!(msg.contains("Bad request"));
    }

    #[test]
    fn test_error_server_error() {
        let err = ForgeError::ServerError {
            status: 503,
            message: "Service unavailable".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Server error"));
        assert!(msg.contains("503"));
        assert!(msg.contains("Service unavailable"));
    }

    #[test]
    fn test_error_is_debug() {
        // Verify all error variants implement Debug
        let errors: Vec<ForgeError> = vec![
            ForgeError::AuthenticationFailed { message: "test".to_string() },
            ForgeError::TokenExpired { forge: Forge::GitHub, message: "test".to_string() },
            ForgeError::Forbidden { message: "test".to_string() },
            ForgeError::RateLimited { retry_after: Duration::from_secs(1) },
            ForgeError::RepoNotFound { name: "test".to_string() },
            ForgeError::RepoAlreadyExists { name: "test".to_string() },
            ForgeError::ApiError { status: 400, message: "test".to_string() },
            ForgeError::ServerError { status: 500, message: "test".to_string() },
        ];

        for err in errors {
            // This should compile - all variants implement Debug
            let _ = format!("{:?}", err);
        }
    }

    // =========================================================================
    // AuthStatus tests
    // =========================================================================

    #[test]
    fn test_auth_status_serialization() {
        let status = AuthStatus {
            authenticated: true,
            username: "test-user".to_string(),
            scopes: vec!["repo".to_string(), "read:org".to_string()],
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("test-user"));
        assert!(json.contains("repo"));

        let deserialized: AuthStatus = serde_json::from_str(&json).unwrap();
        assert!(deserialized.authenticated);
        assert_eq!(deserialized.username, "test-user");
        assert_eq!(deserialized.scopes.len(), 2);
    }

    // =========================================================================
    // ForgeRepo tests
    // =========================================================================

    #[test]
    fn test_forge_repo_serialization() {
        let repo = ForgeRepo {
            name: "my-repo".to_string(),
            full_name: "owner/my-repo".to_string(),
            description: Some("A test repo".to_string()),
            visibility: Visibility::Public,
            clone_url: "https://github.com/owner/my-repo.git".to_string(),
            ssh_url: "git@github.com:owner/my-repo.git".to_string(),
        };

        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("my-repo"));
        assert!(json.contains("owner/my-repo"));

        let deserialized: ForgeRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "my-repo");
        assert_eq!(deserialized.full_name, "owner/my-repo");
        assert!(deserialized.description.is_some());
    }

    #[test]
    fn test_forge_repo_with_no_description() {
        let repo = ForgeRepo {
            name: "minimal".to_string(),
            full_name: "owner/minimal".to_string(),
            description: None,
            visibility: Visibility::Private,
            clone_url: "".to_string(),
            ssh_url: "".to_string(),
        };

        let json = serde_json::to_string(&repo).unwrap();
        let deserialized: ForgeRepo = serde_json::from_str(&json).unwrap();
        assert!(deserialized.description.is_none());
    }

    // =========================================================================
    // RepoCreateConfig tests
    // =========================================================================

    #[test]
    fn test_repo_create_config_with_values() {
        let config = RepoCreateConfig {
            description: Some("My new repo".to_string()),
            visibility: Visibility::Private,
            auto_init: true,
        };

        assert_eq!(config.description, Some("My new repo".to_string()));
        assert!(matches!(config.visibility, Visibility::Private));
        assert!(config.auto_init);
    }

    #[test]
    fn test_repo_create_config_serialization() {
        let config = RepoCreateConfig {
            description: Some("Test".to_string()),
            visibility: Visibility::Public,
            auto_init: false,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: RepoCreateConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.description, Some("Test".to_string()));
    }

    // =========================================================================
    // create_client factory tests
    // =========================================================================

    #[test]
    fn test_create_client_github() {
        let client = create_client(Forge::GitHub);
        assert!(matches!(client.forge(), Forge::GitHub));
    }

    #[test]
    fn test_create_client_codeberg() {
        let client = create_client(Forge::Codeberg);
        assert!(matches!(client.forge(), Forge::Codeberg));
    }

    #[test]
    #[should_panic(expected = "not yet implemented")]
    fn test_create_client_gitlab_not_implemented() {
        let _ = create_client(Forge::GitLab);
    }
}
