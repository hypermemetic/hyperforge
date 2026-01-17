//! In-memory forge implementation for local repository state.
//!
//! LocalForge treats local configuration as a forge, enabling symmetric
//! sync operations between any two forges (including local).
//!
//! # Example
//!
//! ```ignore
//! use hyperforge::adapters::LocalForge;
//! use hyperforge::ports::ForgePort;
//!
//! // Import from GitHub to local
//! let github = GitHubAdapter::new(token);
//! let local = LocalForge::new();
//! sync(&github, &local, "my-org", SyncOptions::new()).await?;
//!
//! // Push from local to Codeberg
//! let codeberg = CodebergAdapter::new(token);
//! sync(&local, &codeberg, "my-org", SyncOptions::new()).await?;
//! ```
//!
//! # Persistence
//!
//! LocalForge can optionally persist state to disk in YAML format:
//!
//! ```ignore
//! // Load from existing file or create new
//! let forge = LocalForge::load_or_create(&path)?;
//!
//! // With auto-save on every mutation
//! let forge = LocalForge::with_auto_save(path, "myorg".into())?;
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use thiserror::Error;

use crate::domain::{DesiredRepo, ForgeRepoState, ObservedRepo, RepoIdentity};
use crate::ports::{ForgeError, ForgePort};
use crate::types::{Forge, ReposConfig, Visibility};

/// Internal representation of a repo in LocalForge.
#[derive(Debug, Clone)]
struct LocalRepo {
    identity: RepoIdentity,
    description: Option<String>,
    visibility: Visibility,
}

/// Configuration for LocalForge persistence.
#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    /// Path to save/load from
    pub path: PathBuf,
    /// Organization name (for YAML format compatibility)
    pub org: String,
    /// Auto-save on mutations
    pub auto_save: bool,
}

/// Errors from persistence operations.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("Failed to read {path}: {message}")]
    ReadError { path: PathBuf, message: String },

    #[error("Failed to parse {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Failed to write {path}: {message}")]
    WriteError { path: PathBuf, message: String },

    #[error("Failed to serialize: {message}")]
    SerializeError { message: String },
}

/// In-memory forge for local repository state.
///
/// Stores repos in a nested HashMap: org -> repo_name -> Repo.
/// Thread-safe via RwLock for concurrent access.
///
/// This enables symmetric sync operations where local is treated
/// as just another forge:
/// - `sync(github, local)` = import from GitHub
/// - `sync(local, github)` = push to GitHub
/// - `sync(github, codeberg)` = mirror GitHub to Codeberg
pub struct LocalForge {
    /// Nested map: org_name -> (repo_name -> repo_data)
    repos: RwLock<HashMap<String, HashMap<String, LocalRepo>>>,
    /// Optional persistence configuration
    persistence_config: RwLock<Option<PersistenceConfig>>,
}

// YAML file format (compatible with existing repos.yaml)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReposYamlFile {
    owner: String,
    #[serde(default)]
    repos: HashMap<String, RepoYamlConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoYamlConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    visibility: Visibility,
    #[serde(default)]
    forges: Vec<Forge>,
    #[serde(default)]
    protected: bool,
    #[serde(default, rename = "_delete")]
    delete: bool,
}

impl LocalForge {
    /// Create a new empty LocalForge.
    pub fn new() -> Self {
        Self {
            repos: RwLock::new(HashMap::new()),
            persistence_config: RwLock::new(None),
        }
    }

    /// Create a LocalForge pre-populated with repos.
    ///
    /// Useful for testing or loading from persisted state.
    pub fn with_repos(repos: Vec<DesiredRepo>) -> Self {
        let forge = Self::new();
        {
            let mut map = forge.repos.write().unwrap();
            for repo in repos {
                let org_repos = map.entry(repo.identity.org.clone()).or_default();
                org_repos.insert(
                    repo.identity.name.clone(),
                    LocalRepo {
                        identity: repo.identity,
                        description: repo.description,
                        visibility: repo.visibility,
                    },
                );
            }
        }
        forge
    }

    /// Create a LocalForge from a ReposConfig (including staged changes).
    ///
    /// This is the preferred method when you have already merged staged
    /// changes into the config, as it respects the `delete` flag.
    pub fn from_repos_config(config: &ReposConfig) -> Self {
        let forge = Self::new();
        {
            let mut map = forge.repos.write().unwrap();
            let org_repos = map.entry(config.owner.clone()).or_default();

            for (name, repo_config) in &config.repos {
                // Skip repos marked for deletion
                if repo_config.delete {
                    continue;
                }

                org_repos.insert(
                    name.clone(),
                    LocalRepo {
                        identity: RepoIdentity::new(&config.owner, name),
                        description: repo_config.description.clone(),
                        visibility: repo_config.visibility.unwrap_or_default(),
                    },
                );
            }
        }
        forge
    }

    // =========================================================================
    // Persistence Methods
    // =========================================================================

    /// Load LocalForge state from a YAML file.
    ///
    /// The file format is compatible with the existing repos.yaml format:
    /// ```yaml
    /// owner: myorg
    /// repos:
    ///   repo-name:
    ///     description: "..."
    ///     visibility: public
    ///     forges: [github, codeberg]
    /// ```
    pub fn load(path: &Path) -> Result<Self, PersistenceError> {
        let contents = std::fs::read_to_string(path).map_err(|e| PersistenceError::ReadError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let file: ReposYamlFile =
            serde_yaml::from_str(&contents).map_err(|e| PersistenceError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let forge = Self::new();
        {
            let mut map = forge.repos.write().unwrap();
            let org_repos = map.entry(file.owner.clone()).or_default();

            for (name, config) in file.repos {
                // Skip repos marked for deletion
                if config.delete {
                    continue;
                }
                org_repos.insert(
                    name.clone(),
                    LocalRepo {
                        identity: RepoIdentity::new(&file.owner, &name),
                        description: config.description,
                        visibility: config.visibility,
                    },
                );
            }
        }

        Ok(forge)
    }

    /// Load or create a LocalForge from a path.
    ///
    /// If the file doesn't exist, returns an empty LocalForge.
    pub fn load_or_create(path: &Path) -> Result<Self, PersistenceError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::new())
        }
    }

    /// Save LocalForge state to a YAML file.
    ///
    /// Saves in the repos.yaml compatible format.
    pub fn save(&self, path: &Path, org: &str) -> Result<(), PersistenceError> {
        let map = self.repos.read().unwrap();
        let org_repos = map.get(org).cloned().unwrap_or_default();

        let mut repos = HashMap::new();
        for (name, local) in org_repos {
            repos.insert(
                name,
                RepoYamlConfig {
                    description: local.description,
                    visibility: local.visibility,
                    forges: vec![], // LocalForge doesn't track target forges
                    protected: false,
                    delete: false,
                },
            );
        }

        let file = ReposYamlFile {
            owner: org.to_string(),
            repos,
        };

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| PersistenceError::WriteError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        }

        let contents =
            serde_yaml::to_string(&file).map_err(|e| PersistenceError::SerializeError {
                message: e.to_string(),
            })?;

        std::fs::write(path, contents).map_err(|e| PersistenceError::WriteError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        Ok(())
    }

    /// Create a LocalForge with auto-save enabled.
    ///
    /// Every mutation (create, update, delete) will automatically
    /// save to the specified path.
    pub fn with_auto_save(path: PathBuf, org: String) -> Result<Self, PersistenceError> {
        let forge = Self::load_or_create(&path)?;
        forge.persistence_config.write().unwrap().replace(PersistenceConfig {
            path,
            org,
            auto_save: true,
        });
        Ok(forge)
    }

    /// Trigger auto-save if enabled
    fn maybe_auto_save(&self) -> Result<(), PersistenceError> {
        let config = self.persistence_config.read().unwrap();
        if let Some(config) = config.as_ref() {
            if config.auto_save {
                self.save(&config.path, &config.org)?;
            }
        }
        Ok(())
    }

    // =========================================================================
    // Query Methods
    // =========================================================================

    /// Get a snapshot of all repo identities for an org.
    ///
    /// Useful for testing and debugging.
    pub fn get_org_repos(&self, org: &str) -> Vec<RepoIdentity> {
        let map = self.repos.read().unwrap();
        map.get(org)
            .map(|org_repos| org_repos.values().map(|r| r.identity.clone()).collect())
            .unwrap_or_default()
    }

    /// Get total number of repos across all orgs.
    pub fn repo_count(&self) -> usize {
        let map = self.repos.read().unwrap();
        map.values().map(|org| org.len()).sum()
    }

    /// Clear all repos (useful for testing).
    pub fn clear(&self) {
        let mut map = self.repos.write().unwrap();
        map.clear();
    }
}

impl Default for LocalForge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ForgePort for LocalForge {
    fn forge_type(&self) -> Forge {
        Forge::Local
    }

    async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
        let map = self.repos.read().unwrap();
        let repos = map
            .get(org)
            .map(|org_repos| {
                org_repos
                    .values()
                    .map(|local| {
                        ObservedRepo::new(local.identity.clone()).with_forge_state(
                            ForgeRepoState::found(
                                self.forge_type(),
                                format!("local://{}/{}", local.identity.org, local.identity.name),
                                local.visibility.clone(),
                                None,
                                local.description.clone(),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(repos)
    }

    async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        // Scope the write lock
        {
            let mut map = self.repos.write().unwrap();
            let org_repos = map.entry(repo.identity.org.clone()).or_default();

            if org_repos.contains_key(&repo.identity.name) {
                return Err(ForgeError::RepoAlreadyExists(repo.identity.clone()));
            }

            let local = LocalRepo {
                identity: repo.identity.clone(),
                description: repo.description.clone(),
                visibility: repo.visibility.clone(),
            };
            org_repos.insert(repo.identity.name.clone(), local);
        }

        // Trigger auto-save after successful mutation
        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(ObservedRepo::new(repo.identity.clone()).with_forge_state(ForgeRepoState::found(
            self.forge_type(),
            format!("local://{}/{}", repo.identity.org, repo.identity.name),
            repo.visibility.clone(),
            None,
            repo.description.clone(),
        )))
    }

    async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        // Scope the write lock
        {
            let mut map = self.repos.write().unwrap();
            let org_repos = map
                .get_mut(&repo.identity.org)
                .ok_or_else(|| ForgeError::RepoNotFound(repo.identity.clone()))?;

            let local = org_repos
                .get_mut(&repo.identity.name)
                .ok_or_else(|| ForgeError::RepoNotFound(repo.identity.clone()))?;

            local.description = repo.description.clone();
            local.visibility = repo.visibility.clone();
        }

        // Trigger auto-save after successful mutation
        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(ObservedRepo::new(repo.identity.clone()).with_forge_state(ForgeRepoState::found(
            self.forge_type(),
            format!("local://{}/{}", repo.identity.org, repo.identity.name),
            repo.visibility.clone(),
            None,
            repo.description.clone(),
        )))
    }

    async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError> {
        // Scope the write lock
        {
            let mut map = self.repos.write().unwrap();
            let org_repos = map
                .get_mut(&identity.org)
                .ok_or_else(|| ForgeError::RepoNotFound(identity.clone()))?;

            if org_repos.remove(&identity.name).is_none() {
                return Err(ForgeError::RepoNotFound(identity.clone()));
            }
        }

        // Trigger auto-save after successful mutation
        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(())
    }

    async fn repo_exists(&self, identity: &RepoIdentity) -> Result<bool, ForgeError> {
        let map = self.repos.read().unwrap();
        Ok(map
            .get(&identity.org)
            .map(|org_repos| org_repos.contains_key(&identity.name))
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn github_forges() -> HashSet<Forge> {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges
    }

    fn make_desired(org: &str, name: &str) -> DesiredRepo {
        DesiredRepo::new(RepoIdentity::new(org, name), Visibility::Public, github_forges())
    }

    fn make_private(org: &str, name: &str) -> DesiredRepo {
        DesiredRepo::new(RepoIdentity::new(org, name), Visibility::Private, github_forges())
    }

    // ==========================================================================
    // Basic CRUD operations
    // ==========================================================================

    #[tokio::test]
    async fn test_create_and_list() {
        let forge = LocalForge::new();
        let repo = make_desired("myorg", "myrepo");

        forge.create_repo(&repo).await.unwrap();

        let repos = forge.list_repos("myorg").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].identity.name, "myrepo");
    }

    #[tokio::test]
    async fn test_create_duplicate_fails() {
        let forge = LocalForge::new();
        let repo = make_desired("myorg", "myrepo");

        forge.create_repo(&repo).await.unwrap();
        let result = forge.create_repo(&repo).await;

        assert!(matches!(result, Err(ForgeError::RepoAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_update_existing_repo() {
        let forge = LocalForge::new();
        let repo = make_desired("myorg", "myrepo");
        forge.create_repo(&repo).await.unwrap();

        // Update to private
        let updated = make_private("myorg", "myrepo");
        let result = forge.update_repo(&updated).await.unwrap();

        // Verify the state reflects the update
        let state = result.get_forge_state(&Forge::Local).unwrap();
        assert_eq!(state.visibility, Some(Visibility::Private));
    }

    #[tokio::test]
    async fn test_update_nonexistent_fails() {
        let forge = LocalForge::new();
        let repo = make_desired("myorg", "myrepo");

        let result = forge.update_repo(&repo).await;
        assert!(matches!(result, Err(ForgeError::RepoNotFound(_))));
    }

    #[tokio::test]
    async fn test_delete_removes_repo() {
        let forge = LocalForge::new();
        let repo = make_desired("myorg", "myrepo");

        forge.create_repo(&repo).await.unwrap();
        assert!(forge.repo_exists(&repo.identity).await.unwrap());

        forge.delete_repo(&repo.identity).await.unwrap();
        assert!(!forge.repo_exists(&repo.identity).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_fails() {
        let forge = LocalForge::new();
        let identity = RepoIdentity::new("myorg", "myrepo");

        let result = forge.delete_repo(&identity).await;
        assert!(matches!(result, Err(ForgeError::RepoNotFound(_))));
    }

    // ==========================================================================
    // Constructor tests
    // ==========================================================================

    #[tokio::test]
    async fn test_with_repos_constructor() {
        let repos = vec![
            make_desired("org1", "repo1"),
            make_desired("org1", "repo2"),
            make_desired("org2", "repo3"),
        ];
        let forge = LocalForge::with_repos(repos);

        let org1_repos = forge.list_repos("org1").await.unwrap();
        let org2_repos = forge.list_repos("org2").await.unwrap();

        assert_eq!(org1_repos.len(), 2);
        assert_eq!(org2_repos.len(), 1);
    }

    #[tokio::test]
    async fn test_empty_org_returns_empty_vec() {
        let forge = LocalForge::new();
        let repos = forge.list_repos("nonexistent").await.unwrap();
        assert!(repos.is_empty());
    }

    // ==========================================================================
    // Utility method tests
    // ==========================================================================

    #[tokio::test]
    async fn test_repo_count() {
        let repos = vec![
            make_desired("org1", "repo1"),
            make_desired("org1", "repo2"),
            make_desired("org2", "repo3"),
        ];
        let forge = LocalForge::with_repos(repos);

        assert_eq!(forge.repo_count(), 3);
    }

    #[tokio::test]
    async fn test_get_org_repos() {
        let repos = vec![
            make_desired("myorg", "repo1"),
            make_desired("myorg", "repo2"),
        ];
        let forge = LocalForge::with_repos(repos);

        let identities = forge.get_org_repos("myorg");
        assert_eq!(identities.len(), 2);

        let names: Vec<_> = identities.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"repo1"));
        assert!(names.contains(&"repo2"));
    }

    #[tokio::test]
    async fn test_clear() {
        let repos = vec![make_desired("myorg", "repo1")];
        let forge = LocalForge::with_repos(repos);

        assert_eq!(forge.repo_count(), 1);
        forge.clear();
        assert_eq!(forge.repo_count(), 0);
    }

    // ==========================================================================
    // Forge type verification
    // ==========================================================================

    #[test]
    fn test_forge_type_is_local() {
        let forge = LocalForge::new();
        assert_eq!(forge.forge_type(), Forge::Local);
    }

    // ==========================================================================
    // ObservedRepo state verification
    // ==========================================================================

    #[tokio::test]
    async fn test_list_repos_returns_correct_forge_state() {
        let repo = make_desired("myorg", "myrepo").with_description("Test repo");
        let forge = LocalForge::with_repos(vec![repo]);

        let repos = forge.list_repos("myorg").await.unwrap();
        assert_eq!(repos.len(), 1);

        let observed = &repos[0];
        assert!(observed.exists_on(&Forge::Local));

        let state = observed.get_forge_state(&Forge::Local).unwrap();
        assert!(state.exists);
        assert_eq!(state.url, Some("local://myorg/myrepo".to_string()));
        assert_eq!(state.visibility, Some(Visibility::Public));
        assert_eq!(state.description, Some("Test repo".to_string()));
    }

    #[tokio::test]
    async fn test_create_returns_correct_forge_state() {
        let forge = LocalForge::new();
        let repo = make_private("myorg", "private-repo").with_description("Private stuff");

        let observed = forge.create_repo(&repo).await.unwrap();

        let state = observed.get_forge_state(&Forge::Local).unwrap();
        assert!(state.exists);
        assert_eq!(state.visibility, Some(Visibility::Private));
        assert_eq!(state.description, Some("Private stuff".to_string()));
    }

    // ==========================================================================
    // Persistence tests
    // ==========================================================================

    #[tokio::test]
    async fn test_save_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        // Create and save
        let repos = vec![
            make_desired("myorg", "repo1").with_description("First repo"),
            make_private("myorg", "repo2"),
        ];
        let forge = LocalForge::with_repos(repos);
        forge.save(&path, "myorg").unwrap();

        // Load into new forge
        let loaded = LocalForge::load(&path).unwrap();

        // Verify
        let org_repos = loaded.list_repos("myorg").await.unwrap();
        assert_eq!(org_repos.len(), 2);

        let names: Vec<_> = org_repos.iter().map(|r| r.identity.name.as_str()).collect();
        assert!(names.contains(&"repo1"));
        assert!(names.contains(&"repo2"));

        // Check visibility preserved
        let repo1 = org_repos.iter().find(|r| r.identity.name == "repo1").unwrap();
        let state1 = repo1.get_forge_state(&Forge::Local).unwrap();
        assert_eq!(state1.visibility, Some(Visibility::Public));

        let repo2 = org_repos.iter().find(|r| r.identity.name == "repo2").unwrap();
        let state2 = repo2.get_forge_state(&Forge::Local).unwrap();
        assert_eq!(state2.visibility, Some(Visibility::Private));
    }

    #[tokio::test]
    async fn test_load_existing_repos_yaml_format() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        // Write a file in the existing repos.yaml format
        let yaml = r#"
owner: hypermemetic
repos:
  hyperforge:
    description: "Multi-forge repo management"
    visibility: public
    forges: [github, codeberg]
    protected: false
  private-tool:
    visibility: private
    forges: [github]
"#;
        std::fs::write(&path, yaml).unwrap();

        // Load and verify
        let forge = LocalForge::load(&path).unwrap();
        let repos = forge.list_repos("hypermemetic").await.unwrap();

        assert_eq!(repos.len(), 2);

        let hyperforge = repos.iter().find(|r| r.identity.name == "hyperforge").unwrap();
        let state = hyperforge.get_forge_state(&Forge::Local).unwrap();
        assert_eq!(state.description, Some("Multi-forge repo management".to_string()));
        assert_eq!(state.visibility, Some(Visibility::Public));
    }

    #[tokio::test]
    async fn test_load_skips_deleted_repos() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        let yaml = r#"
owner: myorg
repos:
  active-repo:
    visibility: public
  deleted-repo:
    visibility: public
    _delete: true
"#;
        std::fs::write(&path, yaml).unwrap();

        let forge = LocalForge::load(&path).unwrap();
        let repos = forge.list_repos("myorg").await.unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].identity.name, "active-repo");
    }

    #[test]
    fn test_load_or_create_nonexistent_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("does-not-exist.yaml");

        let forge = LocalForge::load_or_create(&path).unwrap();
        assert_eq!(forge.repo_count(), 0);
    }

    #[tokio::test]
    async fn test_auto_save_on_create() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        // Create forge with auto-save
        let forge = LocalForge::with_auto_save(path.clone(), "myorg".to_string()).unwrap();

        // Create a repo (should auto-save)
        forge.create_repo(&make_desired("myorg", "new-repo")).await.unwrap();

        // Load from file and verify
        let loaded = LocalForge::load(&path).unwrap();
        let repos = loaded.list_repos("myorg").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].identity.name, "new-repo");
    }

    #[tokio::test]
    async fn test_auto_save_on_update() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        // Create forge with auto-save and initial repo
        let forge = LocalForge::with_auto_save(path.clone(), "myorg".to_string()).unwrap();
        forge.create_repo(&make_desired("myorg", "my-repo")).await.unwrap();

        // Update (should auto-save)
        let updated = make_private("myorg", "my-repo");
        forge.update_repo(&updated).await.unwrap();

        // Load and verify
        let loaded = LocalForge::load(&path).unwrap();
        let repos = loaded.list_repos("myorg").await.unwrap();
        let state = repos[0].get_forge_state(&Forge::Local).unwrap();
        assert_eq!(state.visibility, Some(Visibility::Private));
    }

    #[tokio::test]
    async fn test_auto_save_on_delete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("repos.yaml");

        // Create forge with auto-save and two repos
        let forge = LocalForge::with_auto_save(path.clone(), "myorg".to_string()).unwrap();
        forge.create_repo(&make_desired("myorg", "repo1")).await.unwrap();
        forge.create_repo(&make_desired("myorg", "repo2")).await.unwrap();

        // Delete one (should auto-save)
        forge.delete_repo(&RepoIdentity::new("myorg", "repo1")).await.unwrap();

        // Load and verify only repo2 remains
        let loaded = LocalForge::load(&path).unwrap();
        let repos = loaded.list_repos("myorg").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].identity.name, "repo2");
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("nested").join("dir").join("repos.yaml");

        let repos = vec![make_desired("myorg", "repo1")];
        let forge = LocalForge::with_repos(repos);

        // Should create nested/dir/ and write file
        forge.save(&path, "myorg").unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_load_nonexistent_file_errors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("does-not-exist.yaml");

        let result = LocalForge::load(&path);
        assert!(matches!(result, Err(PersistenceError::ReadError { .. })));
    }

    #[test]
    fn test_load_invalid_yaml_errors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("invalid.yaml");

        std::fs::write(&path, "this is not valid yaml: [ missing bracket").unwrap();

        let result = LocalForge::load(&path);
        assert!(matches!(result, Err(PersistenceError::ParseError { .. })));
    }
}
