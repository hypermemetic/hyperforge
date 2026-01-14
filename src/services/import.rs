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
    use crate::domain::RepoIdentity;

    #[tokio::test]
    async fn test_import_service_creation() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = ImportService::new(vec![], storage);

        assert!(service.forges.is_empty());
    }

    #[tokio::test]
    async fn test_import_options_builder() {
        let opts = ImportOptions::all()
            .from_forge(Forge::GitHub)
            .skip_existing()
            .dry_run();

        assert!(opts.include_private);
        assert!(opts.from_forges.contains(&Forge::GitHub));
        assert!(opts.skip_existing);
        assert!(opts.dry_run);
    }

    #[tokio::test]
    async fn test_import_options_public_only() {
        let opts = ImportOptions::public_only();

        assert!(!opts.include_private);
        assert!(opts.from_forges.is_empty());
    }

    #[tokio::test]
    async fn test_no_forges_configured_error() {
        let storage = Arc::new(InMemoryStorageAdapter::new());
        let service = ImportService::new(vec![], storage);

        let result = service
            .import_org("testorg", &ImportOptions::all())
            .await;

        assert!(matches!(result, Err(ImportError::NoForgesConfigured)));
    }

    #[tokio::test]
    async fn test_import_result_new() {
        let result = ImportResult::new("testorg");

        assert_eq!(result.org, "testorg");
        assert!(result.imported.is_empty());
        assert!(result.skipped.is_empty());
        assert!(result.discovered.is_empty());
        assert!(result.errors.is_empty());
        assert_eq!(result.total(), 0);
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_import_result_total() {
        let mut result = ImportResult::new("testorg");
        result.imported.push(DesiredRepo::new(
            RepoIdentity::new("testorg", "repo1"),
            Visibility::Public,
            HashSet::new(),
        ));
        result.skipped.push("repo2".to_string());
        result.errors.push(("repo3".to_string(), "error".to_string()));

        assert_eq!(result.total(), 3);
        assert!(!result.is_success()); // Has errors
    }
}
