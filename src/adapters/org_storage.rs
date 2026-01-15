//! Adapter that wraps OrgStorage to implement StoragePort.
//!
//! This bridges the existing OrgStorage implementation with the new
//! hexagonal architecture's StoragePort interface.

use async_trait::async_trait;
use std::collections::HashSet;

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
        let repos_config = self.storage.load_repos().await.map_err(|e| {
            StorageError::read_error(e.to_string(), None)
        })?;

        let mut repos = Vec::new();

        for (name, config) in repos_config.repos {
            if let Some(synced) = config.synced {
                let identity = RepoIdentity::new(org, &name);
                let mut observed = ObservedRepo::new(identity);

                for (forge, state) in synced.forges {
                    observed = observed.with_forge_state(ForgeRepoState::found(
                        forge,
                        state.url,
                        config.visibility.clone().unwrap_or(Visibility::Public),
                        state.id,
                        config.description.clone(),
                    ));
                }

                repos.push(observed);
            }
        }

        Ok(repos)
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
