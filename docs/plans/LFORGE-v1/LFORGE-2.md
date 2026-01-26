# LFORGE-2: Create LocalForge

**blocked_by:** []
**unlocks:** [LFORGE-3, LFORGE-4, LFORGE-5]

## Scope

Implement `LocalForge`, an in-memory `ForgePort` implementation that stores repositories in a thread-safe HashMap. This is the foundational piece that enables symmetric sync operations - treating local storage as just another forge.

## Deliverables

1. `src/adapters/local_forge.rs` with `LocalForge` struct implementing `ForgePort`
2. Thread-safe storage using `RwLock<HashMap<String, HashMap<String, Repo>>>`
3. Constructor methods: `LocalForge::new()` and `LocalForge::with_repos()`
4. All `ForgePort` trait methods implemented
5. Unit tests for all methods

## Verification Steps

```bash
# Run the LocalForge unit tests
cd ~/dev/controlflow/hypermemetic/hyperforge
cargo test local_forge

# Verify the module is exported
cargo doc --open
# Navigate to adapters::LocalForge
```

## Implementation Notes

### File Structure

Create `src/adapters/local_forge.rs`:

```rust
//! In-memory forge implementation for local repository state.
//!
//! LocalForge treats local configuration as a forge, enabling symmetric
//! sync operations between any two forges (including local).

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::domain::{DesiredRepo, ObservedRepo, RepoIdentity, ForgeRepoState};
use crate::ports::{ForgePort, ForgeError};
use crate::types::{Forge, Visibility};

/// In-memory forge for local repository state.
///
/// Stores repos in a nested HashMap: org -> repo_name -> Repo.
/// Thread-safe via RwLock for concurrent access.
pub struct LocalForge {
    /// Nested map: org_name -> (repo_name -> repo_data)
    repos: RwLock<HashMap<String, HashMap<String, LocalRepo>>>,
}

/// Internal representation of a repo in LocalForge
#[derive(Debug, Clone)]
struct LocalRepo {
    identity: RepoIdentity,
    description: Option<String>,
    visibility: Visibility,
}

impl LocalForge {
    /// Create a new empty LocalForge
    pub fn new() -> Self {
        Self {
            repos: RwLock::new(HashMap::new()),
        }
    }

    /// Create a LocalForge pre-populated with repos
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

    /// Get a snapshot of all repos for an org (for testing)
    pub fn get_org_repos(&self, org: &str) -> Vec<RepoIdentity> {
        let map = self.repos.read().unwrap();
        map.get(org)
            .map(|org_repos| org_repos.values().map(|r| r.identity.clone()).collect())
            .unwrap_or_default()
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
        // Use a special "Local" forge type, or reuse an existing one
        // For now, we'll use GitHub as placeholder - LFORGE-3 will add Forge::Local
        Forge::GitHub  // TODO: Change to Forge::Local in LFORGE-3
    }

    async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
        let map = self.repos.read().unwrap();
        let repos = map.get(org).map(|org_repos| {
            org_repos
                .values()
                .map(|local| {
                    ObservedRepo::new(local.identity.clone())
                        .with_forge_state(ForgeRepoState::found(
                            self.forge_type(),
                            format!("local://{}/{}", local.identity.org, local.identity.name),
                            local.visibility.clone(),
                            None,
                            local.description.clone(),
                        ))
                })
                .collect()
        }).unwrap_or_default();

        Ok(repos)
    }

    async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
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
        org_repos.insert(repo.identity.name.clone(), local.clone());

        Ok(ObservedRepo::new(repo.identity.clone())
            .with_forge_state(ForgeRepoState::found(
                self.forge_type(),
                format!("local://{}/{}", repo.identity.org, repo.identity.name),
                repo.visibility.clone(),
                None,
                repo.description.clone(),
            )))
    }

    async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        let mut map = self.repos.write().unwrap();
        let org_repos = map.get_mut(&repo.identity.org)
            .ok_or_else(|| ForgeError::RepoNotFound(repo.identity.clone()))?;

        let local = org_repos.get_mut(&repo.identity.name)
            .ok_or_else(|| ForgeError::RepoNotFound(repo.identity.clone()))?;

        local.description = repo.description.clone();
        local.visibility = repo.visibility.clone();

        Ok(ObservedRepo::new(repo.identity.clone())
            .with_forge_state(ForgeRepoState::found(
                self.forge_type(),
                format!("local://{}/{}", repo.identity.org, repo.identity.name),
                repo.visibility.clone(),
                None,
                repo.description.clone(),
            )))
    }

    async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError> {
        let mut map = self.repos.write().unwrap();
        let org_repos = map.get_mut(&identity.org)
            .ok_or_else(|| ForgeError::RepoNotFound(identity.clone()))?;

        if org_repos.remove(&identity.name).is_none() {
            return Err(ForgeError::RepoNotFound(identity.clone()));
        }

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
```

### Export in mod.rs

Add to `src/adapters/mod.rs`:

```rust
mod local_forge;
pub use local_forge::LocalForge;
```

### Test Structure

```rust
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
        DesiredRepo::new(
            RepoIdentity::new(org, name),
            Visibility::Public,
            github_forges(),
        )
    }

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
}
```

### Key Design Decisions

1. **RwLock over Mutex**: Allows concurrent reads, only blocks on writes
2. **Nested HashMap**: `org -> repo_name -> repo` matches ForgePort semantics
3. **No Forge::Local yet**: Using Forge::GitHub as placeholder until LFORGE-3 adds it
4. **LocalRepo internal type**: Simplified storage format, converts to/from domain types
5. **URL format**: `local://org/repo` clearly identifies local repos
