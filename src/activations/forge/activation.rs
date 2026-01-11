use async_trait::async_trait;
use async_stream::stream;
use chrono::Utc;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError,
    ChildSummary,
};
use hub_macro::hub_methods;

use crate::bridge::{create_client, KeychainBridge};
use crate::storage::{HyperforgePaths, TokenManager, TokenStatus};
use crate::events::ForgeEvent;
use crate::types::Forge;

use super::{GitHubRouter, CodebergRouter};

pub struct ForgeActivation {
    paths: Arc<HyperforgePaths>,
}

impl ForgeActivation {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }
}

#[hub_methods(
    namespace = "forge",
    version = "1.0.0",
    description = "Direct forge API access",
    crate_path = "hub_core",
    hub
)]
impl ForgeActivation {
    /// List supported forges
    #[hub_method(description = "List supported forges")]
    pub async fn list(&self) -> impl Stream<Item = ForgeEvent> + Send + 'static {
        stream! {
            // Just emit info about available forges
            yield ForgeEvent::ApiProgress {
                forge: Forge::GitHub,
                operation: "list".into(),
                message: "GitHub (github.com)".into(),
            };
            yield ForgeEvent::ApiProgress {
                forge: Forge::Codeberg,
                operation: "list".into(),
                message: "Codeberg (codeberg.org)".into(),
            };
        }
    }

    /// Check authentication status for a forge
    #[hub_method(
        description = "Check authentication status for a forge",
        params(
            forge = "Forge to check (github, codeberg, gitlab)",
            org = "Organization to check token for (optional, defaults to hypermemetic)"
        )
    )]
    pub async fn auth(
        &self,
        forge: String,
        org: Option<String>,
    ) -> impl Stream<Item = ForgeEvent> + Send + 'static {
        let paths = (*self.paths).clone();

        stream! {
            // Parse forge name
            let forge_type: Forge = match forge.parse() {
                Ok(f) => f,
                Err(e) => {
                    yield ForgeEvent::Error {
                        forge: Forge::GitHub, // Default for error display
                        operation: "auth".into(),
                        message: e,
                        status_code: None,
                    };
                    return;
                }
            };

            // Default org to hypermemetic
            let org_name = org.unwrap_or_else(|| "hypermemetic".to_string());

            yield ForgeEvent::AuthStarted {
                forge: forge_type.clone(),
                org_name: org_name.clone(),
            };

            // Determine the keychain key for this forge
            let token_key = match forge_type {
                Forge::GitHub => "github-token",
                Forge::Codeberg => "codeberg-token",
                Forge::GitLab => "gitlab-token",
            };

            // Get token from keychain
            let keychain = KeychainBridge::new(&org_name);
            let token = match keychain.get(token_key).await {
                Ok(Some(t)) => t,
                Ok(None) => {
                    // Token not found - update storage and return result
                    let token_manager = TokenManager::new(paths.clone());
                    if let Err(e) = token_manager.mark_missing(&org_name, &forge_type).await {
                        yield ForgeEvent::AuthFailed {
                            forge: forge_type.clone(),
                            org_name: org_name.clone(),
                            error: format!("Failed to update token storage: {}", e),
                        };
                        return;
                    }

                    yield ForgeEvent::AuthResult {
                        forge: forge_type,
                        org_name,
                        status: TokenStatus::Missing,
                        username: None,
                        scopes: vec![],
                        last_validated: Some(Utc::now()),
                    };
                    return;
                }
                Err(e) => {
                    yield ForgeEvent::AuthFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Failed to read from keychain: {}", e),
                    };
                    return;
                }
            };

            // Validate token via API
            let client = create_client(forge_type.clone());
            match client.authenticate(&token).await {
                Ok(auth_status) => {
                    // Token is valid - update storage
                    let token_manager = TokenManager::new(paths.clone());
                    if let Err(e) = token_manager.mark_valid(&org_name, &forge_type).await {
                        yield ForgeEvent::AuthFailed {
                            forge: forge_type.clone(),
                            org_name: org_name.clone(),
                            error: format!("Failed to update token storage: {}", e),
                        };
                        return;
                    }

                    yield ForgeEvent::AuthResult {
                        forge: forge_type,
                        org_name,
                        status: TokenStatus::Valid,
                        username: Some(auth_status.username),
                        scopes: auth_status.scopes,
                        last_validated: Some(Utc::now()),
                    };
                }
                Err(crate::bridge::ForgeError::AuthenticationFailed { message }) => {
                    // Token is expired/invalid - update storage
                    let token_manager = TokenManager::new(paths.clone());
                    if let Err(e) = token_manager.mark_expired(&org_name, &forge_type, &message).await {
                        yield ForgeEvent::AuthFailed {
                            forge: forge_type.clone(),
                            org_name: org_name.clone(),
                            error: format!("Failed to update token storage: {}", e),
                        };
                        return;
                    }

                    yield ForgeEvent::AuthResult {
                        forge: forge_type,
                        org_name,
                        status: TokenStatus::Expired { error: message },
                        username: None,
                        scopes: vec![],
                        last_validated: Some(Utc::now()),
                    };
                }
                Err(crate::bridge::ForgeError::RateLimited { retry_after }) => {
                    // Rate limited - don't mark as expired
                    yield ForgeEvent::AuthFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Rate limited. Retry after {:?}", retry_after),
                    };
                }
                Err(crate::bridge::ForgeError::NetworkError(e)) => {
                    // Network error - don't mark as expired
                    yield ForgeEvent::AuthFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Network error: {}", e),
                    };
                }
                Err(e) => {
                    // Other error - don't mark as expired
                    yield ForgeEvent::AuthFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("API error: {}", e),
                    };
                }
            }
        }
    }

    /// Refresh/update token for a forge
    #[hub_method(
        description = "Refresh/update token for a forge",
        params(
            forge = "Forge to refresh token for (github, codeberg, gitlab)",
            org = "Organization to refresh token for (optional, defaults to hypermemetic)",
            token = "Token to use (optional, will read from env var if not provided)"
        )
    )]
    pub async fn refresh(
        &self,
        forge: String,
        org: Option<String>,
        token: Option<String>,
    ) -> impl Stream<Item = ForgeEvent> + Send + 'static {
        let paths = (*self.paths).clone();

        stream! {
            // Parse forge name
            let forge_type: Forge = match forge.parse() {
                Ok(f) => f,
                Err(e) => {
                    yield ForgeEvent::Error {
                        forge: Forge::GitHub, // Default for error display
                        operation: "refresh".into(),
                        message: e,
                        status_code: None,
                    };
                    return;
                }
            };

            // Default org to hypermemetic
            let org_name = org.unwrap_or_else(|| "hypermemetic".to_string());

            yield ForgeEvent::RefreshStarted {
                forge: forge_type.clone(),
                org_name: org_name.clone(),
            };

            // Determine token: provided > env var
            let new_token = match token {
                Some(t) => t,
                None => {
                    // Try env var based on forge type
                    let env_var = match forge_type {
                        Forge::GitHub => "GITHUB_TOKEN",
                        Forge::Codeberg => "CODEBERG_TOKEN",
                        Forge::GitLab => "GITLAB_TOKEN",
                    };

                    match std::env::var(env_var) {
                        Ok(t) if !t.is_empty() => t,
                        _ => {
                            yield ForgeEvent::RefreshFailed {
                                forge: forge_type,
                                org_name,
                                error: format!(
                                    "No token provided. Pass --token or set {} environment variable",
                                    env_var
                                ),
                            };
                            return;
                        }
                    }
                }
            };

            // Validate token via API before storing
            let client = create_client(forge_type.clone());
            match client.authenticate(&new_token).await {
                Ok(_auth_status) => {
                    // Token is valid - store in keychain
                    let token_key = match forge_type {
                        Forge::GitHub => "github-token",
                        Forge::Codeberg => "codeberg-token",
                        Forge::GitLab => "gitlab-token",
                    };

                    let keychain = KeychainBridge::new(&org_name);
                    if let Err(e) = keychain.set(token_key, &new_token).await {
                        yield ForgeEvent::RefreshFailed {
                            forge: forge_type,
                            org_name,
                            error: format!("Failed to store token in keychain: {}", e),
                        };
                        return;
                    }

                    // Update token storage status
                    let token_manager = TokenManager::new(paths.clone());
                    if let Err(e) = token_manager.mark_valid(&org_name, &forge_type).await {
                        yield ForgeEvent::RefreshFailed {
                            forge: forge_type.clone(),
                            org_name: org_name.clone(),
                            error: format!("Failed to update token storage: {}", e),
                        };
                        return;
                    }

                    yield ForgeEvent::RefreshComplete {
                        forge: forge_type,
                        org_name,
                        status: TokenStatus::Valid,
                    };
                }
                Err(crate::bridge::ForgeError::AuthenticationFailed { message }) => {
                    yield ForgeEvent::RefreshFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Token validation failed: {}", message),
                    };
                }
                Err(crate::bridge::ForgeError::RateLimited { retry_after }) => {
                    yield ForgeEvent::RefreshFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Rate limited during validation. Retry after {:?}", retry_after),
                    };
                }
                Err(crate::bridge::ForgeError::NetworkError(e)) => {
                    yield ForgeEvent::RefreshFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("Network error during validation: {}", e),
                    };
                }
                Err(e) => {
                    yield ForgeEvent::RefreshFailed {
                        forge: forge_type,
                        org_name,
                        error: format!("API error during validation: {}", e),
                    };
                }
            }
        }
    }

    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![
            ChildSummary {
                namespace: "github".into(),
                description: "GitHub API".into(),
                hash: "github".into(),
            },
            ChildSummary {
                namespace: "codeberg".into(),
                description: "Codeberg API".into(),
                hash: "codeberg".into(),
            },
        ]
    }
}

#[async_trait]
impl ChildRouter for ForgeActivation {
    fn router_namespace(&self) -> &str {
        "forge"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "github" => Some(Box::new(GitHubRouter::new(self.paths.clone()))),
            "codeberg" => Some(Box::new(CodebergRouter::new(self.paths.clone()))),
            _ => None,
        }
    }
}
