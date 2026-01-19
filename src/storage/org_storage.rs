use std::collections::{HashMap, HashSet};
use crate::types::{Forge, RepoConfig, ReposConfig, SyncedState, ForgeSyncedState, DiscoveredState, ForgeDiscoveredState};
use crate::error::Result;
use super::HyperforgePaths;

/// Storage operations for a specific organization's repos
#[derive(Clone)]
pub struct OrgStorage {
    paths: HyperforgePaths,
    org_name: String,
}

impl OrgStorage {
    /// Create a new OrgStorage for the given organization
    pub fn new(paths: HyperforgePaths, org_name: String) -> Self {
        Self { paths, org_name }
    }

    /// Get the paths configuration
    pub fn paths(&self) -> &HyperforgePaths {
        &self.paths
    }

    /// Load committed repos from repos.yaml
    pub async fn load_repos(&self) -> Result<ReposConfig> {
        let path = self.paths.repos_file(&self.org_name);

        if !path.exists() {
            return Ok(ReposConfig {
                owner: self.org_name.clone(),
                default_forges: None,
                repos: HashMap::new(),
            });
        }

        let contents = tokio::fs::read_to_string(&path).await?;
        let config: ReposConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Load staged repos from staged-repos.yaml
    /// Handles legacy files that may be missing the `owner` field
    pub async fn load_staged(&self) -> Result<ReposConfig> {
        let path = self.paths.staged_repos_file(&self.org_name);

        if !path.exists() {
            return Ok(ReposConfig {
                owner: self.org_name.clone(),
                default_forges: None,
                repos: HashMap::new(),
            });
        }

        let contents = tokio::fs::read_to_string(&path).await?;

        // Try parsing as full ReposConfig first, fall back to repos-only for legacy files
        match serde_yaml::from_str::<ReposConfig>(&contents) {
            Ok(config) => Ok(config),
            Err(_) => {
                // Legacy format: just repos without owner
                #[derive(serde::Deserialize)]
                struct LegacyStagedRepos {
                    repos: HashMap<String, RepoConfig>,
                }
                let legacy: LegacyStagedRepos = serde_yaml::from_str(&contents)?;
                Ok(ReposConfig {
                    owner: self.org_name.clone(),
                    default_forges: None,
                    repos: legacy.repos,
                })
            }
        }
    }

    /// Save staged repos to staged-repos.yaml
    pub async fn save_staged(&self, config: &ReposConfig) -> Result<()> {
        let path = self.paths.staged_repos_file(&self.org_name);

        // Ensure org directory exists
        let org_dir = self.paths.org_dir(&self.org_name);
        tokio::fs::create_dir_all(&org_dir).await?;

        let contents = serde_yaml::to_string(config)?;
        tokio::fs::write(&path, contents).await?;
        Ok(())
    }

    /// Save committed repos to repos.yaml
    pub async fn save_repos(&self, config: &ReposConfig) -> Result<()> {
        let path = self.paths.repos_file(&self.org_name);

        // Ensure org directory exists
        let org_dir = self.paths.org_dir(&self.org_name);
        tokio::fs::create_dir_all(&org_dir).await?;

        let contents = serde_yaml::to_string(config)?;
        tokio::fs::write(&path, contents).await?;
        Ok(())
    }

    /// Merge staged repos into committed repos (for sync operations)
    /// Returns the merged repos config
    ///
    /// Note: Repos marked for deletion are kept with `delete: true` so the sync
    /// can delete them from forges. Call `remove_deleted_repos()` after successful sync.
    pub async fn merge_staged(&self) -> Result<ReposConfig> {
        let mut repos = self.load_repos().await?;
        let staged = self.load_staged().await?;

        for (name, config) in staged.repos {
            if config.delete {
                // Mark for deletion but keep in config so sync can delete from forges
                if let Some(existing) = repos.repos.get_mut(&name) {
                    existing.delete = true;
                }
                // If repo doesn't exist in config, nothing to delete
            } else {
                repos.repos.insert(name, config);
            }
        }

        // Clear staged file
        let staged_path = self.paths.staged_repos_file(&self.org_name);
        if staged_path.exists() {
            tokio::fs::remove_file(&staged_path).await?;
        }

        // Save merged repos (with delete markers still present)
        self.save_repos(&repos).await?;

        Ok(repos)
    }

    /// Remove repos that were successfully deleted from forges
    /// Call this after sync successfully deletes the repos
    pub async fn remove_deleted_repos(&self) -> Result<()> {
        let mut repos = self.load_repos().await?;
        repos.repos.retain(|_, config| !config.delete);
        self.save_repos(&repos).await
    }

    /// Stage a repo for creation/update
    pub async fn stage_repo(&self, name: String, config: RepoConfig) -> Result<()> {
        let mut staged = self.load_staged().await?;
        staged.repos.insert(name, config);
        self.save_staged(&staged).await
    }

    /// Mark a repo for deletion in staged
    pub async fn stage_deletion(&self, name: String) -> Result<()> {
        let mut staged = self.load_staged().await?;
        staged.repos.insert(name, RepoConfig {
            description: None,
            visibility: None,
            forges: None,
            protected: false,
            delete: true,
            synced: None,
            discovered: None,
        });
        self.save_staged(&staged).await
    }

    /// Update _synced state for a repo
    pub async fn update_synced(
        &self,
        repo_name: &str,
        forge: Forge,
        url: String,
        id: Option<String>,
    ) -> Result<()> {
        let mut repos = self.load_repos().await?;

        if let Some(config) = repos.repos.get_mut(repo_name) {
            let synced = config.synced.get_or_insert_with(SyncedState::default);
            synced.forges.insert(forge, ForgeSyncedState {
                url,
                id,
                synced_at: chrono::Utc::now(),
            });
        }

        self.save_repos(&repos).await
    }

    /// Update _discovered state for a repo
    pub async fn update_discovered(
        &self,
        repo_name: &str,
        forge: Forge,
        exists: bool,
        url: Option<String>,
        id: Option<String>,
    ) -> Result<()> {
        let mut repos = self.load_repos().await?;

        if let Some(config) = repos.repos.get_mut(repo_name) {
            let discovered = config.discovered.get_or_insert_with(DiscoveredState::default);
            discovered.forges.insert(forge, ForgeDiscoveredState {
                exists,
                url,
                id,
            });
            discovered.last_refresh = Some(chrono::Utc::now());
        }

        self.save_repos(&repos).await
    }

    /// Check if repo needs sync (desired != synced)
    pub fn needs_sync(&self, config: &RepoConfig) -> bool {
        let desired_forges = config.forges.as_ref()
            .map(|f| f.iter().cloned().collect::<HashSet<_>>())
            .unwrap_or_default();

        let synced_forges = config.synced.as_ref()
            .map(|s| s.forges.keys().cloned().collect::<HashSet<_>>())
            .unwrap_or_default();

        desired_forges != synced_forges
    }
}
