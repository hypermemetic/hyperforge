//! LocalForge - In-memory forge state with YAML persistence
//!
//! LocalForge implements ForgePort and serves as the local source of truth
//! for repository configurations. It can be persisted to/from repos.yaml.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::forge_port::{ForgeError, ForgePort, ForgeResult};
use crate::types::Repo;

/// In-memory forge state with YAML persistence
#[derive(Clone)]
pub struct LocalForge {
    /// Organization name
    org: String,
    /// Repository state (repo_name -> Repo)
    repos: Arc<RwLock<HashMap<String, Repo>>>,
    /// Path to repos.yaml file
    config_path: Option<PathBuf>,
}

impl LocalForge {
    /// Create a new LocalForge for the given organization
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            repos: Arc::new(RwLock::new(HashMap::new())),
            config_path: None,
        }
    }

    /// Create a LocalForge with a config path for persistence
    pub fn with_config_path(org: impl Into<String>, path: PathBuf) -> Self {
        Self {
            org: org.into(),
            repos: Arc::new(RwLock::new(HashMap::new())),
            config_path: Some(path),
        }
    }

    /// Get the organization name
    pub fn org(&self) -> &str {
        &self.org
    }

    /// Add a repository to local state
    pub fn add_repo(&self, repo: Repo) -> ForgeResult<()> {
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        if repos.contains_key(&repo.name) {
            return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
        }

        repos.insert(repo.name.clone(), repo);
        Ok(())
    }

    /// Remove a repository from local state
    pub fn remove_repo(&self, name: &str) -> ForgeResult<()> {
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        repos.remove(name)
            .ok_or_else(|| ForgeError::RepoNotFound { name: name.to_string() })?;

        Ok(())
    }

    /// Get all repositories as a Vec
    pub fn all_repos(&self) -> ForgeResult<Vec<Repo>> {
        let repos = self.repos.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        Ok(repos.values().cloned().collect())
    }

    /// Load repositories from YAML file
    pub async fn load_from_yaml(&self) -> ForgeResult<()> {
        let path = self.config_path.as_ref()
            .ok_or_else(|| ForgeError::ApiError("No config path set".to_string()))?;

        if !path.exists() {
            // If file doesn't exist, start with empty state
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path).await?;
        let config: ReposYaml = serde_yaml::from_str(&content)
            .map_err(|e| ForgeError::SerdeError(e.to_string()))?;

        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        repos.clear();
        for (name, repo) in config.repos {
            let mut r = repo;
            r.name = name;
            repos.insert(r.name.clone(), r);
        }

        Ok(())
    }

    /// Save repositories to YAML file
    pub async fn save_to_yaml(&self) -> ForgeResult<()> {
        let path = self.config_path.as_ref()
            .ok_or_else(|| ForgeError::ApiError("No config path set".to_string()))?;

        // Clone data while holding lock, then release before async operations
        let yaml_repos = {
            let repos = self.repos.read().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;

            let mut map = HashMap::new();
            for (name, repo) in repos.iter() {
                map.insert(name.clone(), repo.clone());
            }
            map
        }; // Lock is dropped here

        let config = ReposYaml {
            repos: yaml_repos,
        };

        let yaml = serde_yaml::to_string(&config)
            .map_err(|e| ForgeError::SerdeError(e.to_string()))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(path, yaml).await?;

        Ok(())
    }
}

#[async_trait]
impl ForgePort for LocalForge {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>> {
        if org != self.org {
            return Ok(Vec::new());
        }

        self.all_repos()
    }

    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo> {
        if org != self.org {
            return Err(ForgeError::RepoNotFound { name: name.to_string() });
        }

        let repos = self.repos.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        repos.get(name)
            .cloned()
            .ok_or_else(|| ForgeError::RepoNotFound { name: name.to_string() })
    }

    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        if org != self.org {
            return Err(ForgeError::ApiError(format!(
                "Organization mismatch: expected {}, got {}",
                self.org, org
            )));
        }

        self.add_repo(repo.clone())
    }

    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        if org != self.org {
            return Err(ForgeError::ApiError(format!(
                "Organization mismatch: expected {}, got {}",
                self.org, org
            )));
        }

        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        if !repos.contains_key(&repo.name) {
            return Err(ForgeError::RepoNotFound { name: repo.name.clone() });
        }

        repos.insert(repo.name.clone(), repo.clone());
        Ok(())
    }

    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()> {
        if org != self.org {
            return Err(ForgeError::ApiError(format!(
                "Organization mismatch: expected {}, got {}",
                self.org, org
            )));
        }

        self.remove_repo(name)
    }

    async fn rename_repo(&self, org: &str, old_name: &str, new_name: &str) -> ForgeResult<()> {
        if org != self.org {
            return Err(ForgeError::ApiError(format!(
                "Organization mismatch: expected {}, got {}",
                self.org, org
            )));
        }

        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        // Get the existing repo
        let mut repo = repos.remove(old_name).ok_or_else(|| {
            ForgeError::RepoNotFound { name: old_name.to_string() }
        })?;

        // Check new name doesn't already exist
        if repos.contains_key(new_name) {
            // Put the old one back
            repos.insert(old_name.to_string(), repo);
            return Err(ForgeError::RepoAlreadyExists { name: new_name.to_string() });
        }

        // Update the name and insert with new key
        repo.name = new_name.to_string();
        repos.insert(new_name.to_string(), repo);

        Ok(())
    }
}

/// YAML file format for repos.yaml
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ReposYaml {
    repos: HashMap<String, Repo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Forge, Visibility};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_forge_create_repo() {
        let forge = LocalForge::new("testorg");
        let repo = Repo::new("test-repo", Forge::GitHub)
            .with_description("Test repository");

        forge.create_repo("testorg", &repo).await.unwrap();

        let retrieved = forge.get_repo("testorg", "test-repo").await.unwrap();
        assert_eq!(retrieved.name, "test-repo");
        assert_eq!(retrieved.description, Some("Test repository".to_string()));
    }

    #[tokio::test]
    async fn test_local_forge_list_repos() {
        let forge = LocalForge::new("testorg");

        let repo1 = Repo::new("repo1", Forge::GitHub);
        let repo2 = Repo::new("repo2", Forge::Codeberg);

        forge.create_repo("testorg", &repo1).await.unwrap();
        forge.create_repo("testorg", &repo2).await.unwrap();

        let repos = forge.list_repos("testorg").await.unwrap();
        assert_eq!(repos.len(), 2);
    }

    #[tokio::test]
    async fn test_local_forge_update_repo() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        let updated = repo.with_visibility(Visibility::Private);
        forge.update_repo("testorg", &updated).await.unwrap();

        let retrieved = forge.get_repo("testorg", "test-repo").await.unwrap();
        assert_eq!(retrieved.visibility, Visibility::Private);
    }

    #[tokio::test]
    async fn test_local_forge_delete_repo() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        forge.delete_repo("testorg", "test-repo").await.unwrap();

        let result = forge.get_repo("testorg", "test-repo").await;
        assert!(matches!(result, Err(ForgeError::RepoNotFound { .. })));
    }

    #[tokio::test]
    async fn test_local_forge_duplicate_create() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        let result = forge.create_repo("testorg", &repo).await;
        assert!(matches!(result, Err(ForgeError::RepoAlreadyExists { .. })));
    }

    #[tokio::test]
    async fn test_local_forge_wrong_org() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        let result = forge.create_repo("wrongorg", &repo).await;
        assert!(result.is_err());

        let repos = forge.list_repos("wrongorg").await.unwrap();
        assert!(repos.is_empty());
    }

    #[tokio::test]
    async fn test_local_forge_repo_exists() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        assert!(forge.repo_exists("testorg", "test-repo").await.unwrap());
        assert!(!forge.repo_exists("testorg", "nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_yaml_persistence_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let yaml_path = tmp.path().join("repos.yaml");

        // Create forge with config path
        let forge = LocalForge::with_config_path("testorg", yaml_path.clone());

        // Add some repos
        let repo1 = Repo::new("repo1", Forge::GitHub)
            .with_description("First repo")
            .with_visibility(Visibility::Public);
        let repo2 = Repo::new("repo2", Forge::Codeberg)
            .with_description("Second repo")
            .with_mirror(Forge::GitHub);

        forge.create_repo("testorg", &repo1).await.unwrap();
        forge.create_repo("testorg", &repo2).await.unwrap();

        // Save to YAML
        forge.save_to_yaml().await.unwrap();

        // Verify file was created
        assert!(yaml_path.exists());

        // Create new forge and load
        let forge2 = LocalForge::with_config_path("testorg", yaml_path.clone());
        forge2.load_from_yaml().await.unwrap();

        // Verify repos were loaded
        let loaded_repos = forge2.list_repos("testorg").await.unwrap();
        assert_eq!(loaded_repos.len(), 2);

        let loaded_repo1 = forge2.get_repo("testorg", "repo1").await.unwrap();
        assert_eq!(loaded_repo1.name, "repo1");
        assert_eq!(loaded_repo1.description, Some("First repo".to_string()));
        assert_eq!(loaded_repo1.visibility, Visibility::Public);

        let loaded_repo2 = forge2.get_repo("testorg", "repo2").await.unwrap();
        assert_eq!(loaded_repo2.name, "repo2");
        assert_eq!(loaded_repo2.origin, Forge::Codeberg);
        assert_eq!(loaded_repo2.mirrors, vec![Forge::GitHub]);
    }

    #[tokio::test]
    async fn test_yaml_load_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let yaml_path = tmp.path().join("nonexistent.yaml");

        let forge = LocalForge::with_config_path("testorg", yaml_path);

        // Loading nonexistent file should succeed with empty state
        forge.load_from_yaml().await.unwrap();

        let repos = forge.list_repos("testorg").await.unwrap();
        assert!(repos.is_empty());
    }

    #[tokio::test]
    async fn test_yaml_roundtrip_preserves_data() {
        let tmp = TempDir::new().unwrap();
        let yaml_path = tmp.path().join("repos.yaml");

        let forge = LocalForge::with_config_path("testorg", yaml_path.clone());

        // Create complex repo with all fields
        let repo = Repo::new("complex-repo", Forge::GitHub)
            .with_description("Complex repository")
            .with_visibility(Visibility::Private)
            .with_mirrors(vec![Forge::Codeberg, Forge::GitLab])
            .with_protected(true);

        forge.create_repo("testorg", &repo).await.unwrap();
        forge.save_to_yaml().await.unwrap();

        // Load into new forge
        let forge2 = LocalForge::with_config_path("testorg", yaml_path);
        forge2.load_from_yaml().await.unwrap();

        let loaded = forge2.get_repo("testorg", "complex-repo").await.unwrap();
        assert_eq!(loaded.name, "complex-repo");
        assert_eq!(loaded.description, Some("Complex repository".to_string()));
        assert_eq!(loaded.visibility, Visibility::Private);
        assert_eq!(loaded.origin, Forge::GitHub);
        assert_eq!(loaded.mirrors.len(), 2);
        assert!(loaded.mirrors.contains(&Forge::Codeberg));
        assert!(loaded.mirrors.contains(&Forge::GitLab));
        assert!(loaded.protected);
    }
}
