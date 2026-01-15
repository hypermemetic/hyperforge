//! Import service - imports repositories from forges into local configuration.
//!
//! This service discovers repositories from forge APIs and creates corresponding
//! desired state entries in local storage. It handles the "import" workflow
//! where users bring existing repos under hyperforge management.

use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;

use crate::domain::{DesiredRepo, ObservedRepo};
use crate::ports::{ForgeError, ForgePort, StorageError, StoragePort};
use crate::types::{Forge, Visibility};

/// Errors that can occur during import operations.
#[derive(Debug, Error)]
pub enum ImportError {
    /// A forge operation failed
    #[error("Forge operation failed: {0}")]
    ForgeError(#[from] ForgeError),

    /// A storage operation failed
    #[error("Storage operation failed: {0}")]
    StorageError(#[from] StorageError),

    /// Organization not found on any forge
    #[error("Organization '{org}' not found on any configured forge")]
    OrgNotFound { org: String },

    /// No forge adapters configured
    #[error("No forge adapters configured for import")]
    NoForgesConfigured,
}

/// Options for import operation.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Include private repositories
    pub include_private: bool,
    /// Only import from specific forges (empty = all configured forges)
    pub from_forges: HashSet<Forge>,
    /// Skip repos that already exist in config
    pub skip_existing: bool,
    /// Dry run - don't actually save to storage
    pub dry_run: bool,
}

impl ImportOptions {
    /// Create options for importing all repos from all forges.
    pub fn all() -> Self {
        Self {
            include_private: true,
            from_forges: HashSet::new(),
            skip_existing: false,
            dry_run: false,
        }
    }

    /// Create options for importing public repos only.
    pub fn public_only() -> Self {
        Self {
            include_private: false,
            from_forges: HashSet::new(),
            skip_existing: false,
            dry_run: false,
        }
    }

    /// Builder: set include_private
    pub fn with_private(mut self, include: bool) -> Self {
        self.include_private = include;
        self
    }

    /// Builder: limit to specific forges
    pub fn from_forge(mut self, forge: Forge) -> Self {
        self.from_forges.insert(forge);
        self
    }

    /// Builder: skip existing repos
    pub fn skip_existing(mut self) -> Self {
        self.skip_existing = true;
        self
    }

    /// Builder: set dry run
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// Result of an import operation.
#[derive(Debug, Clone)]
pub struct ImportResult {
    /// Organization that was imported
    pub org: String,
    /// Repos that were imported (new to config)
    pub imported: Vec<DesiredRepo>,
    /// Repos that were skipped (already in config)
    pub skipped: Vec<String>,
    /// Repos that were discovered on forges
    pub discovered: Vec<ObservedRepo>,
    /// Errors encountered for specific repos
    pub errors: Vec<(String, String)>,
}

impl ImportResult {
    /// Create a new empty import result.
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            imported: Vec::new(),
            skipped: Vec::new(),
            discovered: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Total number of repos processed.
    pub fn total(&self) -> usize {
        self.imported.len() + self.skipped.len() + self.errors.len()
    }

    /// Check if the import was successful (no errors).
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Service for importing repositories from forges.
///
/// Discovers repos on forges and creates corresponding desired state
/// entries in local storage.
pub struct ImportService {
    /// Forge adapters for querying
    forges: Vec<Arc<dyn ForgePort>>,
    /// Storage adapter for saving state
    storage: Arc<dyn StoragePort>,
}

impl ImportService {
    /// Create a new import service with the given ports.
    ///
    /// # Arguments
    ///
    /// * `forges` - List of forge adapters to query
    /// * `storage` - Storage adapter for persisting imported config
    pub fn new(forges: Vec<Arc<dyn ForgePort>>, storage: Arc<dyn StoragePort>) -> Self {
        Self { forges, storage }
    }

    /// Import repositories from forges for an organization.
    ///
    /// Queries configured forges for repos in the given org and creates
    /// corresponding desired state entries.
    ///
    /// # Arguments
    ///
    /// * `org` - The organization to import repos from
    /// * `options` - Import options
    ///
    /// # Returns
    ///
    /// An `ImportResult` describing what was imported.
    pub async fn import_org(
        &self,
        org: &str,
        options: &ImportOptions,
    ) -> Result<ImportResult, ImportError> {
        if self.forges.is_empty() {
            return Err(ImportError::NoForgesConfigured);
        }

        let mut result = ImportResult::new(org);

        // Determine which forges to query
        let forges_to_query: Vec<_> = if options.from_forges.is_empty() {
            self.forges.iter().collect()
        } else {
            self.forges
                .iter()
                .filter(|f| options.from_forges.contains(&f.forge_type()))
                .collect()
        };

        if forges_to_query.is_empty() {
            return Err(ImportError::NoForgesConfigured);
        }

        // Query each forge for repos
        let mut all_observed: Vec<ObservedRepo> = Vec::new();
        let mut discovered_forges: HashSet<Forge> = HashSet::new();

        for forge_adapter in forges_to_query {
            match forge_adapter.list_repos(org).await {
                Ok(repos) => {
                    discovered_forges.insert(forge_adapter.forge_type());
                    for observed in repos {
                        // Merge with existing observed state for this repo
                        if let Some(existing) = all_observed
                            .iter_mut()
                            .find(|o| o.identity == observed.identity)
                        {
                            // Merge forge states
                            for state in observed.forge_states {
                                *existing = existing.clone().with_forge_state(state);
                            }
                        } else {
                            all_observed.push(observed);
                        }
                    }
                }
                Err(ForgeError::OrgNotFound { .. }) => {
                    // Org doesn't exist on this forge, continue
                    continue;
                }
                Err(e) => {
                    result
                        .errors
                        .push((forge_adapter.forge_type().to_string(), e.to_string()));
                }
            }
        }

        if discovered_forges.is_empty() && result.errors.is_empty() {
            return Err(ImportError::OrgNotFound { org: org.to_string() });
        }

        result.discovered = all_observed.clone();

        // Load existing desired state if skip_existing is set
        let existing_repos: HashSet<String> = if options.skip_existing {
            match self.storage.load_desired(org).await {
                Ok(repos) => repos.into_iter().map(|r| r.identity.name).collect(),
                Err(StorageError::OrgNotFound(_)) => HashSet::new(),
                Err(e) => return Err(ImportError::StorageError(e)),
            }
        } else {
            HashSet::new()
        };

        // Convert observed repos to desired repos
        for observed in all_observed {
            // Skip private repos if not included
            if !options.include_private {
                let is_private = observed.forge_states.iter().any(|s| {
                    s.visibility
                        .as_ref()
                        .map(|v| *v == Visibility::Private)
                        .unwrap_or(false)
                });
                if is_private {
                    continue;
                }
            }

            // Skip existing if configured
            if options.skip_existing && existing_repos.contains(&observed.identity.name) {
                result.skipped.push(observed.identity.name.clone());
                continue;
            }

            // Create desired repo from observed state
            let desired = self.observed_to_desired(&observed, &discovered_forges);
            result.imported.push(desired);
        }

        // Save to storage if not a dry run
        if !options.dry_run && !result.imported.is_empty() {
            // Merge with existing repos if skip_existing was set
            let repos_to_save = if options.skip_existing {
                let mut existing = self
                    .storage
                    .load_desired(org)
                    .await
                    .unwrap_or_default();
                existing.extend(result.imported.clone());
                existing
            } else {
                result.imported.clone()
            };

            self.storage.save_desired(org, &repos_to_save).await?;

            // Also save observed state as synced state
            if !result.discovered.is_empty() {
                self.storage.save_synced(org, &result.discovered).await?;
            }
        }

        Ok(result)
    }

    /// Convert observed repo to desired repo.
    fn observed_to_desired(
        &self,
        observed: &ObservedRepo,
        target_forges: &HashSet<Forge>,
    ) -> DesiredRepo {
        // Determine visibility from observed state (prefer most restrictive)
        let visibility = observed
            .forge_states
            .iter()
            .filter_map(|s| s.visibility.clone())
            .find(|v| *v == Visibility::Private)
            .unwrap_or(Visibility::Public);

        // Determine description from observed state
        let description = observed
            .forge_states
            .iter()
            .find_map(|s| s.description.clone());

        // Use forges where repo exists, plus any additional target forges
        let mut forges: HashSet<Forge> = observed.existing_forges().into_iter().collect();
        forges.extend(target_forges.iter().cloned());

        DesiredRepo::new(observed.identity.clone(), visibility, forges)
            .with_description(description.unwrap_or_default())
    }

    /// Import a single repository by name.
    ///
    /// Looks up the repo on configured forges and creates a desired state entry.
    pub async fn import_repo(
        &self,
        org: &str,
        repo_name: &str,
        options: &ImportOptions,
    ) -> Result<Option<DesiredRepo>, ImportError> {
        let result = self.import_org(org, options).await?;

        Ok(result
            .imported
            .into_iter()
            .find(|r| r.identity.name == repo_name))
    }

    /// List available repos from forges without importing.
    ///
    /// Useful for preview/discovery before actual import.
    pub async fn discover_repos(
        &self,
        org: &str,
        options: &ImportOptions,
    ) -> Result<Vec<ObservedRepo>, ImportError> {
        // Use dry_run to prevent saving
        let mut opts = options.clone();
        opts.dry_run = true;

        let result = self.import_org(org, &opts).await?;
        Ok(result.discovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::InMemoryStorageAdapter;
    use crate::domain::{ForgeRepoState, RepoIdentity};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    // =========================================================================
    // MockForgePort - records calls and returns configured responses
    // =========================================================================

    struct MockForgePort {
        forge_type: Forge,
        /// Repos to return from list_repos, keyed by org
        repos_by_org: Mutex<std::collections::HashMap<String, Vec<ObservedRepo>>>,
        /// Count of list_repos calls
        list_repos_calls: AtomicUsize,
        /// Count of create_repo calls
        create_repo_calls: AtomicUsize,
    }

    impl MockForgePort {
        fn new(forge: Forge) -> Self {
            Self {
                forge_type: forge,
                repos_by_org: Mutex::new(std::collections::HashMap::new()),
                list_repos_calls: AtomicUsize::new(0),
                create_repo_calls: AtomicUsize::new(0),
            }
        }

        async fn set_repos(&self, org: &str, repos: Vec<ObservedRepo>) {
            let mut map = self.repos_by_org.lock().await;
            map.insert(org.to_string(), repos);
        }

        fn list_repos_call_count(&self) -> usize {
            self.list_repos_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ForgePort for MockForgePort {
        fn forge_type(&self) -> Forge {
            self.forge_type.clone()
        }

        async fn list_repos(&self, org: &str) -> Result<Vec<ObservedRepo>, ForgeError> {
            self.list_repos_calls.fetch_add(1, Ordering::SeqCst);

            let map = self.repos_by_org.lock().await;
            match map.get(org) {
                Some(repos) => Ok(repos.clone()),
                None => Err(ForgeError::OrgNotFound {
                    forge: self.forge_type.clone(),
                    org: org.to_string(),
                }),
            }
        }

        async fn create_repo(&self, _repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            self.create_repo_calls.fetch_add(1, Ordering::SeqCst);
            unimplemented!("not needed for import tests")
        }

        async fn update_repo(&self, _repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
            unimplemented!("not needed for import tests")
        }

        async fn delete_repo(&self, _identity: &RepoIdentity) -> Result<(), ForgeError> {
            unimplemented!("not needed for import tests")
        }

        async fn repo_exists(&self, _identity: &RepoIdentity) -> Result<bool, ForgeError> {
            unimplemented!("not needed for import tests")
        }
    }

    // =========================================================================
    // Test helpers
    // =========================================================================

    fn make_observed_public(org: &str, name: &str, forge: Forge) -> ObservedRepo {
        ObservedRepo::new(RepoIdentity::new(org, name)).with_forge_state(ForgeRepoState::found(
            forge.clone(),
            format!("https://{}/{}/{}", forge.ssh_host(), org, name),
            Visibility::Public,
            Some(format!("{}-id", name)),
            Some(format!("{} description", name)),
        ))
    }

    fn make_observed_private(org: &str, name: &str, forge: Forge) -> ObservedRepo {
        ObservedRepo::new(RepoIdentity::new(org, name)).with_forge_state(ForgeRepoState::found(
            forge.clone(),
            format!("https://{}/{}/{}", forge.ssh_host(), org, name),
            Visibility::Private,
            Some(format!("{}-id", name)),
            Some(format!("{} description", name)),
        ))
    }

    // =========================================================================
    // ImportService orchestration tests
    // =========================================================================

    #[tokio::test]
    async fn test_import_converts_forge_repos_to_desired_and_saves() {
        // Given: forge has repos [A, B]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos(
                "testorg",
                vec![
                    make_observed_public("testorg", "repo-a", Forge::GitHub),
                    make_observed_public("testorg", "repo-b", Forge::GitHub),
                ],
            )
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import org
        let result = service.import_org("testorg", &ImportOptions::all()).await.unwrap();

        // Then: both repos are imported
        assert_eq!(result.imported.len(), 2);
        assert!(result.imported.iter().any(|r| r.name() == "repo-a"));
        assert!(result.imported.iter().any(|r| r.name() == "repo-b"));
        assert!(result.is_success());

        // And: they are saved to storage
        let saved = storage.load_desired("testorg").await.unwrap();
        assert_eq!(saved.len(), 2);
    }

    #[tokio::test]
    async fn test_import_filters_private_repos_when_include_private_false() {
        // Given: forge has [public-repo, private-repo]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos(
                "testorg",
                vec![
                    make_observed_public("testorg", "public-repo", Forge::GitHub),
                    make_observed_private("testorg", "private-repo", Forge::GitHub),
                ],
            )
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import with include_private=false
        let result = service.import_org("testorg", &ImportOptions::public_only()).await.unwrap();

        // Then: only public repo is imported
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.imported[0].name(), "public-repo");

        // Discovered should still contain both
        assert_eq!(result.discovered.len(), 2);
    }

    #[tokio::test]
    async fn test_import_includes_private_repos_when_include_private_true() {
        // Given: forge has [public-repo, private-repo]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos(
                "testorg",
                vec![
                    make_observed_public("testorg", "public-repo", Forge::GitHub),
                    make_observed_private("testorg", "private-repo", Forge::GitHub),
                ],
            )
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import with include_private=true
        let result = service.import_org("testorg", &ImportOptions::all()).await.unwrap();

        // Then: both repos are imported
        assert_eq!(result.imported.len(), 2);
        assert!(result.imported.iter().any(|r| r.name() == "public-repo"));
        assert!(result.imported.iter().any(|r| r.name() == "private-repo"));
    }

    #[tokio::test]
    async fn test_import_skip_existing_does_not_overwrite() {
        // Given: storage already has repo-a
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let existing = DesiredRepo::new(
            RepoIdentity::new("testorg", "repo-a"),
            Visibility::Private, // existing has Private
            {
                let mut forges = HashSet::new();
                forges.insert(Forge::GitHub);
                forges
            },
        )
        .with_description("existing description");
        storage.save_desired("testorg", &[existing]).await.unwrap();

        // And: forge has [repo-a with Public, repo-b]
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos(
                "testorg",
                vec![
                    make_observed_public("testorg", "repo-a", Forge::GitHub), // Public on forge
                    make_observed_public("testorg", "repo-b", Forge::GitHub),
                ],
            )
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import with skip_existing=true
        let result = service
            .import_org("testorg", &ImportOptions::all().skip_existing())
            .await
            .unwrap();

        // Then: repo-a is skipped, repo-b is imported
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0], "repo-a");
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.imported[0].name(), "repo-b");

        // And: existing repo-a retains its original settings (Private)
        let saved = storage.load_desired("testorg").await.unwrap();
        let saved_repo_a = saved.iter().find(|r| r.name() == "repo-a").unwrap();
        assert_eq!(saved_repo_a.visibility, Visibility::Private);
        assert_eq!(saved_repo_a.description, Some("existing description".to_string()));
    }

    #[tokio::test]
    async fn test_import_merges_with_existing_when_skip_existing() {
        // Given: storage has [repo-a]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let existing = DesiredRepo::new(
            RepoIdentity::new("testorg", "repo-a"),
            Visibility::Public,
            {
                let mut forges = HashSet::new();
                forges.insert(Forge::GitHub);
                forges
            },
        );
        storage.save_desired("testorg", &[existing]).await.unwrap();

        // And: forge has [repo-a, repo-b]
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos(
                "testorg",
                vec![
                    make_observed_public("testorg", "repo-a", Forge::GitHub),
                    make_observed_public("testorg", "repo-b", Forge::GitHub),
                ],
            )
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import with skip_existing
        service
            .import_org("testorg", &ImportOptions::all().skip_existing())
            .await
            .unwrap();

        // Then: storage now has both repos
        let saved = storage.load_desired("testorg").await.unwrap();
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().any(|r| r.name() == "repo-a"));
        assert!(saved.iter().any(|r| r.name() == "repo-b"));
    }

    #[tokio::test]
    async fn test_import_dry_run_does_not_save() {
        // Given: forge has [repo-a]
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos("testorg", vec![make_observed_public("testorg", "repo-a", Forge::GitHub)])
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import with dry_run=true
        let result = service
            .import_org("testorg", &ImportOptions::all().dry_run())
            .await
            .unwrap();

        // Then: result shows what would be imported
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.imported[0].name(), "repo-a");

        // But: nothing is saved to storage
        let saved = storage.load_desired("testorg").await;
        assert!(matches!(saved, Err(crate::ports::StorageError::OrgNotFound(_))));
    }

    #[tokio::test]
    async fn test_import_returns_error_when_no_forges_configured() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = ImportService::new(vec![], storage);

        let result = service.import_org("testorg", &ImportOptions::all()).await;

        assert!(matches!(result, Err(ImportError::NoForgesConfigured)));
    }

    #[tokio::test]
    async fn test_import_returns_org_not_found_when_no_forge_has_org() {
        // Given: forge returns OrgNotFound
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        // Don't set any repos - will return OrgNotFound

        let service = ImportService::new(vec![mock_forge.clone()], storage);

        // When: import org that doesn't exist on forge
        let result = service.import_org("nonexistent", &ImportOptions::all()).await;

        // Then: returns OrgNotFound error
        assert!(matches!(result, Err(ImportError::OrgNotFound { org }) if org == "nonexistent"));
    }

    #[tokio::test]
    async fn test_import_saves_synced_state() {
        // Given: forge has repos
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos("testorg", vec![make_observed_public("testorg", "repo-a", Forge::GitHub)])
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import
        service.import_org("testorg", &ImportOptions::all()).await.unwrap();

        // Then: synced state is also saved (discovered repos)
        let synced = storage.load_synced("testorg").await.unwrap();
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].name(), "repo-a");
    }

    #[tokio::test]
    async fn test_import_from_specific_forge_only() {
        // Given: two forge adapters
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let github_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        let codeberg_forge = Arc::new(MockForgePort::new(Forge::Codeberg));

        github_forge
            .set_repos("testorg", vec![make_observed_public("testorg", "gh-repo", Forge::GitHub)])
            .await;
        codeberg_forge
            .set_repos("testorg", vec![make_observed_public("testorg", "cb-repo", Forge::Codeberg)])
            .await;

        let service = ImportService::new(vec![github_forge.clone(), codeberg_forge.clone()], storage.clone());

        // When: import from GitHub only
        let result = service
            .import_org("testorg", &ImportOptions::all().from_forge(Forge::GitHub))
            .await
            .unwrap();

        // Then: only GitHub repo is imported
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.imported[0].name(), "gh-repo");

        // And: only GitHub adapter was queried
        assert_eq!(github_forge.list_repos_call_count(), 1);
        assert_eq!(codeberg_forge.list_repos_call_count(), 0);
    }

    #[tokio::test]
    async fn test_discover_repos_does_not_save() {
        // Given: forge has repos
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos("testorg", vec![make_observed_public("testorg", "repo-a", Forge::GitHub)])
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: discover repos (preview mode)
        let discovered = service.discover_repos("testorg", &ImportOptions::all()).await.unwrap();

        // Then: returns discovered repos
        assert_eq!(discovered.len(), 1);

        // But: nothing is saved
        assert!(storage.load_desired("testorg").await.is_err());
    }

    #[tokio::test]
    async fn test_import_result_helpers() {
        let mut result = ImportResult::new("testorg");
        assert!(result.is_success());
        assert_eq!(result.total(), 0);

        result.imported.push(DesiredRepo::new(
            RepoIdentity::new("testorg", "repo1"),
            Visibility::Public,
            HashSet::new(),
        ));
        result.skipped.push("repo2".to_string());
        assert_eq!(result.total(), 2);
        assert!(result.is_success());

        result.errors.push(("repo3".to_string(), "error msg".to_string()));
        assert_eq!(result.total(), 3);
        assert!(!result.is_success()); // Has errors now
    }

    #[tokio::test]
    async fn test_import_options_builders() {
        let all = ImportOptions::all();
        assert!(all.include_private);
        assert!(!all.skip_existing);
        assert!(!all.dry_run);
        assert!(all.from_forges.is_empty());

        let public = ImportOptions::public_only();
        assert!(!public.include_private);

        let modified = ImportOptions::all()
            .with_private(false)
            .from_forge(Forge::GitHub)
            .from_forge(Forge::Codeberg)
            .skip_existing()
            .dry_run();
        assert!(!modified.include_private);
        assert!(modified.from_forges.contains(&Forge::GitHub));
        assert!(modified.from_forges.contains(&Forge::Codeberg));
        assert!(modified.skip_existing);
        assert!(modified.dry_run);
    }

    #[tokio::test]
    async fn test_import_preserves_visibility_from_forge() {
        // Given: forge has a private repo
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let mock_forge = Arc::new(MockForgePort::new(Forge::GitHub));
        mock_forge
            .set_repos("testorg", vec![make_observed_private("testorg", "private-repo", Forge::GitHub)])
            .await;

        let service = ImportService::new(vec![mock_forge.clone()], storage.clone());

        // When: import (include_private=true)
        let result = service.import_org("testorg", &ImportOptions::all()).await.unwrap();

        // Then: imported repo has Private visibility
        assert_eq!(result.imported.len(), 1);
        assert_eq!(result.imported[0].visibility, Visibility::Private);
    }
}
