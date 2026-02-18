//! LocalForge - In-memory forge state mirror with YAML persistence
//!
//! LocalForge implements ForgePort and serves as the local source of truth
//! for repository configurations. Internally it stores `RepoRecord` for
//! rich lifecycle tracking, while the ForgePort trait boundary converts
//! to/from `Repo` for compatibility with remote forge adapters.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::forge_port::{ForgeError, ForgePort, ForgeResult};
use crate::types::{Forge, OwnerType, Repo};
use crate::types::repo::RepoRecord;

/// Sync state tracked per remote forge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeSyncState {
    pub last_synced: DateTime<Utc>,
    pub etag: Option<String>,
}

/// In-memory forge state mirror with YAML persistence
#[derive(Clone)]
pub struct LocalForge {
    /// Organization name
    org: String,
    /// Repository state (repo_name -> RepoRecord)
    repos: Arc<RwLock<HashMap<String, RepoRecord>>>,
    /// Per-forge sync state
    forges: Arc<RwLock<HashMap<Forge, ForgeSyncState>>>,
    /// Path to repos.yaml file
    config_path: Option<PathBuf>,
    /// Whether the org is a user account or an organization
    owner_type: Arc<RwLock<Option<OwnerType>>>,
}

impl LocalForge {
    /// Create a new LocalForge for the given organization
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            repos: Arc::new(RwLock::new(HashMap::new())),
            forges: Arc::new(RwLock::new(HashMap::new())),
            config_path: None,
            owner_type: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a LocalForge with a config path for persistence
    pub fn with_config_path(org: impl Into<String>, path: PathBuf) -> Self {
        Self {
            org: org.into(),
            repos: Arc::new(RwLock::new(HashMap::new())),
            forges: Arc::new(RwLock::new(HashMap::new())),
            config_path: Some(path),
            owner_type: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the organization name
    pub fn org(&self) -> &str {
        &self.org
    }

    /// Get the owner type (user vs org)
    pub fn owner_type(&self) -> Option<OwnerType> {
        self.owner_type.read().ok().and_then(|ot| ot.clone())
    }

    /// Set the owner type
    pub fn set_owner_type(&self, ot: OwnerType) {
        if let Ok(mut guard) = self.owner_type.write() {
            *guard = Some(ot);
        }
    }

    /// Add a repository to local state (converts Repo to RepoRecord internally)
    pub fn add_repo(&self, repo: Repo) -> ForgeResult<()> {
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        if repos.contains_key(&repo.name) {
            return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
        }

        let record = RepoRecord::from_repo(&repo);
        repos.insert(repo.name.clone(), record);
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

    /// Get all repositories as a Vec of Repo (for backward compat)
    pub fn all_repos(&self) -> ForgeResult<Vec<Repo>> {
        let repos = self.repos.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;

        Ok(repos.values().map(|r| r.to_repo()).collect())
    }

    // --- RepoRecord API ---

    /// Get forge sync states
    pub fn forge_states(&self) -> ForgeResult<HashMap<Forge, ForgeSyncState>> {
        let states = self.forges.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        Ok(states.clone())
    }

    /// Update forge sync state
    pub fn set_forge_state(&self, forge: Forge, state: ForgeSyncState) -> ForgeResult<()> {
        let mut states = self.forges.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        states.insert(forge, state);
        Ok(())
    }

    /// Get a repo record by name
    pub fn get_record(&self, name: &str) -> ForgeResult<RepoRecord> {
        let repos = self.repos.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        repos.get(name)
            .cloned()
            .ok_or_else(|| ForgeError::RepoNotFound { name: name.to_string() })
    }

    /// Update a repo record
    pub fn update_record(&self, record: &RepoRecord) -> ForgeResult<()> {
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        repos.insert(record.name.clone(), record.clone());
        Ok(())
    }

    /// Add or merge a repo record
    pub fn upsert_record(&self, record: RepoRecord) -> ForgeResult<()> {
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        if let Some(existing) = repos.get_mut(&record.name) {
            // Merge present_on sets
            for forge in &record.present_on {
                existing.present_on.insert(forge.clone());
            }
            // Update description/visibility if the incoming record has them
            if record.description.is_some() {
                existing.description = record.description.clone();
            }
            existing.visibility = record.visibility.clone();
        } else {
            repos.insert(record.name.clone(), record);
        }
        Ok(())
    }

    /// Get all repo records
    pub fn all_records(&self) -> ForgeResult<Vec<RepoRecord>> {
        let repos = self.repos.read().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        Ok(repos.values().cloned().collect())
    }

    /// Load repositories from YAML file (supports migration from old format)
    pub async fn load_from_yaml(&self) -> ForgeResult<()> {
        let path = self.config_path.as_ref()
            .ok_or_else(|| ForgeError::ApiError("No config path set".to_string()))?;

        if !path.exists() {
            // If file doesn't exist, start with empty state
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path).await?;

        // Try new format first
        if let Ok(config) = serde_yaml::from_str::<ReposYaml>(&content) {
            let mut repos = self.repos.write().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;
            repos.clear();
            for (name, mut record) in config.repos {
                record.name = name.clone();
                repos.insert(name, record);
            }
            // Load forge states
            let mut states = self.forges.write().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;
            for (forge_str, state) in config.forge_states {
                if let Some(forge) = crate::config::HyperforgeConfig::parse_forge(&forge_str) {
                    states.insert(forge, state);
                }
            }
            // Load owner type
            if let Some(ot) = config.owner_type {
                if let Ok(mut guard) = self.owner_type.write() {
                    *guard = Some(ot);
                }
            }
            return Ok(());
        }

        // Try old format (migration)
        if let Ok(old_config) = serde_yaml::from_str::<OldReposYaml>(&content) {
            let mut repos = self.repos.write().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;
            repos.clear();
            for (name, repo) in old_config.repos {
                let mut r = repo;
                r.name = name.clone();
                let record = RepoRecord::from_repo(&r);
                repos.insert(name, record);
            }
            return Ok(());
        }

        Err(ForgeError::SerdeError("Failed to parse repos.yaml in either old or new format".to_string()))
    }

    /// Save repositories to YAML file (new format with RepoRecord)
    pub async fn save_to_yaml(&self) -> ForgeResult<()> {
        let path = self.config_path.as_ref()
            .ok_or_else(|| ForgeError::ApiError("No config path set".to_string()))?;

        // Clone data while holding lock, then release before async operations
        let (yaml_repos, forge_states, owner_type) = {
            let repos = self.repos.read().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;
            let states = self.forges.read().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;
            let ot = self.owner_type.read().map_err(|e| {
                ForgeError::ApiError(format!("Lock poisoned: {}", e))
            })?;

            let repo_map: HashMap<String, RepoRecord> = repos.iter()
                .map(|(name, record)| (name.clone(), record.clone()))
                .collect();
            let state_map: HashMap<String, ForgeSyncState> = states.iter()
                .map(|(forge, state)| (format!("{:?}", forge).to_lowercase(), state.clone()))
                .collect();
            (repo_map, state_map, ot.clone())
        }; // Locks are dropped here

        let config = ReposYaml {
            repos: yaml_repos,
            forge_states,
            owner_type,
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
            .map(|r| r.to_repo())
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

        // Update the existing record from the incoming Repo
        let record = RepoRecord::from_repo(repo);
        repos.insert(repo.name.clone(), record);
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

    async fn set_default_branch(&self, _org: &str, name: &str, branch: &str) -> ForgeResult<()> {
        // LocalForge now tracks default branch in RepoRecord
        let mut repos = self.repos.write().map_err(|e| {
            ForgeError::ApiError(format!("Lock poisoned: {}", e))
        })?;
        if let Some(record) = repos.get_mut(name) {
            record.default_branch = branch.to_string();
        }
        Ok(())
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

        // Get the existing record
        let mut record = repos.remove(old_name).ok_or_else(|| {
            ForgeError::RepoNotFound { name: old_name.to_string() }
        })?;

        // Check new name doesn't already exist
        if repos.contains_key(new_name) {
            // Put the old one back
            repos.insert(old_name.to_string(), record);
            return Err(ForgeError::RepoAlreadyExists { name: new_name.to_string() });
        }

        // Track the previous name
        record.previous_names.push(old_name.to_string());
        // Update the name and insert with new key
        record.name = new_name.to_string();
        repos.insert(new_name.to_string(), record);

        Ok(())
    }
}

/// YAML file format for repos.yaml (new format with RepoRecord)
#[derive(Debug, Serialize, Deserialize)]
struct ReposYaml {
    #[serde(default)]
    repos: HashMap<String, RepoRecord>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    forge_states: HashMap<String, ForgeSyncState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_type: Option<OwnerType>,
}

/// Old YAML format for migration from pre-state-mirror versions
#[derive(Debug, Deserialize)]
struct OldReposYaml {
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
        // After RepoRecord conversion, origin/mirror distinction is based on set iteration order
        // Just verify both forges are present
        let all_forges = loaded_repo2.all_forges();
        assert!(all_forges.contains(&Forge::Codeberg));
        assert!(all_forges.contains(&Forge::GitHub));
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
        // Verify all forges are present (origin/mirror assignment may vary with HashSet)
        let all_forges = loaded.all_forges();
        assert!(all_forges.contains(&Forge::GitHub));
        assert!(all_forges.contains(&Forge::Codeberg));
        assert!(all_forges.contains(&Forge::GitLab));
        assert_eq!(all_forges.len(), 3);
    }

    #[tokio::test]
    async fn test_yaml_migration_from_old_format() {
        let tmp = TempDir::new().unwrap();
        let yaml_path = tmp.path().join("repos.yaml");

        // Write old format
        let old_yaml = r#"repos:
  my-repo:
    name: my-repo
    description: A test repo
    visibility: public
    origin: github
    mirrors:
      - codeberg
    protected: false
"#;
        tokio::fs::write(&yaml_path, old_yaml).await.unwrap();

        // Load with new code
        let forge = LocalForge::with_config_path("testorg", yaml_path.clone());
        forge.load_from_yaml().await.unwrap();

        // Verify migration
        let repos = forge.list_repos("testorg").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");

        // Verify RepoRecord fields
        let record = forge.get_record("my-repo").unwrap();
        assert!(record.present_on.contains(&Forge::GitHub));
        assert!(record.present_on.contains(&Forge::Codeberg));
        assert_eq!(record.default_branch, "main");
        assert!(!record.managed);
        assert!(!record.dismissed);
    }

    #[tokio::test]
    async fn test_repo_record_api() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub)
            .with_description("Test")
            .with_mirror(Forge::Codeberg);

        forge.add_repo(repo).unwrap();

        // Test get_record
        let record = forge.get_record("test-repo").unwrap();
        assert_eq!(record.name, "test-repo");
        assert!(record.present_on.contains(&Forge::GitHub));
        assert!(record.present_on.contains(&Forge::Codeberg));
        assert_eq!(record.default_branch, "main");

        // Test update_record
        let mut updated = record.clone();
        updated.managed = true;
        updated.default_branch = "develop".to_string();
        forge.update_record(&updated).unwrap();

        let reloaded = forge.get_record("test-repo").unwrap();
        assert!(reloaded.managed);
        assert_eq!(reloaded.default_branch, "develop");

        // Test all_records
        let all = forge.all_records().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_upsert_record_merge() {
        let forge = LocalForge::new("testorg");

        // Create initial record with GitHub only
        let repo = Repo::new("test-repo", Forge::GitHub)
            .with_description("Original");
        forge.add_repo(repo).unwrap();

        // Upsert with Codeberg - should merge
        let mut new_record = forge.get_record("test-repo").unwrap();
        new_record.present_on.clear();
        new_record.present_on.insert(Forge::Codeberg);
        new_record.description = Some("Updated".to_string());
        forge.upsert_record(new_record).unwrap();

        let merged = forge.get_record("test-repo").unwrap();
        assert!(merged.present_on.contains(&Forge::GitHub)); // preserved
        assert!(merged.present_on.contains(&Forge::Codeberg)); // added
        assert_eq!(merged.description, Some("Updated".to_string()));
    }

    #[tokio::test]
    async fn test_upsert_record_insert() {
        let forge = LocalForge::new("testorg");

        // Upsert a new record (no existing)
        let mut present_on = std::collections::HashSet::new();
        present_on.insert(Forge::GitLab);
        let record = RepoRecord {
            name: "new-repo".to_string(),
            description: Some("Brand new".to_string()),
            visibility: Visibility::Private,
            default_branch: "main".to_string(),
            present_on,
            managed: true,
            dismissed: false,
            deleted_from: Vec::new(),
            deleted_at: None,
            previous_names: Vec::new(),
        };
        forge.upsert_record(record).unwrap();

        let retrieved = forge.get_record("new-repo").unwrap();
        assert_eq!(retrieved.description, Some("Brand new".to_string()));
        assert!(retrieved.managed);
        assert!(retrieved.present_on.contains(&Forge::GitLab));
    }

    #[tokio::test]
    async fn test_forge_sync_state() {
        let forge = LocalForge::new("testorg");

        // Initially empty
        let states = forge.forge_states().unwrap();
        assert!(states.is_empty());

        // Set a state
        let now = Utc::now();
        forge.set_forge_state(Forge::GitHub, ForgeSyncState {
            last_synced: now,
            etag: Some("abc123".to_string()),
        }).unwrap();

        let states = forge.forge_states().unwrap();
        assert_eq!(states.len(), 1);
        assert!(states.contains_key(&Forge::GitHub));
        assert_eq!(states[&Forge::GitHub].etag, Some("abc123".to_string()));
    }

    #[tokio::test]
    async fn test_rename_tracks_previous_names() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("old-name", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        forge.rename_repo("testorg", "old-name", "new-name").await.unwrap();

        let record = forge.get_record("new-name").unwrap();
        assert_eq!(record.name, "new-name");
        assert_eq!(record.previous_names, vec!["old-name".to_string()]);
    }

    #[tokio::test]
    async fn test_set_default_branch_updates_record() {
        let forge = LocalForge::new("testorg");

        let repo = Repo::new("test-repo", Forge::GitHub);
        forge.create_repo("testorg", &repo).await.unwrap();

        forge.set_default_branch("testorg", "test-repo", "develop").await.unwrap();

        let record = forge.get_record("test-repo").unwrap();
        assert_eq!(record.default_branch, "develop");
    }
}
