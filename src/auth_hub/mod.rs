//! Auth Hub - Simple secret management plugin
//!
//! Provides basic secret storage using YAML files.
//! Secrets are stored at ~/.config/hyperforge/secrets.yaml

pub mod storage;
pub mod types;

use std::sync::Arc;

use async_stream::stream;
use futures::stream::Stream;
use hub_macro::hub_methods;
use serde::{Deserialize, Serialize};

use storage::{StorageError, YamlStorage};
use types::{Secret, SecretPath};

/// Auth hub events
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthEvent {
    /// Secret retrieved successfully
    Secret {
        path: String,
        value: String,
        created_at: Option<String>,
        updated_at: Option<String>,
    },

    /// Secret info (no value)
    SecretInfo {
        path: String,
        created_at: Option<String>,
        updated_at: Option<String>,
    },

    /// Success message
    Success { message: String },

    /// Error message
    Error { message: String },
}

/// Auth Hub - manages secrets in YAML storage
#[derive(Clone)]
pub struct AuthHub {
    storage: Arc<YamlStorage>,
}

impl AuthHub {
    /// Create a new auth hub with default storage location
    pub async fn new() -> Result<Self, StorageError> {
        let storage = YamlStorage::default_location()?;

        // Load existing secrets
        storage.load().await?;

        Ok(Self {
            storage: Arc::new(storage),
        })
    }

    /// Create auth hub with custom storage location
    pub async fn with_storage(storage: YamlStorage) -> Result<Self, StorageError> {
        storage.load().await?;
        Ok(Self {
            storage: Arc::new(storage),
        })
    }
}

#[hub_methods(
    namespace = "auth",
    version = "1.0.0",
    description = "Simple secret management with YAML storage",
    crate_path = "hub_core"
)]
impl AuthHub {
    /// Get a secret by path
    #[hub_method(
        description = "Get a secret by path",
        params(
            path = "Secret path (e.g., 'github/alice/token')"
        )
    )]
    pub async fn get_secret(
        &self,
        path: String,
    ) -> impl Stream<Item = AuthEvent> + Send + 'static {
        let storage = self.storage.clone();

        stream! {
            let secret_path = SecretPath::new(path);

            match storage.get(&secret_path) {
                Ok(secret) => {
                    yield AuthEvent::Secret {
                        path: secret.path.to_string(),
                        value: secret.value,
                        created_at: secret.created_at.map(|d| d.to_rfc3339()),
                        updated_at: secret.updated_at.map(|d| d.to_rfc3339()),
                    };
                }
                Err(e) => {
                    yield AuthEvent::Error {
                        message: format!("Failed to get secret: {}", e),
                    };
                }
            }
        }
    }

    /// Set a secret
    #[hub_method(
        description = "Set a secret value",
        params(
            path = "Secret path (e.g., 'github/alice/token')",
            value = "Secret value"
        )
    )]
    pub async fn set_secret(
        &self,
        path: String,
        value: String,
    ) -> impl Stream<Item = AuthEvent> + Send + 'static {
        let storage = self.storage.clone();

        stream! {
            let secret = Secret::new(path.clone(), value);

            match storage.set(secret).await {
                Ok(_) => {
                    yield AuthEvent::Success {
                        message: format!("Secret set: {}", path),
                    };
                }
                Err(e) => {
                    yield AuthEvent::Error {
                        message: format!("Failed to set secret: {}", e),
                    };
                }
            }
        }
    }

    /// List secrets matching a prefix
    #[hub_method(
        description = "List secrets matching a prefix",
        params(
            prefix = "Prefix to filter by (empty string for all secrets)"
        )
    )]
    pub async fn list_secrets(
        &self,
        prefix: String,
    ) -> impl Stream<Item = AuthEvent> + Send + 'static {
        let storage = self.storage.clone();

        stream! {
            match storage.list(&prefix) {
                Ok(secrets) => {
                    for info in secrets {
                        yield AuthEvent::SecretInfo {
                            path: info.path.to_string(),
                            created_at: info.created_at.map(|d| d.to_rfc3339()),
                            updated_at: info.updated_at.map(|d| d.to_rfc3339()),
                        };
                    }
                }
                Err(e) => {
                    yield AuthEvent::Error {
                        message: format!("Failed to list secrets: {}", e),
                    };
                }
            }
        }
    }

    /// Delete a secret
    #[hub_method(
        description = "Delete a secret",
        params(
            path = "Secret path to delete"
        )
    )]
    pub async fn delete_secret(
        &self,
        path: String,
    ) -> impl Stream<Item = AuthEvent> + Send + 'static {
        let storage = self.storage.clone();

        stream! {
            let secret_path = SecretPath::new(path.clone());

            match storage.delete(&secret_path).await {
                Ok(_) => {
                    yield AuthEvent::Success {
                        message: format!("Secret deleted: {}", path),
                    };
                }
                Err(e) => {
                    yield AuthEvent::Error {
                        message: format!("Failed to delete secret: {}", e),
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::path::PathBuf;
    use futures::StreamExt;

    async fn create_test_hub() -> (AuthHub, TempDir) {
        let temp = TempDir::new().unwrap();
        let storage = YamlStorage::new(temp.path().join("secrets.yaml"));
        let hub = AuthHub::with_storage(storage).await.unwrap();
        (hub, temp)
    }

    #[tokio::test]
    async fn test_set_and_get_secret() {
        let (hub, _temp) = create_test_hub().await;

        // Set a secret
        let mut set_stream = hub.set_secret("github/alice/token".to_string(), "ghp_xxx".to_string()).await;
        while let Some(event) = set_stream.next().await {
            match event {
                AuthEvent::Success { .. } => {}
                AuthEvent::Error { message } => panic!("Unexpected error: {}", message),
                _ => panic!("Unexpected event"),
            }
        }

        // Get it back
        let mut get_stream = hub.get_secret("github/alice/token".to_string()).await;
        let mut found = false;
        while let Some(event) = get_stream.next().await {
            match event {
                AuthEvent::Secret { value, .. } => {
                    assert_eq!(value, "ghp_xxx");
                    found = true;
                }
                AuthEvent::Error { message } => panic!("Unexpected error: {}", message),
                _ => panic!("Unexpected event"),
            }
        }
        assert!(found);
    }

    #[tokio::test]
    async fn test_list_secrets() {
        let (hub, _temp) = create_test_hub().await;

        // Set multiple secrets
        hub.set_secret("github/alice/token".to_string(), "ghp_xxx".to_string()).await;
        hub.set_secret("github/bob/token".to_string(), "ghp_yyy".to_string()).await;
        hub.set_secret("codeberg/alice/token".to_string(), "cb_zzz".to_string()).await;

        // List github secrets
        let mut list_stream = hub.list_secrets("github/".to_string()).await;
        let mut count = 0;
        while let Some(event) = list_stream.next().await {
            match event {
                AuthEvent::SecretInfo { path, .. } => {
                    assert!(path.starts_with("github/"));
                    count += 1;
                }
                AuthEvent::Error { message } => panic!("Unexpected error: {}", message),
                _ => {}
            }
        }
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_delete_secret() {
        let (hub, _temp) = create_test_hub().await;

        // Set a secret
        hub.set_secret("github/alice/token".to_string(), "ghp_xxx".to_string()).await;

        // Delete it
        let mut delete_stream = hub.delete_secret("github/alice/token".to_string()).await;
        while let Some(event) = delete_stream.next().await {
            match event {
                AuthEvent::Success { .. } => {}
                AuthEvent::Error { message } => panic!("Unexpected error: {}", message),
                _ => panic!("Unexpected event"),
            }
        }

        // Try to get it (should fail)
        let mut get_stream = hub.get_secret("github/alice/token".to_string()).await;
        let mut got_error = false;
        while let Some(event) = get_stream.next().await {
            match event {
                AuthEvent::Error { .. } => got_error = true,
                _ => {}
            }
        }
        assert!(got_error);
    }
}
