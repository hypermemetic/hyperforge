use super::HyperforgePaths;
use crate::error::Result;
use crate::types::Forge;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Status of a token for a specific org/forge combination
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum TokenStatus {
    Valid,
    Expired { error: String },
    Missing,
}

impl Default for TokenStatus {
    fn default() -> Self {
        TokenStatus::Missing
    }
}

/// State tracking for a single token (per-org, per-forge)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenState {
    #[serde(flatten)]
    pub status: TokenStatus,
    pub last_checked: DateTime<Utc>,
}

impl TokenState {
    /// Create a new valid token state
    pub fn valid() -> Self {
        Self {
            status: TokenStatus::Valid,
            last_checked: Utc::now(),
        }
    }

    /// Create a new expired token state with error message
    pub fn expired(error: String) -> Self {
        Self {
            status: TokenStatus::Expired { error },
            last_checked: Utc::now(),
        }
    }

    /// Create a new missing token state
    pub fn missing() -> Self {
        Self {
            status: TokenStatus::Missing,
            last_checked: Utc::now(),
        }
    }

    /// Check if the token is valid
    pub fn is_valid(&self) -> bool {
        matches!(self.status, TokenStatus::Valid)
    }

    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        matches!(self.status, TokenStatus::Expired { .. })
    }

    /// Check if the token is missing
    pub fn is_missing(&self) -> bool {
        matches!(self.status, TokenStatus::Missing)
    }

    /// Get the error message if expired
    pub fn error(&self) -> Option<&str> {
        match &self.status {
            TokenStatus::Expired { error } => Some(error),
            _ => None,
        }
    }
}

/// Per-organization token states for all forges
pub type OrgTokenStates = HashMap<Forge, TokenState>;

/// Root structure for tokens.yaml file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokensFile {
    #[serde(default)]
    pub tokens: HashMap<String, OrgTokenStates>,
}

/// Low-level storage operations for token state file
pub struct TokenStorage {
    paths: HyperforgePaths,
}

impl TokenStorage {
    /// Create a new TokenStorage instance
    pub fn new(paths: HyperforgePaths) -> Self {
        Self { paths }
    }

    /// Get the path to the tokens.yaml file
    pub fn tokens_file(&self) -> PathBuf {
        self.paths.config_dir.join("tokens.yaml")
    }

    /// Load token states from disk
    /// Returns empty state if file doesn't exist
    pub async fn load(&self) -> Result<TokensFile> {
        let path = self.tokens_file();

        if !path.exists() {
            return Ok(TokensFile::default());
        }

        let contents = tokio::fs::read_to_string(&path).await?;
        let tokens: TokensFile = serde_yaml::from_str(&contents)?;
        Ok(tokens)
    }

    /// Save token states to disk with atomic write
    /// Writes to a temp file then renames to prevent corruption
    pub async fn save(&self, tokens: &TokensFile) -> Result<()> {
        let path = self.tokens_file();
        let temp_path = path.with_extension("yaml.tmp");

        // Ensure config directory exists
        tokio::fs::create_dir_all(&self.paths.config_dir).await?;

        // Serialize to YAML
        let contents = serde_yaml::to_string(tokens)?;

        // Write to temp file
        tokio::fs::write(&temp_path, &contents).await?;

        // Set restrictive permissions (0600) on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(&temp_path, permissions).await?;
        }

        // Atomic rename
        tokio::fs::rename(&temp_path, &path).await?;

        Ok(())
    }
}

/// High-level token state management
/// Provides business logic for token validation and state tracking
pub struct TokenManager {
    storage: TokenStorage,
}

impl TokenManager {
    /// Create a new TokenManager
    pub fn new(paths: HyperforgePaths) -> Self {
        Self {
            storage: TokenStorage::new(paths),
        }
    }

    /// Get the current token status for an org/forge combination
    /// Returns Missing if no state exists
    pub async fn get_token(&self, org: &str, forge: &Forge) -> Result<TokenStatus> {
        let tokens = self.storage.load().await?;

        let status = tokens
            .tokens
            .get(org)
            .and_then(|org_tokens| org_tokens.get(forge))
            .map(|state| state.status.clone())
            .unwrap_or(TokenStatus::Missing);

        Ok(status)
    }

    /// Get the full token state for an org/forge combination
    /// Returns None if no state exists
    pub async fn get_token_state(&self, org: &str, forge: &Forge) -> Result<Option<TokenState>> {
        let tokens = self.storage.load().await?;

        let state = tokens
            .tokens
            .get(org)
            .and_then(|org_tokens| org_tokens.get(forge))
            .cloned();

        Ok(state)
    }

    /// Validate the current token status
    /// Returns the current status without making any changes
    pub async fn validate_token(&self, org: &str, forge: &Forge) -> Result<TokenStatus> {
        self.get_token(org, forge).await
    }

    /// Mark a token as expired with an error message
    pub async fn mark_expired(&self, org: &str, forge: &Forge, error_msg: &str) -> Result<()> {
        let mut tokens = self.storage.load().await?;

        let org_tokens = tokens.tokens.entry(org.to_string()).or_default();
        org_tokens.insert(forge.clone(), TokenState::expired(error_msg.to_string()));

        self.storage.save(&tokens).await
    }

    /// Mark a token as valid
    pub async fn mark_valid(&self, org: &str, forge: &Forge) -> Result<()> {
        let mut tokens = self.storage.load().await?;

        let org_tokens = tokens.tokens.entry(org.to_string()).or_default();
        org_tokens.insert(forge.clone(), TokenState::valid());

        self.storage.save(&tokens).await
    }

    /// Mark a token as missing
    pub async fn mark_missing(&self, org: &str, forge: &Forge) -> Result<()> {
        let mut tokens = self.storage.load().await?;

        let org_tokens = tokens.tokens.entry(org.to_string()).or_default();
        org_tokens.insert(forge.clone(), TokenState::missing());

        self.storage.save(&tokens).await
    }

    /// Refresh token status - triggers re-validation
    /// This updates the last_checked timestamp and sets status to valid
    /// (Actual token validation against forge APIs would be done by caller)
    pub async fn refresh_token(&self, org: &str, forge: &Forge) -> Result<()> {
        self.mark_valid(org, forge).await
    }

    /// Get all token states for an organization
    pub async fn get_org_tokens(&self, org: &str) -> Result<OrgTokenStates> {
        let tokens = self.storage.load().await?;

        Ok(tokens.tokens.get(org).cloned().unwrap_or_default())
    }

    /// Get all token states
    pub async fn get_all_tokens(&self) -> Result<TokensFile> {
        self.storage.load().await
    }

    /// Clear token state for an org/forge combination
    pub async fn clear_token(&self, org: &str, forge: &Forge) -> Result<()> {
        let mut tokens = self.storage.load().await?;

        if let Some(org_tokens) = tokens.tokens.get_mut(org) {
            org_tokens.remove(forge);
            // Clean up empty org entries
            if org_tokens.is_empty() {
                tokens.tokens.remove(org);
            }
        }

        self.storage.save(&tokens).await
    }

    /// Clear all token states for an organization
    pub async fn clear_org_tokens(&self, org: &str) -> Result<()> {
        let mut tokens = self.storage.load().await?;
        tokens.tokens.remove(org);
        self.storage.save(&tokens).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create test paths using a temp directory
    fn test_paths(temp_dir: &TempDir) -> HyperforgePaths {
        HyperforgePaths {
            config_dir: temp_dir.path().to_path_buf(),
        }
    }

    #[test]
    fn test_token_status_serialization() {
        // Test Valid status
        let valid = TokenState::valid();
        let yaml = serde_yaml::to_string(&valid).unwrap();
        assert!(yaml.contains("status: valid"));

        // Test Expired status
        let expired = TokenState::expired("401 Unauthorized".to_string());
        let yaml = serde_yaml::to_string(&expired).unwrap();
        assert!(yaml.contains("status: expired"));
        assert!(yaml.contains("401 Unauthorized"));

        // Test Missing status
        let missing = TokenState::missing();
        let yaml = serde_yaml::to_string(&missing).unwrap();
        assert!(yaml.contains("status: missing"));
    }

    #[test]
    fn test_token_state_methods() {
        let valid = TokenState::valid();
        assert!(valid.is_valid());
        assert!(!valid.is_expired());
        assert!(!valid.is_missing());
        assert!(valid.error().is_none());

        let expired = TokenState::expired("test error".to_string());
        assert!(!expired.is_valid());
        assert!(expired.is_expired());
        assert!(!expired.is_missing());
        assert_eq!(expired.error(), Some("test error"));

        let missing = TokenState::missing();
        assert!(!missing.is_valid());
        assert!(!missing.is_expired());
        assert!(missing.is_missing());
        assert!(missing.error().is_none());
    }

    #[test]
    fn test_tokens_file_structure() {
        let yaml = r#"
tokens:
  hypermemetic:
    github:
      status: valid
      last_checked: "2026-01-09T10:00:00Z"
    codeberg:
      status: expired
      error: "401 Unauthorized"
      last_checked: "2026-01-09T09:00:00Z"
"#;

        let tokens: TokensFile = serde_yaml::from_str(yaml).unwrap();
        assert!(tokens.tokens.contains_key("hypermemetic"));

        let org_tokens = tokens.tokens.get("hypermemetic").unwrap();
        assert!(org_tokens.get(&Forge::GitHub).unwrap().is_valid());
        assert!(org_tokens.get(&Forge::Codeberg).unwrap().is_expired());
    }

    // =========================================================================
    // TokenStorage async tests
    // =========================================================================

    #[tokio::test]
    async fn test_storage_load_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let storage = TokenStorage::new(paths);

        // Loading from nonexistent file should return empty state
        let tokens = storage.load().await.unwrap();
        assert!(tokens.tokens.is_empty());
    }

    #[tokio::test]
    async fn test_storage_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let storage = TokenStorage::new(paths);

        // Create some token state
        let mut tokens = TokensFile::default();
        let mut org_tokens = OrgTokenStates::new();
        org_tokens.insert(Forge::GitHub, TokenState::valid());
        org_tokens.insert(Forge::Codeberg, TokenState::expired("test error".to_string()));
        tokens.tokens.insert("test-org".to_string(), org_tokens);

        // Save it
        storage.save(&tokens).await.unwrap();

        // Verify file exists
        assert!(storage.tokens_file().exists());

        // Load it back
        let loaded = storage.load().await.unwrap();
        assert!(loaded.tokens.contains_key("test-org"));

        let loaded_org = loaded.tokens.get("test-org").unwrap();
        assert!(loaded_org.get(&Forge::GitHub).unwrap().is_valid());
        assert!(loaded_org.get(&Forge::Codeberg).unwrap().is_expired());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_storage_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let storage = TokenStorage::new(paths);

        let tokens = TokensFile::default();
        storage.save(&tokens).await.unwrap();

        // Verify file has 0600 permissions
        let metadata = std::fs::metadata(storage.tokens_file()).unwrap();
        let mode = metadata.permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "Expected 0600 permissions, got {:o}", mode & 0o777);
    }

    #[tokio::test]
    async fn test_storage_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let storage = TokenStorage::new(paths);

        // Initial save
        let mut tokens = TokensFile::default();
        tokens.tokens.insert("org1".to_string(), OrgTokenStates::new());
        storage.save(&tokens).await.unwrap();

        // Second save - atomic rename should work
        tokens.tokens.insert("org2".to_string(), OrgTokenStates::new());
        storage.save(&tokens).await.unwrap();

        // Verify both orgs exist
        let loaded = storage.load().await.unwrap();
        assert!(loaded.tokens.contains_key("org1"));
        assert!(loaded.tokens.contains_key("org2"));

        // Verify no temp file left behind
        let temp_file = storage.tokens_file().with_extension("yaml.tmp");
        assert!(!temp_file.exists(), "Temp file should be cleaned up");
    }

    // =========================================================================
    // TokenManager async tests
    // =========================================================================

    #[tokio::test]
    async fn test_manager_mark_expired() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Mark as expired
        manager.mark_expired("my-org", &Forge::GitHub, "401 Unauthorized").await.unwrap();

        // Verify
        let status = manager.get_token("my-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Expired { .. }));

        if let TokenStatus::Expired { error } = status {
            assert_eq!(error, "401 Unauthorized");
        }
    }

    #[tokio::test]
    async fn test_manager_mark_valid() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // First mark expired
        manager.mark_expired("my-org", &Forge::GitHub, "some error").await.unwrap();

        // Then mark valid
        manager.mark_valid("my-org", &Forge::GitHub).await.unwrap();

        // Verify
        let status = manager.get_token("my-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Valid));
    }

    #[tokio::test]
    async fn test_manager_get_missing_token() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Get token that was never set
        let status = manager.get_token("unknown-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Missing));
    }

    #[tokio::test]
    async fn test_manager_clear_token() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Set then clear
        manager.mark_valid("my-org", &Forge::GitHub).await.unwrap();
        manager.clear_token("my-org", &Forge::GitHub).await.unwrap();

        // Verify it's gone
        let state = manager.get_token_state("my-org", &Forge::GitHub).await.unwrap();
        assert!(state.is_none());
    }

    #[tokio::test]
    async fn test_manager_clear_org_tokens() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Set multiple tokens for org
        manager.mark_valid("my-org", &Forge::GitHub).await.unwrap();
        manager.mark_valid("my-org", &Forge::Codeberg).await.unwrap();

        // Clear all
        manager.clear_org_tokens("my-org").await.unwrap();

        // Verify all gone
        let org_tokens = manager.get_org_tokens("my-org").await.unwrap();
        assert!(org_tokens.is_empty());
    }

    #[tokio::test]
    async fn test_manager_get_org_tokens() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Set multiple tokens
        manager.mark_valid("my-org", &Forge::GitHub).await.unwrap();
        manager.mark_expired("my-org", &Forge::Codeberg, "error").await.unwrap();

        // Get all for org
        let org_tokens = manager.get_org_tokens("my-org").await.unwrap();
        assert_eq!(org_tokens.len(), 2);
        assert!(org_tokens.get(&Forge::GitHub).unwrap().is_valid());
        assert!(org_tokens.get(&Forge::Codeberg).unwrap().is_expired());
    }

    #[tokio::test]
    async fn test_manager_multiple_orgs() {
        let temp_dir = TempDir::new().unwrap();
        let paths = test_paths(&temp_dir);
        let manager = TokenManager::new(paths);

        // Set tokens for multiple orgs
        manager.mark_valid("org1", &Forge::GitHub).await.unwrap();
        manager.mark_valid("org2", &Forge::Codeberg).await.unwrap();

        // Verify isolation
        let org1_tokens = manager.get_org_tokens("org1").await.unwrap();
        let org2_tokens = manager.get_org_tokens("org2").await.unwrap();

        assert!(org1_tokens.contains_key(&Forge::GitHub));
        assert!(!org1_tokens.contains_key(&Forge::Codeberg));
        assert!(!org2_tokens.contains_key(&Forge::GitHub));
        assert!(org2_tokens.contains_key(&Forge::Codeberg));
    }
}
