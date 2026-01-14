//! In-memory storage adapter for hyperforge.
//!
//! This adapter implements the StoragePort trait using an in-memory HashMap.
//! It's primarily useful for testing, allowing tests to run without touching
//! the filesystem.
//!
//! # Thread Safety
//!
//! Uses `tokio::sync::RwLock` for interior mutability, allowing concurrent
//! reads while serializing writes.
//!
//! # Example
//!
//! ```rust
//! use hyperforge::adapters::InMemoryStorageAdapter;
//! use hyperforge::ports::StoragePort;
//!
//! #[tokio::main]
//! async fn main() {
//!     let storage = InMemoryStorageAdapter::new();
//!
//!     // Use the storage adapter...
//!     let orgs = storage.list_orgs().await.unwrap();
//!     assert!(orgs.is_empty());
//! }
//! ```

use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::domain::{DesiredRepo, ObservedRepo};
use crate::ports::{StoragePort, StorageError};

/// In-memory storage state for a single organization
#[derive(Debug, Clone, Default)]
struct OrgState {
    desired: Vec<DesiredRepo>,
    synced: Vec<ObservedRepo>,
}

/// In-memory storage adapter for testing.
///
/// Stores all data in memory using a HashMap keyed by organization name.
/// Thread-safe via RwLock.
pub struct InMemoryStorageAdapter {
    state: RwLock<HashMap<String, OrgState>>,
}

impl InMemoryStorageAdapter {
    /// Create a new empty in-memory storage adapter
    pub fn new() -> Self {
        Self {
            state: RwLock::new(HashMap::new()),
        }
    }

    /// Create a storage adapter with initial desired state
    pub fn with_desired(orgs: HashMap<String, Vec<DesiredRepo>>) -> Self {
        let state: HashMap<String, OrgState> = orgs
            .into_iter()
            .map(|(org, desired)| {
                (org, OrgState {
                    desired,
                    synced: Vec::new(),
                })
            })
            .collect();
        Self {
            state: RwLock::new(state),
        }
    }

    /// Get a snapshot of all desired repos for an org (for test assertions)
    pub async fn get_desired_snapshot(&self, org: &str) -> Option<Vec<DesiredRepo>> {
        let state = self.state.read().await;
        state.get(org).map(|s| s.desired.clone())
    }

    /// Get a snapshot of all synced repos for an org (for test assertions)
    pub async fn get_synced_snapshot(&self, org: &str) -> Option<Vec<ObservedRepo>> {
        let state = self.state.read().await;
        state.get(org).map(|s| s.synced.clone())
    }

    /// Clear all data (useful for test cleanup)
    pub async fn clear(&self) {
        let mut state = self.state.write().await;
        state.clear();
    }

    /// Get the total number of orgs stored
    pub async fn org_count(&self) -> usize {
        let state = self.state.read().await;
        state.len()
    }
}

impl Default for InMemoryStorageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InMemoryStorageAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryStorageAdapter")
            .field("state", &"<RwLock<...>>")
            .finish()
    }
}

#[async_trait]
impl StoragePort for InMemoryStorageAdapter {
    async fn load_desired(&self, org: &str) -> Result<Vec<DesiredRepo>, StorageError> {
        let state = self.state.read().await;
        match state.get(org) {
            Some(org_state) => Ok(org_state.desired.clone()),
            None => Err(StorageError::OrgNotFound(org.to_string())),
        }
    }

    async fn save_desired(&self, org: &str, repos: &[DesiredRepo]) -> Result<(), StorageError> {
        let mut state = self.state.write().await;
        let org_state = state.entry(org.to_string()).or_default();
        org_state.desired = repos.to_vec();
        Ok(())
    }

    async fn load_synced(&self, org: &str) -> Result<Vec<ObservedRepo>, StorageError> {
        let state = self.state.read().await;
        match state.get(org) {
            Some(org_state) => Ok(org_state.synced.clone()),
            None => Err(StorageError::OrgNotFound(org.to_string())),
        }
    }

    async fn save_synced(&self, org: &str, repos: &[ObservedRepo]) -> Result<(), StorageError> {
        let mut state = self.state.write().await;
        let org_state = state.entry(org.to_string()).or_default();
        org_state.synced = repos.to_vec();
        Ok(())
    }

    async fn list_orgs(&self) -> Result<Vec<String>, StorageError> {
        let state = self.state.read().await;
        let mut orgs: Vec<String> = state.keys().cloned().collect();
        orgs.sort();
        Ok(orgs)
    }

    async fn org_exists(&self, org: &str) -> Result<bool, StorageError> {
        let state = self.state.read().await;
        Ok(state.contains_key(org))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RepoIdentity, ForgeRepoState};
    use crate::types::{Forge, Visibility};
    use std::collections::HashSet;

    fn test_desired_repo(org: &str, name: &str) -> DesiredRepo {
        let mut forges = HashSet::new();
        forges.insert(Forge::GitHub);

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
    async fn test_new_storage_is_empty() {
        let storage = InMemoryStorageAdapter::new();
        let orgs = storage.list_orgs().await.unwrap();
        assert!(orgs.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_load_desired() {
        let storage = InMemoryStorageAdapter::new();

        let repos = vec![
            test_desired_repo("testorg", "repo1"),
            test_desired_repo("testorg", "repo2"),
        ];

        storage.save_desired("testorg", &repos).await.unwrap();

        let loaded = storage.load_desired("testorg").await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn test_save_and_load_synced() {
        let storage = InMemoryStorageAdapter::new();

        // First create org by saving desired
        storage.save_desired("testorg", &[test_desired_repo("testorg", "repo1")]).await.unwrap();

        let repos = vec![test_observed_repo("testorg", "repo1")];
        storage.save_synced("testorg", &repos).await.unwrap();

        let loaded = storage.load_synced("testorg").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].identity.name, "repo1");
    }

    #[tokio::test]
    async fn test_load_desired_not_found() {
        let storage = InMemoryStorageAdapter::new();
        let result = storage.load_desired("nonexistent").await;
        assert!(matches!(result, Err(StorageError::OrgNotFound(_))));
    }

    #[tokio::test]
    async fn test_list_orgs() {
        let storage = InMemoryStorageAdapter::new();

        storage.save_desired("org1", &[test_desired_repo("org1", "repo")]).await.unwrap();
        storage.save_desired("org2", &[test_desired_repo("org2", "repo")]).await.unwrap();

        let orgs = storage.list_orgs().await.unwrap();
        assert_eq!(orgs.len(), 2);
        assert!(orgs.contains(&"org1".to_string()));
        assert!(orgs.contains(&"org2".to_string()));
    }

    #[tokio::test]
    async fn test_org_exists() {
        let storage = InMemoryStorageAdapter::new();

        assert!(!storage.org_exists("testorg").await.unwrap());

        storage.save_desired("testorg", &[test_desired_repo("testorg", "repo")]).await.unwrap();

        assert!(storage.org_exists("testorg").await.unwrap());
    }

    #[tokio::test]
    async fn test_isolation_between_orgs() {
        let storage = InMemoryStorageAdapter::new();

        storage.save_desired("org1", &[test_desired_repo("org1", "repo1")]).await.unwrap();
        storage.save_desired("org2", &[test_desired_repo("org2", "repo2")]).await.unwrap();

        let org1_repos = storage.load_desired("org1").await.unwrap();
        let org2_repos = storage.load_desired("org2").await.unwrap();

        assert_eq!(org1_repos.len(), 1);
        assert_eq!(org1_repos[0].identity.name, "repo1");

        assert_eq!(org2_repos.len(), 1);
        assert_eq!(org2_repos[0].identity.name, "repo2");
    }

    #[tokio::test]
    async fn test_clear() {
        let storage = InMemoryStorageAdapter::new();

        storage.save_desired("org1", &[test_desired_repo("org1", "repo")]).await.unwrap();
        storage.save_desired("org2", &[test_desired_repo("org2", "repo")]).await.unwrap();

        assert_eq!(storage.org_count().await, 2);

        storage.clear().await;

        assert_eq!(storage.org_count().await, 0);
    }

    #[tokio::test]
    async fn test_with_desired_constructor() {
        let mut initial = HashMap::new();
        initial.insert("org1".to_string(), vec![test_desired_repo("org1", "repo1")]);
        initial.insert("org2".to_string(), vec![test_desired_repo("org2", "repo2")]);

        let storage = InMemoryStorageAdapter::with_desired(initial);

        assert_eq!(storage.org_count().await, 2);

        let repos = storage.load_desired("org1").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].identity.name, "repo1");
    }

    #[tokio::test]
    async fn test_get_snapshot_helpers() {
        let storage = InMemoryStorageAdapter::new();

        let desired = vec![test_desired_repo("testorg", "repo")];
        storage.save_desired("testorg", &desired).await.unwrap();

        let synced = vec![test_observed_repo("testorg", "repo")];
        storage.save_synced("testorg", &synced).await.unwrap();

        let desired_snapshot = storage.get_desired_snapshot("testorg").await.unwrap();
        assert_eq!(desired_snapshot.len(), 1);

        let synced_snapshot = storage.get_synced_snapshot("testorg").await.unwrap();
        assert_eq!(synced_snapshot.len(), 1);

        assert!(storage.get_desired_snapshot("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let storage = std::sync::Arc::new(InMemoryStorageAdapter::new());

        // Spawn multiple writers
        let mut handles = vec![];
        for i in 0..10 {
            let storage = storage.clone();
            handles.push(tokio::spawn(async move {
                let org = format!("org{}", i);
                let repo = test_desired_repo(&org, "repo");
                storage.save_desired(&org, &[repo]).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let orgs = storage.list_orgs().await.unwrap();
        assert_eq!(orgs.len(), 10);
    }
}
