//! Codeberg adapter implementing the ForgePort trait.
//!
//! This adapter wraps the existing CodebergClient from the bridge module and
//! translates between Codeberg/Gitea API types and domain types.

use async_trait::async_trait;

use crate::bridge::{CodebergClient, ForgeClient, ForgeRepo, RepoCreateConfig};
use crate::bridge::ForgeError as BridgeForgeError;
use crate::domain::{DesiredRepo, ForgeRepoState, ObservedRepo, RepoIdentity};
use crate::ports::forge::{ForgeError, ForgePort};
use crate::types::Forge;

/// Adapter for Codeberg (Gitea-compatible) API implementing the ForgePort trait.
///
/// This adapter wraps the existing CodebergClient and translates between
/// the bridge types (API-level) and domain types (business logic).
///
/// Codeberg uses the Gitea API, so this adapter also works with self-hosted
/// Gitea instances when configured with a custom base URL.
pub struct CodebergAdapter {
    client: CodebergClient,
    token: String,
}

impl CodebergAdapter {
    /// Create a new Codeberg adapter with the given API token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            client: CodebergClient::new(),
            token: token.into(),
        }
    }

    /// Create a Codeberg adapter with a custom base URL (for self-hosted Gitea).
    pub fn with_base_url(token: impl Into<String>, base_url: &str) -> Self {
        Self {
            client: CodebergClient::with_base_url(base_url),
            token: token.into(),
        }
    }

    /// Convert a bridge ForgeRepo to a domain ObservedRepo.
    fn forge_repo_to_observed(&self, api_repo: ForgeRepo, org: &str) -> ObservedRepo {
        let identity = RepoIdentity::new(org, &api_repo.name);
        let forge_state = ForgeRepoState::found(
            Forge::Codeberg,
            api_repo.clone_url.clone(),
            api_repo.visibility.clone(),
            None, // Gitea API doesn't return a simple ID we can use directly
            api_repo.description.clone(),
        );

        ObservedRepo::new(identity).with_forge_state(forge_state)
    }

    /// Convert a DesiredRepo to the bridge RepoCreateConfig.
    fn desired_to_create_config(repo: &DesiredRepo) -> RepoCreateConfig {
        RepoCreateConfig {
            description: repo.description.clone(),
            visibility: repo.visibility.clone(),
            auto_init: false,
        }
    }

    /// Map bridge errors to port errors.
    fn map_error(&self, err: BridgeForgeError) -> ForgeError {
        match err {
            BridgeForgeError::AuthenticationFailed { message } => {
                ForgeError::auth_failed(Forge::Codeberg, message)
            }
            BridgeForgeError::TokenExpired { message, .. } => {
                ForgeError::auth_failed(Forge::Codeberg, message)
            }
            BridgeForgeError::Forbidden { message } => {
                ForgeError::permission_denied(Forge::Codeberg, message)
            }
            BridgeForgeError::RateLimited { retry_after } => {
                ForgeError::rate_limited(Forge::Codeberg, Some(retry_after.as_secs()))
            }
            BridgeForgeError::RepoNotFound { name } => {
                // Parse owner/name from the error message if possible
                let parts: Vec<&str> = name.split('/').collect();
                let identity = if parts.len() == 2 {
                    RepoIdentity::new(parts[0], parts[1])
                } else {
                    RepoIdentity::new("unknown", &name)
                };
                ForgeError::RepoNotFound(identity)
            }
            BridgeForgeError::RepoAlreadyExists { name } => {
                ForgeError::RepoAlreadyExists(RepoIdentity::new("unknown", &name))
            }
            BridgeForgeError::NetworkError(err) => {
                ForgeError::network_error(Forge::Codeberg, err.to_string())
            }
            BridgeForgeError::ApiError { status, message } => {
                ForgeError::api_error(Forge::Codeberg, format!("HTTP {}: {}", status, message))
            }
            BridgeForgeError::ServerError { status, message } => {
                ForgeError::api_error(Forge::Codeberg, format!("Server error {}: {}", status, message))
            }
        }
    }
}

#[async_trait]
impl ForgePort for CodebergAdapter {
    fn forge_type(&self) -> Forge {
        Forge::Codeberg
    }

    async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
        let api_repos = self
            .client
            .list_repos(org, &self.token)
            .await
            .map_err(|e| self.map_error(e))?;

        Ok(api_repos
            .into_iter()
            .map(|r| self.forge_repo_to_observed(r, org))
            .collect())
    }

    async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        let config = Self::desired_to_create_config(repo);

        let api_repo = self
            .client
            .create_repo(repo.name(), &config, &self.token)
            .await
            .map_err(|e| self.map_error(e))?;

        Ok(self.forge_repo_to_observed(api_repo, repo.org()))
    }

    async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        let config = RepoCreateConfig {
            description: repo.description.clone(),
            visibility: repo.visibility.clone(),
            auto_init: false,
        };

        let result = self.client
            .update_repo(repo.org(), repo.name(), &config, &self.token)
            .await
            .map_err(|e| self.map_error(e))?;

        Ok(self.forge_repo_to_observed(result, repo.org()))
    }

    async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError> {
        self.client
            .delete_repo(&identity.org, &identity.name, &self.token)
            .await
            .map_err(|e| self.map_error(e))
    }

    async fn repo_exists(&self, identity: &RepoIdentity) -> Result<bool, ForgeError> {
        // List all repos and check if ours is there
        // A more efficient implementation would use GET /repos/{owner}/{repo}
        let repos = self.list_repos(&identity.org).await?;

        Ok(repos.iter().any(|r| r.name() == identity.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Visibility;
    use std::collections::HashSet;
    use std::time::Duration;

    // =========================================================================
    // Type Conversion Tests
    // =========================================================================

    #[test]
    fn test_forge_repo_to_observed_converts_all_fields() {
        let adapter = CodebergAdapter::new("test-token");

        let api_repo = ForgeRepo {
            name: "my-repo".to_string(),
            full_name: "myorg/my-repo".to_string(),
            description: Some("A Codeberg repository".to_string()),
            visibility: Visibility::Private,
            clone_url: "https://codeberg.org/myorg/my-repo.git".to_string(),
            ssh_url: "git@codeberg.org:myorg/my-repo.git".to_string(),
        };

        let observed = adapter.forge_repo_to_observed(api_repo, "myorg");

        // Verify identity is correctly constructed
        assert_eq!(observed.identity.org, "myorg");
        assert_eq!(observed.identity.name, "my-repo");

        // Verify forge state is correctly populated
        assert!(observed.exists_on(&Forge::Codeberg));
        let state = observed.forge_states.iter()
            .find(|s| s.forge == Forge::Codeberg)
            .expect("Codeberg state should exist");

        assert!(state.exists);
        assert_eq!(state.url, Some("https://codeberg.org/myorg/my-repo.git".to_string()));
        assert_eq!(state.visibility, Some(Visibility::Private));
        assert_eq!(state.description, Some("A Codeberg repository".to_string()));
        // forge_id is None for Codeberg adapter (Gitea API doesn't expose simple ID)
        assert!(state.forge_id.is_none());
    }

    #[test]
    fn test_forge_repo_to_observed_handles_none_description() {
        let adapter = CodebergAdapter::new("test-token");

        let api_repo = ForgeRepo {
            name: "minimal-repo".to_string(),
            full_name: "org/minimal-repo".to_string(),
            description: None,
            visibility: Visibility::Public,
            clone_url: "https://codeberg.org/org/minimal-repo.git".to_string(),
            ssh_url: "git@codeberg.org:org/minimal-repo.git".to_string(),
        };

        let observed = adapter.forge_repo_to_observed(api_repo, "org");

        let state = observed.forge_states.iter()
            .find(|s| s.forge == Forge::Codeberg)
            .expect("Codeberg state should exist");

        assert!(state.description.is_none());
    }

    #[test]
    fn test_desired_to_create_config_converts_all_fields() {
        let mut forges = HashSet::new();
        forges.insert(Forge::Codeberg);

        let desired = DesiredRepo::new(
            RepoIdentity::new("myorg", "myrepo"),
            Visibility::Public,
            forges,
        )
        .with_description("A Codeberg repo");

        let config = CodebergAdapter::desired_to_create_config(&desired);

        assert_eq!(config.description, Some("A Codeberg repo".to_string()));
        assert!(matches!(config.visibility, Visibility::Public));
        assert!(!config.auto_init); // Always false for our use case
    }

    #[test]
    fn test_desired_to_create_config_handles_none_description() {
        let mut forges = HashSet::new();
        forges.insert(Forge::Codeberg);

        let desired = DesiredRepo::new(
            RepoIdentity::new("org", "repo"),
            Visibility::Private,
            forges,
        );
        // No description set

        let config = CodebergAdapter::desired_to_create_config(&desired);

        assert!(config.description.is_none());
        assert!(matches!(config.visibility, Visibility::Private));
    }

    // =========================================================================
    // Adapter Identity Tests
    // =========================================================================

    #[test]
    fn test_adapter_forge_type() {
        let adapter = CodebergAdapter::new("test-token");
        assert_eq!(adapter.forge_type(), Forge::Codeberg);
    }

    // =========================================================================
    // Error Mapping Tests
    // =========================================================================

    #[test]
    fn test_map_auth_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::AuthenticationFailed {
            message: "bad token".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::AuthenticationFailed { forge, message } => {
                assert_eq!(forge, Forge::Codeberg);
                assert_eq!(message, "bad token");
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_map_token_expired_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::TokenExpired {
            forge: Forge::Codeberg,
            message: "token revoked".to_string(),
        };
        let mapped = adapter.map_error(err);

        // TokenExpired maps to AuthenticationFailed
        assert!(matches!(mapped, ForgeError::AuthenticationFailed { .. }));
    }

    #[test]
    fn test_map_forbidden_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::Forbidden {
            message: "insufficient permissions".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::PermissionDenied { forge, message } => {
                assert_eq!(forge, Forge::Codeberg);
                assert_eq!(message, "insufficient permissions");
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_map_rate_limited_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::RateLimited {
            retry_after: Duration::from_secs(120),
        };
        let mapped = adapter.map_error(err);

        // Check retryable first before destructuring
        assert!(mapped.is_retryable());

        match mapped {
            ForgeError::RateLimited { forge, retry_after_secs } => {
                assert_eq!(forge, Forge::Codeberg);
                assert_eq!(retry_after_secs, Some(120));
            }
            _ => panic!("Expected RateLimited error"),
        }
    }

    #[test]
    fn test_map_repo_not_found_parses_owner_name() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::RepoNotFound {
            name: "myorg/myrepo".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::RepoNotFound(identity) => {
                assert_eq!(identity.org, "myorg");
                assert_eq!(identity.name, "myrepo");
            }
            _ => panic!("Expected RepoNotFound error"),
        }
    }

    #[test]
    fn test_map_repo_not_found_without_slash() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::RepoNotFound {
            name: "just-repo-name".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::RepoNotFound(identity) => {
                // When no slash, org defaults to "unknown"
                assert_eq!(identity.org, "unknown");
                assert_eq!(identity.name, "just-repo-name");
            }
            _ => panic!("Expected RepoNotFound error"),
        }
    }

    #[test]
    fn test_map_repo_already_exists_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::RepoAlreadyExists {
            name: "existing-repo".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::RepoAlreadyExists(identity) => {
                assert_eq!(identity.name, "existing-repo");
            }
            _ => panic!("Expected RepoAlreadyExists error"),
        }
    }

    #[test]
    fn test_map_api_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::ApiError {
            status: 400,
            message: "Bad request".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::ApiError { forge, message } => {
                assert_eq!(forge, Forge::Codeberg);
                assert!(message.contains("400"));
                assert!(message.contains("Bad request"));
            }
            _ => panic!("Expected ApiError"),
        }
    }

    #[test]
    fn test_map_server_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::ServerError {
            status: 503,
            message: "Service unavailable".to_string(),
        };
        let mapped = adapter.map_error(err);

        match mapped {
            ForgeError::ApiError { forge, message } => {
                assert_eq!(forge, Forge::Codeberg);
                assert!(message.contains("503"));
                assert!(message.contains("Service unavailable"));
            }
            _ => panic!("Expected ApiError"),
        }
    }

    #[test]
    fn test_all_bridge_errors_map_to_codeberg() {
        // Verify all bridge error variants have the correct forge in the mapped result
        let adapter = CodebergAdapter::new("test-token");

        let errors_with_expected_forge: Vec<(BridgeForgeError, Option<Forge>)> = vec![
            (BridgeForgeError::AuthenticationFailed { message: "x".to_string() }, Some(Forge::Codeberg)),
            (BridgeForgeError::TokenExpired { forge: Forge::Codeberg, message: "x".to_string() }, Some(Forge::Codeberg)),
            (BridgeForgeError::Forbidden { message: "x".to_string() }, Some(Forge::Codeberg)),
            (BridgeForgeError::RateLimited { retry_after: Duration::from_secs(1) }, Some(Forge::Codeberg)),
            (BridgeForgeError::RepoNotFound { name: "a/b".to_string() }, None), // RepoNotFound has no forge
            (BridgeForgeError::RepoAlreadyExists { name: "x".to_string() }, None), // RepoAlreadyExists has no forge
            (BridgeForgeError::ApiError { status: 400, message: "x".to_string() }, Some(Forge::Codeberg)),
            (BridgeForgeError::ServerError { status: 500, message: "x".to_string() }, Some(Forge::Codeberg)),
        ];

        for (bridge_err, expected_forge) in errors_with_expected_forge {
            let mapped = adapter.map_error(bridge_err);
            assert_eq!(mapped.forge().cloned(), expected_forge,
                "Mapped error should have forge {:?}", expected_forge);
        }
    }
}
