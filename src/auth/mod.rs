//! Authentication and secret management
//!
//! For now, reads secrets from a simple file mapping.
//! Future: WorkOS Vault integration.

use async_trait::async_trait;
use std::collections::HashMap;

/// Trait for secret providers
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Get a secret by scope path
    /// Example: "git/github/alice/repo/my-tool/token"
    async fn get_secret(&self, scope: &str) -> anyhow::Result<String>;
}

/// File-based auth provider (reads from config file)
pub struct FileAuthProvider {
    secrets: HashMap<String, String>,
}

impl FileAuthProvider {
    pub fn new() -> Self {
        Self {
            secrets: HashMap::new(),
        }
    }

    /// Load secrets from a TOML file
    pub fn load_from_file(&mut self, _path: &str) -> anyhow::Result<()> {
        todo!("Load secrets from file")
    }
}

#[async_trait]
impl AuthProvider for FileAuthProvider {
    async fn get_secret(&self, scope: &str) -> anyhow::Result<String> {
        self.secrets
            .get(scope)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Secret not found: {}", scope))
    }
}
