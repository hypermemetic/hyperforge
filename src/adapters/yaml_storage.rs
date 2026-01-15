//! YAML file storage adapter for hyperforge.
//!
//! This adapter implements the StoragePort trait using YAML files on the
//! filesystem. It follows the existing config structure:
//!
//! ```text
//! ~/.config/hyperforge/
//!   orgs/
//!     <org>/
//!       repos.yaml       # Desired state
//!       synced.yaml      # Last synced state (observed)
//! ```

use async_trait::async_trait;
use std::path::PathBuf;

use crate::domain::{DesiredRepo, ObservedRepo, RepoIdentity, ForgeRepoState};
use crate::ports::{StoragePort, StorageError};
use crate::types::{Forge, Visibility};

/// YAML file storage adapter.
///
/// Stores desired and synced repository state in YAML files under a
/// configurable base path.
pub struct YamlStorageAdapter {
    /// Base path for config storage (e.g., ~/.config/hyperforge)
    base_path: PathBuf,
}

impl YamlStorageAdapter {
    /// Create a new YAML storage adapter.
    ///
    /// # Arguments
    ///
    /// * `base_path` - Base directory for config files
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Create a YAML storage adapter using the default path (~/.config/hyperforge)
    pub fn default_path() -> Self {
        let home = std::env::var("HOME").expect("HOME environment variable not set");
        Self::new(PathBuf::from(home).join(".config").join("hyperforge"))
    }

    /// Get the orgs directory path
    fn orgs_dir(&self) -> PathBuf {
        self.base_path.join("orgs")
    }

    /// Get the org directory path
    fn org_dir(&self, org: &str) -> PathBuf {
        self.orgs_dir().join(org)
    }

    /// Get the path to the desired state file for an org
    fn desired_path(&self, org: &str) -> PathBuf {
        self.org_dir(org).join("repos.yaml")
    }

    /// Get the path to the synced state file for an org
    fn synced_path(&self, org: &str) -> PathBuf {
        self.org_dir(org).join("synced.yaml")
    }

    /// Ensure org directory exists
    async fn ensure_org_dir(&self, org: &str) -> Result<(), StorageError> {
        let dir = self.org_dir(org);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            StorageError::write_error(e.to_string(), Some(dir))
        })
    }
}

/// YAML format for desired repos file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DesiredReposFile {
    owner: String,
    repos: std::collections::HashMap<String, DesiredRepoConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DesiredRepoConfig {
    description: Option<String>,
    visibility: Visibility,
    forges: Vec<Forge>,
    #[serde(default)]
    protected: bool,
    #[serde(default, rename = "_delete")]
    delete: bool,
}

/// YAML format for synced state file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SyncedReposFile {
    owner: String,
    repos: std::collections::HashMap<String, SyncedRepoConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SyncedRepoConfig {
    #[serde(default)]
    forge_states: Vec<ForgeStateConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ForgeStateConfig {
    forge: Forge,
    exists: bool,
    url: Option<String>,
    forge_id: Option<String>,
    visibility: Option<Visibility>,
    description: Option<String>,
}

impl From<&ForgeRepoState> for ForgeStateConfig {
    fn from(state: &ForgeRepoState) -> Self {
        ForgeStateConfig {
            forge: state.forge.clone(),
            exists: state.exists,
            url: state.url.clone(),
            forge_id: state.forge_id.clone(),
            visibility: state.visibility.clone(),
            description: state.description.clone(),
        }
    }
}

impl From<ForgeStateConfig> for ForgeRepoState {
    fn from(config: ForgeStateConfig) -> Self {
        ForgeRepoState {
            forge: config.forge,
            exists: config.exists,
            url: config.url,
            forge_id: config.forge_id,
            visibility: config.visibility,
            description: config.description,
        }
    }
}

#[async_trait]
impl StoragePort for YamlStorageAdapter {
    async fn load_desired(&self, org: &str) -> Result<Vec<DesiredRepo>, StorageError> {
        let path = self.desired_path(org);

        if !path.exists() {
            return Err(StorageError::OrgNotFound(org.to_string()));
        }

        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StorageError::read_error(e.to_string(), Some(path.clone())))?;

        let file: DesiredReposFile = serde_yaml::from_str(&contents)
            .map_err(|e| StorageError::parse_error(e.to_string(), Some(path.clone())))?;

        let repos = file
            .repos
            .into_iter()
            .map(|(name, config)| {
                let identity = RepoIdentity::new(&file.owner, &name);
                DesiredRepo {
                    identity,
                    description: config.description,
                    visibility: config.visibility,
                    forges: config.forges.into_iter().collect(),
                    protected: config.protected,
                    marked_for_deletion: config.delete,
                }
            })
            .collect();

        Ok(repos)
    }

    async fn save_desired(&self, org: &str, repos: &[DesiredRepo]) -> Result<(), StorageError> {
        self.ensure_org_dir(org).await?;
        let path = self.desired_path(org);

        let mut repos_map = std::collections::HashMap::new();
        for repo in repos {
            let config = DesiredRepoConfig {
                description: repo.description.clone(),
                visibility: repo.visibility.clone(),
                forges: repo.forges.iter().cloned().collect(),
                protected: repo.protected,
                delete: repo.marked_for_deletion,
            };
            repos_map.insert(repo.identity.name.clone(), config);
        }

        let file = DesiredReposFile {
            owner: org.to_string(),
            repos: repos_map,
        };

        let contents = serde_yaml::to_string(&file)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;

        tokio::fs::write(&path, contents)
            .await
            .map_err(|e| StorageError::write_error(e.to_string(), Some(path)))?;

        Ok(())
    }

    async fn load_synced(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError> {
        let path = self.synced_path(org);

        // If synced file doesn't exist, return empty vec (nothing synced yet)
        if !path.exists() {
            // But check if org exists at all
            let org_dir = self.org_dir(org);
            if !org_dir.exists() {
                return Err(StorageError::OrgNotFound(org.to_string()));
            }
            return Ok(Vec::new());
        }

        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StorageError::read_error(e.to_string(), Some(path.clone())))?;

        let file: SyncedReposFile = serde_yaml::from_str(&contents)
            .map_err(|e| StorageError::parse_error(e.to_string(), Some(path.clone())))?;

        let repos = file
            .repos
            .into_iter()
            .map(|(name, config)| {
                let identity = RepoIdentity::new(&file.owner, &name);
                let forge_states = config
                    .forge_states
                    .into_iter()
                    .map(ForgeRepoState::from)
                    .collect();
                ObservedRepo {
                    identity,
                    forge_states,
                }
            })
            .collect();

        Ok(repos)
    }

    async fn save_synced(&self, org: &str, repos: &[ObservedRepo]) -> Result<(), StorageError> {
        self.ensure_org_dir(org).await?;
        let path = self.synced_path(org);

        let mut repos_map = std::collections::HashMap::new();
        for repo in repos {
            let config = SyncedRepoConfig {
                forge_states: repo.forge_states.iter().map(ForgeStateConfig::from).collect(),
            };
            repos_map.insert(repo.identity.name.clone(), config);
        }

        let file = SyncedReposFile {
            owner: org.to_string(),
            repos: repos_map,
        };

        let contents = serde_yaml::to_string(&file)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;

        tokio::fs::write(&path, contents)
            .await
            .map_err(|e| StorageError::write_error(e.to_string(), Some(path)))?;

        Ok(())
    }

    async fn list_orgs(&self) -> Result<Vec<String>, StorageError> {
        let orgs_dir = self.orgs_dir();

        if !orgs_dir.exists() {
            return Ok(Vec::new());
        }

        let mut orgs = Vec::new();
        let mut entries = tokio::fs::read_dir(&orgs_dir)
            .await
            .map_err(|e| StorageError::read_error(e.to_string(), Some(orgs_dir.clone())))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            StorageError::read_error(e.to_string(), Some(orgs_dir.clone()))
        })? {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    orgs.push(name.to_string());
                }
            }
        }

        orgs.sort();
        Ok(orgs)
    }

    async fn org_exists(&self, org: &str) -> Result<bool, StorageError> {
        let org_dir = self.org_dir(org);
        Ok(org_dir.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    async fn setup() -> (TempDir, YamlStorageAdapter) {
        let tmp = TempDir::new().unwrap();
        let adapter = YamlStorageAdapter::new(tmp.path());
        (tmp, adapter)
    }

    fn test_desired_repo(org: &str, name: &str) -> DesiredRepo {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);

        DesiredRepo {
            identity: RepoIdentity::new(org, name),
            description: Some("Test repository".to_string()),
            visibility: Visibility::Public,
            forges,
            protected: false,
            marked_for_deletion: false,
        }
    }

    fn test_observed_repo(org: &str, name: &str) -> ObservedRepo {
        ObservedRepo::new(RepoIdentity::new(org, name))
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                format!("https://github.com/{}/{}", org, name),
                Visibility::Public,
                Some("123".to_string()),
                Some("Test repo".to_string()),
            ))
    }

    #[tokio::test]
    async fn test_save_and_load_desired() {
        let (_tmp, adapter) = setup().await;

        let repos = vec![
            test_desired_repo("testorg", "repo1"),
            test_desired_repo("testorg", "repo2"),
        ];

        adapter.save_desired("testorg", &repos).await.unwrap();

        let loaded = adapter.load_desired("testorg").await.unwrap();
        assert_eq!(loaded.len(), 2);

        let names: HashSet<_> = loaded.iter().map(|r| r.identity.name.as_str()).collect();
        assert!(names.contains("repo1"));
        assert!(names.contains("repo2"));
    }

    #[tokio::test]
    async fn test_save_and_load_synced() {
        let (_tmp, adapter) = setup().await;

        // First create the org directory
        adapter.save_desired("testorg", &[test_desired_repo("testorg", "repo1")]).await.unwrap();

        let repos = vec![test_observed_repo("testorg", "repo1")];

        adapter.save_synced("testorg", &repos).await.unwrap();

        let loaded = adapter.load_synced("testorg").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].identity.name, "repo1");
        assert!(loaded[0].exists_on(&Forge::GitHub));
    }

    #[tokio::test]
    async fn test_load_desired_not_found() {
        let (_tmp, adapter) = setup().await;

        let result = adapter.load_desired("nonexistent").await;
        assert!(matches!(result, Err(StorageError::OrgNotFound(_))));
    }

    #[tokio::test]
    async fn test_load_synced_empty_when_no_synced_file() {
        let (_tmp, adapter) = setup().await;

        // Create org with desired but no synced
        adapter.save_desired("testorg", &[test_desired_repo("testorg", "repo1")]).await.unwrap();

        let synced = adapter.load_synced("testorg").await.unwrap();
        assert!(synced.is_empty());
    }

    #[tokio::test]
    async fn test_list_orgs() {
        let (_tmp, adapter) = setup().await;

        // Create some orgs
        adapter.save_desired("org1", &[test_desired_repo("org1", "repo1")]).await.unwrap();
        adapter.save_desired("org2", &[test_desired_repo("org2", "repo1")]).await.unwrap();

        let orgs = adapter.list_orgs().await.unwrap();
        assert_eq!(orgs.len(), 2);
        assert!(orgs.contains(&"org1".to_string()));
        assert!(orgs.contains(&"org2".to_string()));
    }

    #[tokio::test]
    async fn test_org_exists() {
        let (_tmp, adapter) = setup().await;

        assert!(!adapter.org_exists("testorg").await.unwrap());

        adapter.save_desired("testorg", &[test_desired_repo("testorg", "repo1")]).await.unwrap();

        assert!(adapter.org_exists("testorg").await.unwrap());
    }

    #[tokio::test]
    async fn test_yaml_roundtrip_preserves_data() {
        let (_tmp, adapter) = setup().await;

        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);
        forges.insert(Forge::Codeberg);

        let original = DesiredRepo {
            identity: RepoIdentity::new("myorg", "myrepo"),
            description: Some("A cool project".to_string()),
            visibility: Visibility::Private,
            forges,
            protected: true,
            marked_for_deletion: false,
        };

        adapter.save_desired("myorg", &[original.clone()]).await.unwrap();
        let loaded = adapter.load_desired("myorg").await.unwrap();

        assert_eq!(loaded.len(), 1);
        let loaded_repo = &loaded[0];
        assert_eq!(loaded_repo.identity, original.identity);
        assert_eq!(loaded_repo.description, original.description);
        assert_eq!(loaded_repo.visibility, original.visibility);
        assert_eq!(loaded_repo.forges, original.forges);
        assert_eq!(loaded_repo.protected, original.protected);
        assert_eq!(loaded_repo.marked_for_deletion, original.marked_for_deletion);
    }

    #[tokio::test]
    async fn test_malformed_yaml_returns_parse_error() {
        let tmp = TempDir::new().unwrap();
        let adapter = YamlStorageAdapter::new(tmp.path());

        // Create org directory and write malformed YAML
        let org_dir = tmp.path().join("orgs").join("testorg");
        tokio::fs::create_dir_all(&org_dir).await.unwrap();

        let repos_path = org_dir.join("repos.yaml");
        tokio::fs::write(&repos_path, "this is: [not valid: yaml: {{").await.unwrap();

        let result = adapter.load_desired("testorg").await;
        assert!(matches!(result, Err(StorageError::ParseError { .. })));
    }

    #[tokio::test]
    async fn test_yaml_schema_wrong_structure_returns_parse_error() {
        let tmp = TempDir::new().unwrap();
        let adapter = YamlStorageAdapter::new(tmp.path());

        // Create org directory and write valid YAML but wrong schema
        let org_dir = tmp.path().join("orgs").join("testorg");
        tokio::fs::create_dir_all(&org_dir).await.unwrap();

        let repos_path = org_dir.join("repos.yaml");
        // Missing required 'owner' field
        tokio::fs::write(&repos_path, "repos:\n  my-repo:\n    visibility: public").await.unwrap();

        let result = adapter.load_desired("testorg").await;
        assert!(matches!(result, Err(StorageError::ParseError { .. })));
    }

    #[tokio::test]
    async fn test_yaml_format_is_parseable_by_other_tools() {
        let tmp = TempDir::new().unwrap();
        let adapter = YamlStorageAdapter::new(tmp.path());

        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);

        let repo = DesiredRepo {
            identity: RepoIdentity::new("myorg", "my-repo"),
            description: Some("Test repo".to_string()),
            visibility: Visibility::Public,
            forges,
            protected: false,
            marked_for_deletion: false,
        };

        adapter.save_desired("myorg", &[repo]).await.unwrap();

        // Read the raw YAML and verify it has expected structure
        let repos_path = tmp.path().join("orgs").join("myorg").join("repos.yaml");
        let contents = tokio::fs::read_to_string(&repos_path).await.unwrap();

        // Parse with serde_yaml directly to verify structure
        let parsed: serde_yaml::Value = serde_yaml::from_str(&contents).unwrap();

        // Verify expected top-level keys
        let obj = parsed.as_mapping().expect("should be a mapping");
        assert!(obj.contains_key(&serde_yaml::Value::String("owner".to_string())));
        assert!(obj.contains_key(&serde_yaml::Value::String("repos".to_string())));

        // Verify owner value
        let owner = obj.get(&serde_yaml::Value::String("owner".to_string())).unwrap();
        assert_eq!(owner.as_str(), Some("myorg"));
    }

    #[tokio::test]
    async fn test_synced_roundtrip_preserves_forge_states() {
        let (_tmp, adapter) = setup().await;

        // First create the org
        adapter.save_desired("testorg", &[test_desired_repo("testorg", "repo1")]).await.unwrap();

        let identity = RepoIdentity::new("testorg", "repo1");
        let original = ObservedRepo::new(identity.clone())
            .with_forge_state(ForgeRepoState::found(
                Forge::GitHub,
                "https://github.com/testorg/repo1".to_string(),
                Visibility::Public,
                Some("gh-123".to_string()),
                Some("A repo on GitHub".to_string()),
            ))
            .with_forge_state(ForgeRepoState::found(
                Forge::Codeberg,
                "https://codeberg.org/testorg/repo1".to_string(),
                Visibility::Private,
                Some("cb-456".to_string()),
                Some("A repo on Codeberg".to_string()),
            ));

        adapter.save_synced("testorg", &[original.clone()]).await.unwrap();
        let loaded = adapter.load_synced("testorg").await.unwrap();

        assert_eq!(loaded.len(), 1);
        let loaded_repo = &loaded[0];

        // Verify both forge states are preserved
        assert!(loaded_repo.exists_on(&Forge::GitHub));
        assert!(loaded_repo.exists_on(&Forge::Codeberg));

        // Verify GitHub state details
        let gh_state = loaded_repo.forge_states.iter()
            .find(|s| s.forge == Forge::GitHub)
            .expect("GitHub state should exist");
        assert_eq!(gh_state.url, Some("https://github.com/testorg/repo1".to_string()));
        assert_eq!(gh_state.forge_id, Some("gh-123".to_string()));
        assert_eq!(gh_state.visibility, Some(Visibility::Public));

        // Verify Codeberg state details
        let cb_state = loaded_repo.forge_states.iter()
            .find(|s| s.forge == Forge::Codeberg)
            .expect("Codeberg state should exist");
        assert_eq!(cb_state.url, Some("https://codeberg.org/testorg/repo1".to_string()));
        assert_eq!(cb_state.forge_id, Some("cb-456".to_string()));
        assert_eq!(cb_state.visibility, Some(Visibility::Private));
    }

    #[tokio::test]
    async fn test_delete_flag_roundtrip() {
        let (_tmp, adapter) = setup().await;

        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);

        let repo_to_delete = DesiredRepo {
            identity: RepoIdentity::new("myorg", "doomed-repo"),
            description: None,
            visibility: Visibility::Public,
            forges,
            protected: false,
            marked_for_deletion: true,  // This should be preserved
        };

        adapter.save_desired("myorg", &[repo_to_delete]).await.unwrap();
        let loaded = adapter.load_desired("myorg").await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].marked_for_deletion, "delete flag should be preserved through YAML roundtrip");
    }
}
