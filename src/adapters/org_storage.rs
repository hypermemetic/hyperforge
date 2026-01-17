//! Adapter that wraps OrgStorage to implement StoragePort.
//!
//! This bridges the existing OrgStorage implementation with the new
//! hexagonal architecture's StoragePort interface.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use crate::domain::{DesiredRepo, ForgeRepoState, ObservedRepo, RepoIdentity};
use crate::ports::{StorageError, StoragePort};
use crate::storage::OrgStorage;
use crate::types::Visibility;

/// Adapter that wraps OrgStorage to implement StoragePort.
///
/// Note: This adapter only works for a single org (the one OrgStorage was
/// created for). The `org` parameter in StoragePort methods must match.
pub struct OrgStorageAdapter {
    storage: OrgStorage,
}

impl OrgStorageAdapter {
    /// Create a new adapter wrapping the given OrgStorage.
    pub fn new(storage: OrgStorage) -> Self {
        Self { storage }
    }

    /// Get the org name this adapter is bound to.
    fn org_name(&self) -> &str {
        // OrgStorage doesn't expose org_name directly, so we store it
        // This is a limitation - for now we'll extract it from errors
        // In practice, callers should ensure they use the correct org
        "unknown"
    }

    /// Load synced state from separate synced.yaml file (fallback for legacy storage)
    async fn load_synced_file(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError> {
        let synced_path = self.storage.paths().org_dir(org).join("synced.yaml");

        if !synced_path.exists() {
            return Ok(Vec::new());
        }

        let contents = tokio::fs::read_to_string(&synced_path).await.map_err(|e| {
            StorageError::read_error(e.to_string(), Some(synced_path.clone()))
        })?;

        // Parse the synced.yaml format
        #[derive(serde::Deserialize)]
        struct SyncedReposFile {
            owner: String,
            repos: std::collections::HashMap<String, SyncedRepoConfig>,
        }

        #[derive(serde::Deserialize)]
        struct SyncedRepoConfig {
            #[serde(default)]
            forge_states: Vec<ForgeStateConfig>,
        }

        #[derive(serde::Deserialize)]
        struct ForgeStateConfig {
            forge: crate::types::Forge,
            exists: bool,
            url: Option<String>,
            forge_id: Option<String>,
            visibility: Option<Visibility>,
            description: Option<String>,
        }

        let file: SyncedReposFile = serde_yaml::from_str(&contents).map_err(|e| {
            StorageError::parse_error(e.to_string(), Some(synced_path))
        })?;

        let repos = file.repos.into_iter()
            .map(|(name, config)| {
                let identity = RepoIdentity::new(&file.owner, &name);
                let forge_states = config.forge_states.into_iter()
                    .filter(|s| s.exists)
                    .map(|s| ForgeRepoState {
                        forge: s.forge,
                        exists: s.exists,
                        url: s.url,
                        forge_id: s.forge_id,
                        visibility: s.visibility,
                        description: s.description,
                    })
                    .collect();
                ObservedRepo { identity, forge_states }
            })
            .collect();

        Ok(repos)
    }
}

#[async_trait]
impl StoragePort for OrgStorageAdapter {
    async fn load_desired(&self, org: &str) -> Result<Vec<DesiredRepo>, StorageError> {
        let repos_config = self.storage.load_repos().await.map_err(|e| {
            StorageError::read_error(e.to_string(), None)
        })?;

        // Merge staged repos into view
        let staged_config = self.storage.load_staged().await.unwrap_or_default();

        let mut repos = Vec::new();

        let committed_names: HashSet<_> = repos_config.repos.keys().cloned().collect();

        for (name, config) in repos_config.repos {
            // Skip if marked for deletion in staged
            if staged_config.repos.get(&name).map(|c| c.delete).unwrap_or(false) {
                continue;
            }

            let identity = RepoIdentity::new(org, &name);
            let forges: HashSet<_> = config.forges
                .unwrap_or_default()
                .into_iter()
                .collect();

            repos.push(DesiredRepo {
                identity,
                description: config.description,
                visibility: config.visibility.unwrap_or(Visibility::Public),
                forges,
                protected: config.protected,
                marked_for_deletion: config.delete,
            });
        }

        // Add staged repos that aren't in committed
        for (name, config) in staged_config.repos {
            if config.delete {
                continue;
            }
            if committed_names.contains(&name) {
                continue; // Already handled
            }

            let identity = RepoIdentity::new(org, &name);
            let forges: HashSet<_> = config.forges
                .unwrap_or_default()
                .into_iter()
                .collect();

            repos.push(DesiredRepo {
                identity,
                description: config.description,
                visibility: config.visibility.unwrap_or(Visibility::Public),
                forges,
                protected: config.protected,
                marked_for_deletion: config.delete,
            });
        }

        Ok(repos)
    }

    async fn save_desired(&self, _org: &str, _repos: &[DesiredRepo]) -> Result<(), StorageError> {
        // For now, this adapter is read-only for desired state
        // Writes go through OrgStorage.stage_repo() directly
        Err(StorageError::write_error(
            "OrgStorageAdapter.save_desired not implemented - use OrgStorage.stage_repo() directly",
            None,
        ))
    }

    async fn load_synced(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError> {
        let mut repos_by_name: HashMap<String, ObservedRepo> = HashMap::new();

        // 1. Load from synced.yaml first (baseline)
        if let Ok(legacy_repos) = self.load_synced_file(org).await {
            for repo in legacy_repos {
                repos_by_name.insert(repo.identity.name.clone(), repo);
            }
        }

        // 2. Overlay _synced from repos.yaml (takes precedence per-forge)
        let repos_config = self.storage.load_repos().await.map_err(|e| {
            StorageError::read_error(e.to_string(), None)
        })?;

        for (name, config) in &repos_config.repos {
            if let Some(synced) = &config.synced {
                // Get or create the observed repo entry
                let identity = RepoIdentity::new(org, name);
                let observed = repos_by_name
                    .entry(name.clone())
                    .or_insert_with(|| ObservedRepo::new(identity));

                // Merge forge states (repos.yaml _synced takes precedence per-forge)
                for (forge, state) in &synced.forges {
                    // Remove any existing state for this forge, then add the new one
                    observed.forge_states.retain(|s| &s.forge != forge);
                    observed.forge_states.push(ForgeRepoState::found(
                        forge.clone(),
                        state.url.clone(),
                        config.visibility.clone().unwrap_or(Visibility::Public),
                        state.id.clone(),
                        config.description.clone(),
                    ));
                }
            }
        }

        Ok(repos_by_name.into_values().collect())
    }

    async fn save_synced(&self, _org: &str, _repos: &[ObservedRepo]) -> Result<(), StorageError> {
        // Writes go through OrgStorage.update_synced() directly
        Err(StorageError::write_error(
            "OrgStorageAdapter.save_synced not implemented - use OrgStorage.update_synced() directly",
            None,
        ))
    }

    async fn list_orgs(&self) -> Result<Vec<String>, StorageError> {
        // This adapter only knows about one org
        // Return it if we had access to the name
        Ok(vec![])
    }

    async fn org_exists(&self, _org: &str) -> Result<bool, StorageError> {
        // We assume the org exists if we have an OrgStorage for it
        Ok(true)
    }
}

impl Default for crate::types::ReposConfig {
    fn default() -> Self {
        Self {
            owner: String::new(),
            repos: std::collections::HashMap::new(),
        }
    }
}

// =============================================================================
// Testing Notes
// =============================================================================
//
// OrgStorageAdapter is a bridge adapter that wraps the existing OrgStorage
// implementation. Testing this adapter effectively requires:
//
// 1. Filesystem setup (OrgStorage reads/writes YAML files)
// 2. Creating test fixtures in the expected directory structure
//
// The meaningful tests for this adapter are:
// - Integration tests that verify the full roundtrip through OrgStorage
// - Tests in the storage module itself for OrgStorage behavior
//
// The type conversion logic (RepoConfig -> DesiredRepo, etc.) is straightforward
// field mapping. Since the adapter is largely read-only (save_desired and
// save_synced return errors), and the conversion is mechanical, unit tests
// here would mostly duplicate what's tested in:
// - yaml_storage.rs (for the YamlStorageAdapter which covers similar conversions)
// - storage/org_storage.rs (for the underlying OrgStorage behavior)
//
// If more complex conversion logic is added to this adapter in the future,
// add unit tests for that specific logic here.
