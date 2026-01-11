//! Validated forge client wrapper with token expiration handling
//!
//! This module provides a wrapper around `ForgeClient` implementations that:
//! - Checks token status before making API calls (fail-fast)
//! - Automatically marks tokens as expired on 401 responses
//! - Returns `ForgeError::TokenExpired` for cached expired tokens
//!
//! # Example
//!
//! ```rust,ignore
//! use hyperforge::bridge::{create_client, ValidatedForgeClient};
//! use hyperforge::storage::{HyperforgePaths, TokenManager};
//! use hyperforge::types::Forge;
//!
//! let paths = HyperforgePaths::new();
//! let token_manager = TokenManager::new(paths);
//! let client = create_client(Forge::GitHub);
//! let validated = ValidatedForgeClient::new(client, token_manager, "my-org".to_string());
//!
//! // This will fail-fast if the token is known to be expired
//! let repos = validated.list_repos("owner", "token").await?;
//! ```

use async_trait::async_trait;
use tracing::{info, warn};

use crate::storage::TokenManager;
use crate::types::Forge;

use super::forge_client::{
    AuthStatus, ForgeClient, ForgeError, ForgeRepo, ForgeResult, RepoCreateConfig,
};

/// A wrapper around a `ForgeClient` that validates tokens before API calls
/// and marks them as expired on 401 responses.
///
/// This implements the token validation pattern described in WKSP-9:
/// 1. Before each API call, check if the token is already known to be expired
/// 2. If expired, return `ForgeError::TokenExpired` immediately (fail-fast)
/// 3. If a 401 is received, mark the token as expired and return `ForgeError::TokenExpired`
pub struct ValidatedForgeClient {
    /// The underlying forge client
    client: Box<dyn ForgeClient>,
    /// Token manager for checking/updating token status
    token_manager: TokenManager,
    /// The organization name for token lookups
    org: String,
}

impl ValidatedForgeClient {
    /// Create a new validated forge client
    ///
    /// # Arguments
    /// * `client` - The underlying forge client to wrap
    /// * `token_manager` - Token manager for status tracking
    /// * `org` - Organization name for token lookups
    pub fn new(client: Box<dyn ForgeClient>, token_manager: TokenManager, org: String) -> Self {
        Self {
            client,
            token_manager,
            org,
        }
    }

    /// Check if the token is valid before making an API call
    /// Returns an error if the token is known to be expired
    async fn check_token_status(&self) -> ForgeResult<()> {
        let forge = self.client.forge();
        let status = self
            .token_manager
            .get_token(&self.org, &forge)
            .await
            .map_err(|e| ForgeError::ApiError {
                status: 500,
                message: format!("Failed to check token status: {}", e),
            })?;

        match status {
            crate::storage::TokenStatus::Expired { error } => {
                warn!(
                    forge = %forge,
                    org = %self.org,
                    error = %error,
                    "Token is already marked as expired, failing fast"
                );
                Err(ForgeError::TokenExpired {
                    forge,
                    message: error,
                })
            }
            crate::storage::TokenStatus::Missing => {
                // Token status is unknown, allow the call to proceed
                // The actual token value comes from elsewhere (keychain, etc.)
                Ok(())
            }
            crate::storage::TokenStatus::Valid => Ok(()),
        }
    }

    /// Handle an authentication error by marking the token as expired
    async fn handle_auth_error(&self, error: &ForgeError) -> ForgeError {
        let forge = self.client.forge();

        if let ForgeError::AuthenticationFailed { message } = error {
            info!(
                forge = %forge,
                org = %self.org,
                message = %message,
                "Marking token as expired due to 401 response"
            );

            // Mark the token as expired
            if let Err(e) = self.token_manager.mark_expired(&self.org, &forge, message).await {
                warn!(
                    forge = %forge,
                    org = %self.org,
                    error = %e,
                    "Failed to mark token as expired in storage"
                );
            }

            // Convert to TokenExpired error
            return ForgeError::TokenExpired {
                forge,
                message: message.clone(),
            };
        }

        // For other errors, just clone the original
        // Note: ForgeError doesn't implement Clone, so we need to reconstruct
        match error {
            ForgeError::AuthenticationFailed { message } => ForgeError::TokenExpired {
                forge,
                message: message.clone(),
            },
            ForgeError::Forbidden { message } => ForgeError::Forbidden {
                message: message.clone(),
            },
            ForgeError::RateLimited { retry_after } => ForgeError::RateLimited {
                retry_after: *retry_after,
            },
            ForgeError::RepoNotFound { name } => ForgeError::RepoNotFound { name: name.clone() },
            ForgeError::RepoAlreadyExists { name } => {
                ForgeError::RepoAlreadyExists { name: name.clone() }
            }
            ForgeError::ApiError { status, message } => ForgeError::ApiError {
                status: *status,
                message: message.clone(),
            },
            ForgeError::ServerError { status, message } => ForgeError::ServerError {
                status: *status,
                message: message.clone(),
            },
            ForgeError::TokenExpired { forge, message } => ForgeError::TokenExpired {
                forge: forge.clone(),
                message: message.clone(),
            },
            // NetworkError contains reqwest::Error which doesn't implement Clone
            // We'll convert it to an ApiError
            ForgeError::NetworkError(e) => ForgeError::ApiError {
                status: 0,
                message: e.to_string(),
            },
        }
    }

    /// Wrap an API call result, handling 401 errors
    async fn wrap_result<T>(&self, result: ForgeResult<T>) -> ForgeResult<T> {
        match result {
            Ok(value) => {
                // On success, mark the token as valid
                let forge = self.client.forge();
                if let Err(e) = self.token_manager.mark_valid(&self.org, &forge).await {
                    warn!(
                        forge = %forge,
                        org = %self.org,
                        error = %e,
                        "Failed to mark token as valid in storage"
                    );
                }
                Ok(value)
            }
            Err(ref e @ ForgeError::AuthenticationFailed { .. }) => {
                Err(self.handle_auth_error(e).await)
            }
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl ForgeClient for ValidatedForgeClient {
    fn forge(&self) -> Forge {
        self.client.forge()
    }

    async fn authenticate(&self, token: &str) -> ForgeResult<AuthStatus> {
        // Check if token is already known to be expired
        self.check_token_status().await?;

        // Make the actual API call
        let result = self.client.authenticate(token).await;

        // Handle the result, marking expired on 401
        self.wrap_result(result).await
    }

    async fn list_repos(&self, owner: &str, token: &str) -> ForgeResult<Vec<ForgeRepo>> {
        // Check if token is already known to be expired
        self.check_token_status().await?;

        // Make the actual API call
        let result = self.client.list_repos(owner, token).await;

        // Handle the result, marking expired on 401
        self.wrap_result(result).await
    }

    async fn create_repo(
        &self,
        name: &str,
        config: &RepoCreateConfig,
        token: &str,
    ) -> ForgeResult<ForgeRepo> {
        // Check if token is already known to be expired
        self.check_token_status().await?;

        // Make the actual API call
        let result = self.client.create_repo(name, config, token).await;

        // Handle the result, marking expired on 401
        self.wrap_result(result).await
    }

    async fn delete_repo(&self, owner: &str, name: &str, token: &str) -> ForgeResult<()> {
        // Check if token is already known to be expired
        self.check_token_status().await?;

        // Make the actual API call
        let result = self.client.delete_repo(owner, name, token).await;

        // Handle the result, marking expired on 401
        self.wrap_result(result).await
    }
}

/// Factory function to create a validated forge client
///
/// This is the recommended way to create forge clients when you need
/// automatic token expiration handling.
pub fn create_validated_client(
    forge: Forge,
    token_manager: TokenManager,
    org: String,
) -> ValidatedForgeClient {
    let client = super::forge_client::create_client(forge);
    ValidatedForgeClient::new(client, token_manager, org)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::HyperforgePaths;
    use std::path::PathBuf;

    fn test_paths() -> HyperforgePaths {
        // Use a temp directory for tests
        HyperforgePaths {
            config_dir: PathBuf::from("/tmp/hyperforge-test"),
        }
    }

    #[test]
    fn test_validated_client_creation() {
        let paths = test_paths();
        let token_manager = TokenManager::new(paths);
        let client = super::super::forge_client::create_client(Forge::GitHub);
        let validated = ValidatedForgeClient::new(client, token_manager, "test-org".to_string());

        assert_eq!(validated.forge(), Forge::GitHub);
    }

    #[test]
    fn test_create_validated_client_factory() {
        let paths = test_paths();
        let token_manager = TokenManager::new(paths);
        let validated = create_validated_client(Forge::Codeberg, token_manager, "test-org".to_string());

        assert_eq!(validated.forge(), Forge::Codeberg);
    }
}
