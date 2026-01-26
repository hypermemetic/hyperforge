//! Keychain bridge for secure secret storage
//!
//! This module provides access to the macOS Keychain using the `security` CLI command.
//! Secrets are stored with a service name format of `hyperforge:<org>:<forge>`.

use tokio::process::Command;

/// Bridge to the macOS Keychain for secure secret storage
pub struct KeychainBridge {
    service_prefix: String,
}

impl KeychainBridge {
    /// Create a new KeychainBridge for the given organization
    pub fn new(org_name: &str) -> Self {
        Self {
            service_prefix: format!("hyperforge:{}", org_name),
        }
    }

    /// Get the full service name for a given key
    /// Format: hyperforge:<org>:<key> (e.g., hyperforge:hypermemetic:github)
    fn service_name(&self, key: &str) -> String {
        format!("{}:{}", self.service_prefix, key)
    }

    /// Get a secret value from the keychain
    ///
    /// Uses `security find-generic-password` to retrieve the value.
    /// Returns `Ok(None)` if the secret doesn't exist.
    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let service = self.service_name(key);
        let account = std::env::var("USER").unwrap_or_else(|_| "hyperforge".to_string());

        let output = Command::new("security")
            .args(["find-generic-password", "-a", &account, "-s", &service, "-w"])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Some(value))
        } else {
            // security returns non-zero if the item doesn't exist
            Ok(None)
        }
    }

    /// Set a secret value in the keychain
    ///
    /// Uses `security add-generic-password` to store the value.
    /// Deletes any existing entry first to avoid duplicates.
    pub async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let service = self.service_name(key);
        let account = std::env::var("USER").unwrap_or_else(|_| "hyperforge".to_string());

        // Delete existing entry if present (ignore errors)
        let _ = Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output()
            .await;

        // Add new entry
        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                &service,
                "-a",
                &account,
                "-w",
                value,
                "-U", // Update if exists (shouldn't happen after delete, but safety)
            ])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    /// Check if a secret exists in the keychain
    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        Ok(self.get(key).await?.is_some())
    }

    /// Delete a secret from the keychain
    ///
    /// Uses `security delete-generic-password` to remove the entry.
    pub async fn delete(&self, key: &str) -> Result<(), String> {
        let service = self.service_name(key);

        let output = Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(())
        } else {
            // Don't error if the item didn't exist
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("could not be found") {
                Ok(())
            } else {
                Err(stderr.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_name_format() {
        let bridge = KeychainBridge::new("myorg");
        let service = bridge.service_name("github");
        assert_eq!(service, "hyperforge:myorg:github");
    }

    #[tokio::test]
    #[ignore] // Requires macOS and actual keychain access
    async fn test_set_and_get() {
        let bridge = KeychainBridge::new("test-org");
        let key = "test-token";
        let value = "test-value-12345";

        // Clean up first
        let _ = bridge.delete(key).await;

        // Set the value
        bridge.set(key, value).await.unwrap();

        // Get it back
        let retrieved = bridge.get(key).await.unwrap();
        assert_eq!(retrieved, Some(value.to_string()));

        // Clean up
        bridge.delete(key).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires macOS and actual keychain access
    async fn test_get_nonexistent() {
        let bridge = KeychainBridge::new("test-org");
        let result = bridge.get("nonexistent-key").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    #[ignore] // Requires macOS and actual keychain access
    async fn test_delete_nonexistent() {
        let bridge = KeychainBridge::new("test-org");
        // Should not error even if key doesn't exist
        bridge.delete("nonexistent-key").await.unwrap();
    }
}
