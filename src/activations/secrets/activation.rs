//! SecretsActivation - manages API tokens and credentials for organizations
//!
//! This activation provides methods for listing, getting, setting, and acquiring
//! secrets from various providers (keychain, environment variables, file, pass).

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{ChildRouter, PlexusError, PlexusStream};

use crate::bridge::create_secret_store;
use crate::events::SecretEvent;
use crate::storage::{GlobalConfig, HyperforgePaths, OrgConfig};
use crate::types::SecretKey;

/// Activation for managing secrets and credentials
pub struct SecretsActivation {
    paths: Arc<HyperforgePaths>,
    org_name: String,
    #[allow(dead_code)]
    org_config: OrgConfig,
}

impl SecretsActivation {
    /// Create a new SecretsActivation for the given organization
    pub fn new(paths: Arc<HyperforgePaths>, org_name: String, org_config: OrgConfig) -> Self {
        Self { paths, org_name, org_config }
    }
}

#[hub_macro::hub_methods(
    namespace = "secrets",
    version = "1.0.0",
    description = "Secret and credential management",
    crate_path = "hub_core"
)]
impl SecretsActivation {
    /// List all secrets for this organization
    #[hub_macro::hub_method(description = "List all secret keys")]
    pub async fn list(&self) -> impl Stream<Item = SecretEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let paths = self.paths.clone();

        stream! {
            let provider = GlobalConfig::load(&paths)
                .await
                .map(|c| c.secret_provider)
                .unwrap_or_default();

            let store = create_secret_store(&provider, &org_name);

            // Known secret keys
            let known_keys = vec![
                crate::types::secret::keys::GITHUB_TOKEN,
                crate::types::secret::keys::CODEBERG_TOKEN,
                crate::types::secret::keys::CRATES_TOKEN,
                crate::types::secret::keys::HACKAGE_TOKEN,
            ];

            let mut keys = Vec::new();

            for key in known_keys {
                let is_set = store.exists(key).await.unwrap_or(false);

                keys.push(SecretKey {
                    key: key.to_string(),
                    provider: provider.clone(),
                    is_set,
                });
            }

            yield SecretEvent::Listed { org_name, keys };
        }
    }

    /// Get a secret value
    #[hub_macro::hub_method(
        description = "Get a secret value",
        params(key = "Secret key name")
    )]
    pub async fn get(&self, key: String) -> impl Stream<Item = SecretEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let paths = self.paths.clone();

        stream! {
            let provider = GlobalConfig::load(&paths)
                .await
                .map(|c| c.secret_provider)
                .unwrap_or_default();
            let store = create_secret_store(&provider, &org_name);

            match store.get(&key).await {
                Ok(Some(value)) => {
                    yield SecretEvent::Retrieved {
                        org_name,
                        key,
                        value,
                    };
                }
                Ok(None) => {
                    yield SecretEvent::Error {
                        org_name,
                        key: Some(key.clone()),
                        message: format!("Secret not found: {}", key),
                    };
                }
                Err(e) => {
                    yield SecretEvent::Error {
                        org_name,
                        key: Some(key),
                        message: e,
                    };
                }
            }
        }
    }

    /// Set a secret value
    #[hub_macro::hub_method(
        description = "Set a secret value",
        params(
            key = "Secret key name",
            value = "Secret value (prompts if omitted)"
        )
    )]
    pub async fn set(
        &self,
        key: String,
        value: Option<String>,
    ) -> impl Stream<Item = SecretEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let paths = self.paths.clone();

        stream! {
            // If no value provided, request interactive input
            if value.is_none() {
                yield SecretEvent::PromptRequired {
                    org_name: org_name.clone(),
                    key: key.clone(),
                    message: format!("Enter value for {}", key),
                };
                return;
            }

            let secret_value = value.unwrap();
            let provider = GlobalConfig::load(&paths)
                .await
                .map(|c| c.secret_provider)
                .unwrap_or_default();
            let store = create_secret_store(&provider, &org_name);

            match store.set(&key, &secret_value).await {
                Ok(()) => {
                    yield SecretEvent::Updated { org_name, key };
                }
                Err(e) => {
                    yield SecretEvent::Error {
                        org_name,
                        key: Some(key),
                        message: e,
                    };
                }
            }
        }
    }

    /// Acquire a token from external source (e.g., gh CLI)
    #[hub_macro::hub_method(
        description = "Acquire token from external source",
        params(forge = "Forge to acquire token for (github, codeberg)")
    )]
    pub async fn acquire(&self, forge: String) -> impl Stream<Item = SecretEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let paths = self.paths.clone();

        stream! {
            yield SecretEvent::AcquireStarted {
                org_name: org_name.clone(),
                forge: forge.clone(),
            };

            let provider = GlobalConfig::load(&paths)
                .await
                .map(|c| c.secret_provider)
                .unwrap_or_default();
            let store = create_secret_store(&provider, &org_name);

            match forge.as_str() {
                "github" => {
                    // Try to get token from gh CLI
                    match tokio::process::Command::new("gh")
                        .args(["auth", "token"])
                        .output()
                        .await
                    {
                        Ok(output) if output.status.success() => {
                            let token = String::from_utf8_lossy(&output.stdout)
                                .trim()
                                .to_string();

                            if let Err(e) = store.set("github-token", &token).await {
                                yield SecretEvent::Error {
                                    org_name,
                                    key: Some("github-token".into()),
                                    message: e,
                                };
                                return;
                            }

                            yield SecretEvent::Acquired {
                                org_name,
                                key: "github-token".into(),
                                source: "gh CLI".into(),
                            };
                        }
                        _ => {
                            yield SecretEvent::Error {
                                org_name,
                                key: Some("github-token".into()),
                                message: "Failed to get token from gh CLI. Run 'gh auth login' first.".into(),
                            };
                        }
                    }
                }
                _ => {
                    yield SecretEvent::Error {
                        org_name,
                        key: None,
                        message: format!("Acquire not supported for forge: {}", forge),
                    };
                }
            }
        }
    }

    /// Delete a secret
    #[hub_macro::hub_method(
        description = "Delete a secret",
        params(key = "Secret key name")
    )]
    pub async fn delete(&self, key: String) -> impl Stream<Item = SecretEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let paths = self.paths.clone();

        stream! {
            let provider = GlobalConfig::load(&paths)
                .await
                .map(|c| c.secret_provider)
                .unwrap_or_default();
            let store = create_secret_store(&provider, &org_name);

            match store.delete(&key).await {
                Ok(()) => {
                    yield SecretEvent::Deleted { org_name, key };
                }
                Err(e) => {
                    yield SecretEvent::Error {
                        org_name,
                        key: Some(key),
                        message: e,
                    };
                }
            }
        }
    }
}

#[async_trait]
impl ChildRouter for SecretsActivation {
    fn router_namespace(&self) -> &str {
        "secrets"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        use hub_core::plexus::Activation;
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Secrets has no children
    }
}
