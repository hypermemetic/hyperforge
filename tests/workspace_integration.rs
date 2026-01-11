//! Integration tests for workspace functionality
//!
//! These tests verify the end-to-end behavior of workspace-related operations
//! using temporary directories and mock forge clients.

use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

// Re-export types for integration tests
use hyperforge::bridge::ForgeRepo;
use hyperforge::storage::{GlobalConfig, HyperforgePaths, OrgConfig, TokenManager};
use hyperforge::types::{Forge, ForgesConfig, Visibility};

/// Test harness for workspace integration tests
///
/// Provides a temporary directory structure that mimics a real hyperforge
/// installation, with sample configuration files and workspace bindings.
struct TestHarness {
    /// Root temp directory (cleaned up on drop)
    _temp_dir: TempDir,
    /// Hyperforge paths pointing to temp directory
    paths: HyperforgePaths,
    /// A workspace directory for testing
    workspace_dir: PathBuf,
}

impl TestHarness {
    /// Create a new test harness with basic setup
    async fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_dir = temp_dir.path().join("config");
        let workspace_dir = temp_dir.path().join("workspace");

        // Create directories
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let paths = HyperforgePaths {
            config_dir: config_dir.clone(),
        };

        // Ensure org directories exist
        paths.ensure_dirs().await.unwrap();

        Self {
            _temp_dir: temp_dir,
            paths,
            workspace_dir,
        }
    }

    /// Create a test harness with a sample config
    async fn with_sample_config() -> Self {
        let harness = Self::new().await;

        // Create a sample org config
        let mut forges_map = HashMap::new();
        forges_map.insert(
            Forge::GitHub,
            hyperforge::types::ForgeConfig { sync: true },
        );
        forges_map.insert(
            Forge::Codeberg,
            hyperforge::types::ForgeConfig { sync: true },
        );

        let mut organizations = HashMap::new();
        organizations.insert(
            "test-org".to_string(),
            OrgConfig {
                owner: "test-owner".to_string(),
                ssh_key: "~/.ssh/id_test".to_string(),
                origin: Forge::GitHub,
                forges: ForgesConfig::Object(forges_map),
                default_visibility: Visibility::Public,
            },
        );

        // Add workspace binding
        let mut workspaces = HashMap::new();
        workspaces.insert(harness.workspace_dir.clone(), "test-org".to_string());

        let config = GlobalConfig {
            default_org: Some("test-org".to_string()),
            secret_provider: hyperforge::types::SecretProvider::Keychain,
            organizations,
            workspaces,
        };

        config.save(&harness.paths).await.unwrap();

        harness
    }

    /// Get the paths for this harness
    fn paths(&self) -> &HyperforgePaths {
        &self.paths
    }

    /// Get the workspace directory
    fn workspace_dir(&self) -> &PathBuf {
        &self.workspace_dir
    }

    /// Create a subdirectory in the workspace
    fn create_subdir(&self, name: &str) -> PathBuf {
        let path = self.workspace_dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

// =============================================================================
// Workspace Resolution Tests
// =============================================================================

mod workspace_resolution {
    use super::*;

    #[tokio::test]
    async fn test_resolve_workspace_exact_match() {
        let harness = TestHarness::with_sample_config().await;
        let config = GlobalConfig::load(harness.paths()).await.unwrap();

        // Exact match should resolve to the org
        let resolved = config.resolve_workspace(harness.workspace_dir());
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), "test-org");
    }

    #[tokio::test]
    async fn test_resolve_workspace_subdirectory() {
        let harness = TestHarness::with_sample_config().await;
        let config = GlobalConfig::load(harness.paths()).await.unwrap();

        // Create a subdirectory
        let subdir = harness.create_subdir("my-project");

        // Subdirectory should resolve to parent's org
        let resolved = config.resolve_workspace(&subdir);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), "test-org");
    }

    #[tokio::test]
    async fn test_resolve_workspace_nested_subdirectory() {
        let harness = TestHarness::with_sample_config().await;
        let config = GlobalConfig::load(harness.paths()).await.unwrap();

        // Create nested subdirectories
        let nested = harness.workspace_dir().join("a/b/c/d");
        std::fs::create_dir_all(&nested).unwrap();

        // Deeply nested path should still resolve
        let resolved = config.resolve_workspace(&nested);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), "test-org");
    }

    #[tokio::test]
    async fn test_resolve_workspace_unbound_path() {
        let harness = TestHarness::with_sample_config().await;
        let config = GlobalConfig::load(harness.paths()).await.unwrap();

        // Some random path not under any workspace
        let random_path = PathBuf::from("/tmp/random/path");
        std::fs::create_dir_all(&random_path).ok(); // May or may not exist

        let resolved = config.resolve_workspace(&random_path);
        // Should not resolve since it's not under a workspace
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn test_resolve_workspace_empty_config() {
        let harness = TestHarness::new().await;

        // Create empty config
        let config = GlobalConfig::default();
        config.save(harness.paths()).await.unwrap();

        let loaded = GlobalConfig::load(harness.paths()).await.unwrap();
        let resolved = loaded.resolve_workspace(harness.workspace_dir());

        // Should not resolve with no workspace bindings
        assert!(resolved.is_none());
    }
}

// =============================================================================
// Token Manager Integration Tests
// =============================================================================

mod token_manager {
    use super::*;
    use hyperforge::storage::TokenStatus;

    #[tokio::test]
    async fn test_token_lifecycle() {
        let harness = TestHarness::new().await;
        let manager = TokenManager::new(harness.paths().clone());

        // Initially missing
        let status = manager.get_token("my-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Missing));

        // Mark valid
        manager.mark_valid("my-org", &Forge::GitHub).await.unwrap();
        let status = manager.get_token("my-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Valid));

        // Mark expired
        manager
            .mark_expired("my-org", &Forge::GitHub, "Token revoked")
            .await
            .unwrap();
        let status = manager.get_token("my-org", &Forge::GitHub).await.unwrap();
        assert!(matches!(status, TokenStatus::Expired { .. }));

        // Clear
        manager.clear_token("my-org", &Forge::GitHub).await.unwrap();
        let state = manager
            .get_token_state("my-org", &Forge::GitHub)
            .await
            .unwrap();
        assert!(state.is_none());
    }

    #[tokio::test]
    async fn test_multi_org_token_isolation() {
        let harness = TestHarness::new().await;
        let manager = TokenManager::new(harness.paths().clone());

        // Set tokens for different orgs
        manager.mark_valid("org1", &Forge::GitHub).await.unwrap();
        manager
            .mark_expired("org2", &Forge::GitHub, "error")
            .await
            .unwrap();

        // Verify isolation
        let status1 = manager.get_token("org1", &Forge::GitHub).await.unwrap();
        let status2 = manager.get_token("org2", &Forge::GitHub).await.unwrap();

        assert!(matches!(status1, TokenStatus::Valid));
        assert!(matches!(status2, TokenStatus::Expired { .. }));
    }
}

// =============================================================================
// Config Persistence Tests
// =============================================================================

mod config_persistence {
    use super::*;

    #[tokio::test]
    async fn test_config_save_and_load() {
        let harness = TestHarness::new().await;

        // Create config
        let mut organizations = HashMap::new();
        organizations.insert(
            "my-org".to_string(),
            OrgConfig {
                owner: "my-owner".to_string(),
                ssh_key: "~/.ssh/id_myorg".to_string(),
                origin: Forge::Codeberg,
                forges: ForgesConfig::from_forges(vec![Forge::GitHub, Forge::Codeberg]),
                default_visibility: Visibility::Private,
            },
        );

        let config = GlobalConfig {
            default_org: Some("my-org".to_string()),
            secret_provider: hyperforge::types::SecretProvider::Env,
            organizations,
            workspaces: HashMap::new(),
        };

        // Save
        config.save(harness.paths()).await.unwrap();

        // Load
        let loaded = GlobalConfig::load(harness.paths()).await.unwrap();

        assert_eq!(loaded.default_org, Some("my-org".to_string()));
        assert!(loaded.organizations.contains_key("my-org"));

        let org = loaded.organizations.get("my-org").unwrap();
        assert_eq!(org.owner, "my-owner");
        assert!(matches!(org.origin, Forge::Codeberg));
    }

    #[tokio::test]
    async fn test_config_with_workspace_bindings() {
        let harness = TestHarness::new().await;

        let workspace_path = harness.workspace_dir().clone();

        let mut workspaces = HashMap::new();
        workspaces.insert(workspace_path.clone(), "bound-org".to_string());

        let config = GlobalConfig {
            default_org: None,
            secret_provider: hyperforge::types::SecretProvider::default(),
            organizations: HashMap::new(),
            workspaces,
        };

        config.save(harness.paths()).await.unwrap();
        let loaded = GlobalConfig::load(harness.paths()).await.unwrap();

        assert!(loaded.workspaces.contains_key(&workspace_path));
        assert_eq!(loaded.workspaces.get(&workspace_path).unwrap(), "bound-org");
    }
}

// =============================================================================
// Smoke Tests
// =============================================================================

mod smoke_tests {
    use super::*;

    /// Smoke test: Full workspace setup and resolution flow
    #[tokio::test]
    async fn test_full_workspace_setup_flow() {
        // 1. Set up a test environment
        let harness = TestHarness::with_sample_config().await;

        // 2. Load config
        let config = GlobalConfig::load(harness.paths()).await.unwrap();

        // 3. Verify org exists
        assert!(config.organizations.contains_key("test-org"));
        let org = config.get_org("test-org").unwrap();
        assert_eq!(org.owner, "test-owner");

        // 4. Verify workspace binding
        let resolved = config.resolve_workspace(harness.workspace_dir());
        assert_eq!(resolved, Some("test-org".to_string()));

        // 5. Create a project subdirectory
        let project = harness.create_subdir("my-project");

        // 6. Verify project resolves to same org
        let project_org = config.resolve_workspace(&project);
        assert_eq!(project_org, Some("test-org".to_string()));
    }

    /// Smoke test: Token management across multiple forges
    #[tokio::test]
    async fn test_multi_forge_token_management() {
        let harness = TestHarness::new().await;
        let manager = TokenManager::new(harness.paths().clone());

        // Set up tokens for multiple forges
        let forges = vec![Forge::GitHub, Forge::Codeberg];

        for forge in &forges {
            manager.mark_valid("test-org", forge).await.unwrap();
        }

        // Verify all are valid
        for forge in &forges {
            let status = manager.get_token("test-org", forge).await.unwrap();
            assert!(
                matches!(status, hyperforge::storage::TokenStatus::Valid),
                "Token for {:?} should be valid",
                forge
            );
        }

        // Expire one
        manager
            .mark_expired("test-org", &Forge::GitHub, "Rate limited")
            .await
            .unwrap();

        // Verify states
        let github_status = manager.get_token("test-org", &Forge::GitHub).await.unwrap();
        let codeberg_status = manager
            .get_token("test-org", &Forge::Codeberg)
            .await
            .unwrap();

        assert!(matches!(
            github_status,
            hyperforge::storage::TokenStatus::Expired { .. }
        ));
        assert!(matches!(
            codeberg_status,
            hyperforge::storage::TokenStatus::Valid
        ));
    }
}

// =============================================================================
// Test Utilities
// =============================================================================

/// Helper module for creating test data
pub mod test_utils {
    use super::*;

    /// Create a sample ForgeRepo for testing
    pub fn sample_repo(name: &str) -> ForgeRepo {
        ForgeRepo {
            name: name.to_string(),
            full_name: format!("test-owner/{}", name),
            description: Some(format!("Test repo: {}", name)),
            visibility: Visibility::Public,
            clone_url: format!("https://github.com/test-owner/{}.git", name),
            ssh_url: format!("git@github.com:test-owner/{}.git", name),
        }
    }

    /// Create a sample OrgConfig for testing
    pub fn sample_org_config() -> OrgConfig {
        OrgConfig {
            owner: "test-owner".to_string(),
            ssh_key: "~/.ssh/id_test".to_string(),
            origin: Forge::GitHub,
            forges: ForgesConfig::from_forges(vec![Forge::GitHub]),
            default_visibility: Visibility::Public,
        }
    }
}
