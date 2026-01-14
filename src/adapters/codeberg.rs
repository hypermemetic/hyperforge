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
        // Gitea/Codeberg doesn't have a direct update endpoint that matches our needs.
        // For now, we'll fetch the current state and return it.
        // A full implementation would use PATCH /repos/{owner}/{repo}.
        //
        // TODO: Implement proper repo update via Gitea PATCH API
        self.repo_exists(&repo.identity)
            .await?
            .then_some(())
            .ok_or_else(|| ForgeError::RepoNotFound(repo.identity.clone()))?;

        // Return a synthetic observed repo based on desired state
        // In a full implementation, we'd actually update and return the result
        let forge_state = ForgeRepoState::found(
            Forge::Codeberg,
            format!("https://codeberg.org/{}/{}", repo.org(), repo.name()),
            repo.visibility.clone(),
            None,
            repo.description.clone(),
        );

        Ok(ObservedRepo::new(repo.identity.clone()).with_forge_state(forge_state))
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

    #[test]
    fn test_desired_to_create_config() {
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
        assert!(!config.auto_init);
    }

    #[test]
    fn test_adapter_forge_type() {
        let adapter = CodebergAdapter::new("test-token");
        assert_eq!(adapter.forge_type(), Forge::Codeberg);
    }

    #[test]
    fn test_map_auth_error() {
        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::AuthenticationFailed {
            message: "bad token".to_string(),
        };
        let mapped = adapter.map_error(err);
        assert!(matches!(mapped, ForgeError::AuthenticationFailed { .. }));
    }

    #[test]
    fn test_map_rate_limited_error() {
        use std::time::Duration;

        let adapter = CodebergAdapter::new("test-token");
        let err = BridgeForgeError::RateLimited {
            retry_after: Duration::from_secs(120),
        };
        let mapped = adapter.map_error(err);
        assert!(matches!(mapped, ForgeError::RateLimited { .. }));
        assert!(mapped.is_retryable());
    }

    #[test]
    fn test_map_repo_not_found_error() {
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
}
