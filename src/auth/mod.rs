//! Authentication and secret management
//!
//! Provides secure access to forge tokens via macOS Keychain.

pub mod keychain;

use async_trait::async_trait;
pub use keychain::KeychainBridge;

/// Trait for secret providers
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Get a secret by key
    /// For forge tokens, key format: "<forge>" (e.g., "github", "codeberg")
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<String>>;
}

/// Keychain-based auth provider for forge tokens
pub struct KeychainAuthProvider {
    bridge: KeychainBridge,
}

impl KeychainAuthProvider {
    /// Create a new KeychainAuthProvider for the given organization
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            bridge: KeychainBridge::new(&org.into()),
        }
    }

    /// Set a token in the keychain
    pub async fn set_token(&self, forge: &str, token: &str) -> anyhow::Result<()> {
        self.bridge.set(forge, token).await
            .map_err(|e| anyhow::anyhow!("Failed to set token in keychain: {}", e))
    }

    /// Delete a token from the keychain
    pub async fn delete_token(&self, forge: &str) -> anyhow::Result<()> {
        self.bridge.delete(forge).await
            .map_err(|e| anyhow::anyhow!("Failed to delete token from keychain: {}", e))
    }

    /// Check if a token exists in the keychain
    pub async fn has_token(&self, forge: &str) -> anyhow::Result<bool> {
        self.bridge.exists(forge).await
            .map_err(|e| anyhow::anyhow!("Failed to check keychain: {}", e))
    }
}

#[async_trait]
impl AuthProvider for KeychainAuthProvider {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.bridge.get(key).await
            .map_err(|e| anyhow::anyhow!("Failed to get secret from keychain: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keychain_auth_provider_new() {
        let provider = KeychainAuthProvider::new("testorg");
        // Should not panic
        drop(provider);
    }

    #[tokio::test]
    #[ignore] // Requires macOS keychain access
    async fn test_keychain_auth_provider_roundtrip() {
        let provider = KeychainAuthProvider::new("test-org");

        // Set a token
        provider.set_token("test-forge", "test-token-123").await.unwrap();

        // Get it back
        let token = provider.get_secret("test-forge").await.unwrap();
        assert_eq!(token, Some("test-token-123".to_string()));

        // Check exists
        assert!(provider.has_token("test-forge").await.unwrap());

        // Delete it
        provider.delete_token("test-forge").await.unwrap();

        // Verify it's gone
        let token = provider.get_secret("test-forge").await.unwrap();
        assert_eq!(token, None);
    }
}
