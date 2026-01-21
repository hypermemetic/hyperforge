//! Secret store trait and implementations for different providers.
//!
//! Supports multiple backends: Keychain (macOS), environment variables, file-based, and pass.

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::process::Command;

use crate::types::SecretProvider;

/// Trait for secret storage backends.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Get a secret value by key.
    /// Returns `Ok(None)` if the secret doesn't exist.
    async fn get(&self, key: &str) -> Result<Option<String>, String>;

    /// Set a secret value.
    async fn set(&self, key: &str, value: &str) -> Result<(), String>;

    /// Check if a secret exists.
    async fn exists(&self, key: &str) -> Result<bool, String> {
        Ok(self.get(key).await?.is_some())
    }

    /// Delete a secret.
    async fn delete(&self, key: &str) -> Result<(), String>;

    /// List all keys (if supported by the backend).
    /// Returns empty vec if listing is not supported.
    async fn list_keys(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

/// Create a secret store based on the provider configuration.
pub fn create_secret_store(provider: &SecretProvider, org_name: &str) -> Box<dyn SecretStore> {
    match provider {
        SecretProvider::Keychain => Box::new(KeychainStore::new(org_name)),
        SecretProvider::Env => Box::new(EnvStore::new(org_name)),
        SecretProvider::File => Box::new(FileStore::new(org_name)),
        SecretProvider::Pass => Box::new(PassStore::new(org_name)),
    }
}

// ============================================================================
// KeychainStore - macOS Keychain via `security` CLI
// ============================================================================

/// macOS Keychain secret store using the `security` CLI.
pub struct KeychainStore {
    service_prefix: String,
}

impl KeychainStore {
    pub fn new(org_name: &str) -> Self {
        Self {
            service_prefix: format!("hyperforge:{}", org_name),
        }
    }

    fn service_name(&self, key: &str) -> String {
        format!("{}:{}", self.service_prefix, key)
    }

    fn account() -> String {
        std::env::var("USER").unwrap_or_else(|_| "hyperforge".to_string())
    }
}

#[async_trait]
impl SecretStore for KeychainStore {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let service = self.service_name(key);
        let account = Self::account();

        let output = Command::new("security")
            .args(["find-generic-password", "-a", &account, "-s", &service, "-w"])
            .output()
            .await
            .map_err(|e| format!("Failed to run security command: {}", e))?;

        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Some(value))
        } else {
            // security returns non-zero if the item doesn't exist
            Ok(None)
        }
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let service = self.service_name(key);
        let account = Self::account();

        // Delete existing entry if present (ignore errors)
        let _ = Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output()
            .await;

        // Add new entry
        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-s", &service,
                "-a", &account,
                "-w", value,
                "-U", // Update if exists
            ])
            .output()
            .await
            .map_err(|e| format!("Failed to run security command: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        let service = self.service_name(key);

        let output = Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output()
            .await
            .map_err(|e| format!("Failed to run security command: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("could not be found") {
                Ok(()) // Not an error if it didn't exist
            } else {
                Err(stderr.to_string())
            }
        }
    }
}

// ============================================================================
// EnvStore - Environment variables
// ============================================================================

/// Environment variable secret store.
///
/// Keys are transformed to: `HYPERFORGE_<ORG>_<KEY>` (uppercase, hyphens to underscores)
/// Example: `github-token` for org `hypermemetic` -> `HYPERFORGE_HYPERMEMETIC_GITHUB_TOKEN`
pub struct EnvStore {
    prefix: String,
}

impl EnvStore {
    pub fn new(org_name: &str) -> Self {
        Self {
            prefix: format!("HYPERFORGE_{}", org_name.to_uppercase().replace('-', "_")),
        }
    }

    fn env_key(&self, key: &str) -> String {
        format!("{}_{}", self.prefix, key.to_uppercase().replace('-', "_"))
    }
}

#[async_trait]
impl SecretStore for EnvStore {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let env_key = self.env_key(key);
        Ok(std::env::var(&env_key).ok())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let env_key = self.env_key(key);
        // Note: This only sets for the current process. For persistence,
        // users should set the env var in their shell config.
        std::env::set_var(&env_key, value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        let env_key = self.env_key(key);
        std::env::remove_var(&env_key);
        Ok(())
    }
}

// ============================================================================
// FileStore - File-based secrets
// ============================================================================

/// File-based secret store.
///
/// Secrets are stored in: `~/.config/hyperforge/secrets/<org>/<key>`
/// Files are created with mode 0600 for security.
pub struct FileStore {
    base_path: PathBuf,
}

impl FileStore {
    pub fn new(org_name: &str) -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("hyperforge")
            .join("secrets")
            .join(org_name);
        Self { base_path: base }
    }

    fn secret_path(&self, key: &str) -> PathBuf {
        // Sanitize key to prevent path traversal
        let safe_key = key
            .replace('/', "_")
            .replace('\\', "_")
            .replace("..", "_");
        self.base_path.join(safe_key)
    }
}

#[async_trait]
impl SecretStore for FileStore {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let path = self.secret_path(key);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(content.trim().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Failed to read secret file: {}", e)),
        }
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let path = self.secret_path(key);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create secrets directory: {}", e))?;
        }

        // Write the secret
        tokio::fs::write(&path, value)
            .await
            .map_err(|e| format!("Failed to write secret file: {}", e))?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms)
                .map_err(|e| format!("Failed to set file permissions: {}", e))?;
        }

        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        let path = self.secret_path(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to delete secret file: {}", e)),
        }
    }

    async fn list_keys(&self) -> Result<Vec<String>, String> {
        let mut keys = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.base_path).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("Failed to read secrets directory: {}", e)),
        };

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            if let Some(name) = entry.file_name().to_str() {
                keys.push(name.to_string());
            }
        }

        Ok(keys)
    }
}

// ============================================================================
// PassStore - pass (Unix password manager)
// ============================================================================

/// Pass (the standard Unix password manager) secret store.
///
/// Keys are stored at: `hyperforge/<org>/<key>`
/// Requires `pass` to be installed and initialized.
pub struct PassStore {
    prefix: String,
}

impl PassStore {
    pub fn new(org_name: &str) -> Self {
        Self {
            prefix: format!("hyperforge/{}", org_name),
        }
    }

    fn pass_path(&self, key: &str) -> String {
        format!("{}/{}", self.prefix, key)
    }
}

#[async_trait]
impl SecretStore for PassStore {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let path = self.pass_path(key);

        let output = Command::new("pass")
            .args(["show", &path])
            .output()
            .await
            .map_err(|e| format!("Failed to run pass: {}", e))?;

        if output.status.success() {
            // pass outputs the secret followed by a newline
            let value = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            Ok(Some(value))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not in the password store") {
                Ok(None)
            } else {
                Err(stderr.to_string())
            }
        }
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let path = self.pass_path(key);

        // Use pass insert with echo to avoid interactive prompt
        let mut child = Command::new("pass")
            .args(["insert", "--force", "--multiline", &path])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn pass: {}", e))?;

        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(value.as_bytes()).await.map_err(|e| e.to_string())?;
            stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
        }

        let output = child.wait_with_output().await.map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        let path = self.pass_path(key);

        let output = Command::new("pass")
            .args(["rm", "--force", &path])
            .output()
            .await
            .map_err(|e| format!("Failed to run pass: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not in the password store") {
                Ok(()) // Not an error if it didn't exist
            } else {
                Err(stderr.to_string())
            }
        }
    }

    async fn list_keys(&self) -> Result<Vec<String>, String> {
        let output = Command::new("pass")
            .args(["ls", &self.prefix])
            .output()
            .await
            .map_err(|e| format!("Failed to run pass: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not in the password store") {
                return Ok(vec![]);
            }
            return Err(stderr.to_string());
        }

        // Parse pass ls output (tree format)
        let stdout = String::from_utf8_lossy(&output.stdout);
        let keys: Vec<String> = stdout
            .lines()
            .skip(1) // Skip the directory name header
            .filter_map(|line| {
                // Lines are like "├── key" or "└── key"
                let trimmed = line.trim_start_matches(['│', '├', '└', '─', ' '].as_ref());
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect();

        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_key_format() {
        let store = EnvStore::new("hypermemetic");
        assert_eq!(store.env_key("github-token"), "HYPERFORGE_HYPERMEMETIC_GITHUB_TOKEN");
        assert_eq!(store.env_key("codeberg-token"), "HYPERFORGE_HYPERMEMETIC_CODEBERG_TOKEN");
    }

    #[test]
    fn test_file_path_sanitization() {
        let store = FileStore::new("test-org");
        let path = store.secret_path("../../../etc/passwd");
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn test_pass_path_format() {
        let store = PassStore::new("hypermemetic");
        assert_eq!(store.pass_path("github-token"), "hyperforge/hypermemetic/github-token");
    }
}
