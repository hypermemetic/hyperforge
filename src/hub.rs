//! HyperforgeHub - Root activation for hyperforge

use async_stream::stream;
use futures::Stream;
use hub_macro::hub_methods;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::{ForgePort, LocalForge, GitHubAdapter, CodebergAdapter, GitLabAdapter};
use crate::auth::KeychainAuthProvider;
use crate::services::SymmetricSyncService;
use crate::types::{Forge, Repo, Visibility};

/// Hyperforge event types
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeEvent {
    /// Status information
    Status {
        version: String,
        description: String,
    },
    /// General info message
    Info { message: String },
    /// Error message
    Error { message: String },
    /// Repository information
    Repo {
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Vec<String>,
        protected: bool,
    },
    /// Sync diff result - repo operation
    SyncOp {
        repo_name: String,
        operation: String, // "create", "update", "delete", "in_sync"
        forge: String,
    },
    /// Sync summary
    SyncSummary {
        forge: String,
        total: usize,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
    },
}

/// Root hub for hyperforge operations
#[derive(Clone)]
pub struct HyperforgeHub {
    sync_service: Arc<SymmetricSyncService>,
    /// Cached LocalForge instances per org
    local_forges: Arc<RwLock<HashMap<String, Arc<LocalForge>>>>,
    /// Base config directory
    config_dir: PathBuf,
}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("hyperforge");

        Self {
            sync_service: Arc::new(SymmetricSyncService::new()),
            local_forges: Arc::new(RwLock::new(HashMap::new())),
            config_dir,
        }
    }

    /// Get or create LocalForge for an org with file persistence
    async fn get_local_forge(&self, org: &str) -> Arc<LocalForge> {
        // Try to get existing
        {
            let forges = self.local_forges.read().unwrap();
            if let Some(forge) = forges.get(org) {
                return forge.clone();
            }
        }

        // Create new with persistence
        let yaml_path = self.config_dir.join("orgs").join(org).join("repos.yaml");
        let forge = Arc::new(LocalForge::with_config_path(org, yaml_path));

        // Try to load existing state
        let _ = forge.load_from_yaml().await;

        // Cache it
        {
            let mut forges = self.local_forges.write().unwrap();
            forges.insert(org.to_string(), forge.clone());
        }

        forge
    }
}

impl Default for HyperforgeHub {
    fn default() -> Self {
        Self::new()
    }
}

#[hub_methods(
    namespace = "hyperforge",
    version = "2.0.0",
    description = "Multi-forge repository management",
    crate_path = "hub_core"
)]
impl HyperforgeHub {
    /// Show hyperforge status
    #[hub_method(description = "Show hyperforge status and version")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Status {
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "Multi-forge repository management (LFORGE2)".to_string(),
            };
        }
    }

    /// Show version info
    #[hub_method(description = "Show version information")]
    pub async fn version(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Info {
                message: format!(
                    "hyperforge {} (LFORGE2 - repo-local, git-native)",
                    env!("CARGO_PKG_VERSION")
                ),
            };
        }
    }

    /// Test workspace diff (demonstration)
    #[hub_method(description = "Test workspace diff with sample data")]
    pub async fn test_diff(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let sync_service = self.sync_service.clone();

        stream! {
            // Create test local and target forges
            let local = Arc::new(LocalForge::new("testorg"));
            let target = Arc::new(LocalForge::new("testorg"));

            // Add some test repos to local
            let repo1 = Repo::new("test-repo-1", Forge::GitHub)
                .with_description("Test repository 1");
            let repo2 = Repo::new("test-repo-2", Forge::Codeberg)
                .with_visibility(Visibility::Private);

            if let Err(e) = local.create_repo("testorg", &repo1).await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create test repo: {}", e),
                };
                return;
            }

            if let Err(e) = local.create_repo("testorg", &repo2).await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create test repo: {}", e),
                };
                return;
            }

            // Compute diff
            match sync_service.diff(local, target, "testorg").await {
                Ok(diff) => {
                    // Yield summary
                    yield HyperforgeEvent::SyncSummary {
                        forge: "test".to_string(),
                        total: diff.ops.len(),
                        to_create: diff.to_create().len(),
                        to_update: diff.to_update().len(),
                        to_delete: diff.to_delete().len(),
                        in_sync: diff.in_sync().len(),
                    };

                    // Yield individual operations
                    for op in diff.ops {
                        yield HyperforgeEvent::SyncOp {
                            repo_name: op.repo.name.clone(),
                            operation: format!("{:?}", op.op).to_lowercase(),
                            forge: "test".to_string(),
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Diff failed: {}", e),
                    };
                }
            }
        }
    }

    /// List repositories for an organization (from LocalForge)
    #[hub_method(
        description = "List all repositories in the local forge for an organization",
        params(org = "Organization name")
    )]
    pub async fn repos_list(
        &self,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            match local.list_repos(&org).await {
                Ok(repos) => {
                    for repo in repos {
                        yield HyperforgeEvent::Repo {
                            name: repo.name.clone(),
                            description: repo.description.clone(),
                            visibility: format!("{:?}", repo.visibility).to_lowercase(),
                            origin: format!("{:?}", repo.origin).to_lowercase(),
                            mirrors: repo.mirrors.iter()
                                .map(|f| format!("{:?}", f).to_lowercase())
                                .collect(),
                            protected: repo.protected,
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to list repos: {}", e),
                    };
                }
            }
        }
    }

    /// Create a new repository in LocalForge
    #[hub_method(
        description = "Create a new repository configuration",
        params(
            org = "Organization name",
            name = "Repository name",
            description = "Repository description (optional)",
            visibility = "Repository visibility: public or private",
            origin = "Origin forge: github, codeberg, or gitlab",
            mirrors = "Mirror forges (optional, comma-separated)"
        )
    )]
    pub async fn repos_create(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            // Parse forge from string
            let origin_forge = match origin.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid origin forge: {}. Must be github, codeberg, or gitlab", origin),
                    };
                    return;
                }
            };

            // Parse visibility
            let vis = match visibility.to_lowercase().as_str() {
                "public" => Visibility::Public,
                "private" => Visibility::Private,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid visibility: {}. Must be public or private", visibility),
                    };
                    return;
                }
            };

            // Parse mirrors
            let mirror_forges: Vec<Forge> = if let Some(m) = mirrors {
                m.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| match s.to_lowercase().as_str() {
                        "github" => Some(Forge::GitHub),
                        "codeberg" => Some(Forge::Codeberg),
                        "gitlab" => Some(Forge::GitLab),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Build repo
            let mut repo = Repo::new(name, origin_forge).with_visibility(vis);
            if let Some(desc) = description {
                repo = repo.with_description(desc);
            }
            repo = repo.with_mirrors(mirror_forges);

            // Get or create LocalForge with persistence
            let local = hub.get_local_forge(&org).await;

            match local.create_repo(&org, &repo).await {
                Ok(_) => {
                    // Save to YAML
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Created repository: {}", repo.name),
                    };
                    yield HyperforgeEvent::Repo {
                        name: repo.name.clone(),
                        description: repo.description.clone(),
                        visibility: format!("{:?}", repo.visibility).to_lowercase(),
                        origin: format!("{:?}", repo.origin).to_lowercase(),
                        mirrors: repo.mirrors.iter()
                            .map(|f| format!("{:?}", f).to_lowercase())
                            .collect(),
                        protected: repo.protected,
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create repo: {}", e),
                    };
                }
            }
        }
    }

    /// Update an existing repository
    #[hub_method(
        description = "Update repository configuration",
        params(
            org = "Organization name",
            name = "Repository name",
            description = "New repository description (optional)",
            visibility = "New visibility: public or private (optional)"
        )
    )]
    pub async fn repos_update(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            // Get existing repo
            let mut repo = match local.get_repo(&org, &name).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to get repo: {}", e),
                    };
                    return;
                }
            };

            // Update fields
            if let Some(desc) = description {
                repo.description = Some(desc);
            }

            if let Some(vis) = visibility {
                repo.visibility = match vis.to_lowercase().as_str() {
                    "public" => Visibility::Public,
                    "private" => Visibility::Private,
                    _ => {
                        yield HyperforgeEvent::Error {
                            message: format!("Invalid visibility: {}. Must be public or private", vis),
                        };
                        return;
                    }
                };
            }

            match local.update_repo(&org, &repo).await {
                Ok(_) => {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Updated repository: {}", repo.name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to update repo: {}", e),
                    };
                }
            }
        }
    }

    /// Delete a repository
    #[hub_method(
        description = "Delete a repository from local configuration",
        params(
            org = "Organization name",
            name = "Repository name"
        )
    )]
    pub async fn repos_delete(
        &self,
        org: String,
        name: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            match local.delete_repo(&org, &name).await {
                Ok(_) => {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Deleted repository: {}", name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to delete repo: {}", e),
                    };
                }
            }
        }
    }

    /// Import repositories from a remote forge
    #[hub_method(
        description = "Import repository configurations from a remote forge (GitHub, Codeberg, GitLab)",
        params(
            org = "Organization name",
            forge = "Source forge: github, codeberg, or gitlab"
        )
    )]
    pub async fn repos_import(
        &self,
        org: String,
        forge: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            // Parse forge
            let source_forge = match forge.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge),
                    };
                    return;
                }
            };

            // Get forge adapter
            let auth = Arc::new(KeychainAuthProvider::new(org.clone()));
            let adapter: Arc<dyn ForgePort> = match source_forge {
                Forge::GitHub => {
                    match GitHubAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitHub adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::Codeberg => {
                    match CodebergAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create Codeberg adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::GitLab => {
                    match GitLabAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitLab adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Fetching repositories from {} for {}...", forge, org),
            };

            // List repos from remote forge
            let repos = match adapter.list_repos(&org).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to list repos from {}: {}", forge, e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Found {} repositories", repos.len()),
            };

            // Get local forge
            let local = hub.get_local_forge(&org).await;

            // Import each repo
            let mut imported = 0;
            let mut skipped = 0;
            let mut errors = 0;

            for repo in repos {
                // Check if already exists
                let exists = match local.repo_exists(&org, &repo.name).await {
                    Ok(exists) => exists,
                    Err(e) => {
                        errors += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to check if {} exists: {}", repo.name, e),
                        };
                        continue;
                    }
                };

                if exists {
                    skipped += 1;
                    continue;
                }

                // Create in local forge
                match local.create_repo(&org, &repo).await {
                    Ok(_) => {
                        imported += 1;
                        yield HyperforgeEvent::Repo {
                            name: repo.name.clone(),
                            description: repo.description.clone(),
                            visibility: format!("{:?}", repo.visibility).to_lowercase(),
                            origin: format!("{:?}", repo.origin).to_lowercase(),
                            mirrors: repo.mirrors.iter()
                                .map(|f| format!("{:?}", f).to_lowercase())
                                .collect(),
                            protected: repo.protected,
                        };
                    }
                    Err(e) => {
                        errors += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to import {}: {}", repo.name, e),
                        };
                    }
                }
            }

            // Save to YAML
            if let Err(e) = local.save_to_yaml().await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to save repos.yaml: {}", e),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Import complete: {} imported, {} skipped (already exist), {} errors",
                    imported, skipped, errors
                ),
            };
        }
    }

    /// Compute sync diff between local and a remote forge
    #[hub_method(
        description = "Compute diff between local configuration and a remote forge",
        params(
            org = "Organization name",
            forge = "Target forge: github, codeberg, or gitlab"
        )
    )]
    pub async fn workspace_diff(
        &self,
        org: String,
        forge: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let sync_service = self.sync_service.clone();

        stream! {
            // Parse forge
            let target_forge = match forge.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge),
                    };
                    return;
                }
            };

            // Get forge adapter
            let auth = Arc::new(KeychainAuthProvider::new(org.clone()));
            let adapter: Arc<dyn ForgePort> = match target_forge {
                Forge::GitHub => {
                    match GitHubAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitHub adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::Codeberg => {
                    match CodebergAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create Codeberg adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::GitLab => {
                    match GitLabAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitLab adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
            };

            // Get local forge
            let local = hub.get_local_forge(&org).await;

            yield HyperforgeEvent::Info {
                message: format!("Computing diff with {}...", forge),
            };

            // Compute diff
            match sync_service.diff(local, adapter, &org).await {
                Ok(diff) => {
                    // Yield summary
                    yield HyperforgeEvent::SyncSummary {
                        forge: forge.clone(),
                        total: diff.ops.len(),
                        to_create: diff.to_create().len(),
                        to_update: diff.to_update().len(),
                        to_delete: diff.to_delete().len(),
                        in_sync: diff.in_sync().len(),
                    };

                    // Yield individual operations
                    for op in diff.ops {
                        yield HyperforgeEvent::SyncOp {
                            repo_name: op.repo.name.clone(),
                            operation: format!("{:?}", op.op).to_lowercase(),
                            forge: forge.clone(),
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Diff failed: {}", e),
                    };
                }
            }
        }
    }

    /// Sync local configuration to a remote forge
    #[hub_method(
        description = "Sync repositories from local configuration to a remote forge",
        params(
            org = "Organization name",
            forge = "Target forge: github, codeberg, or gitlab",
            dry_run = "Preview changes without applying them (optional, default: false)"
        )
    )]
    pub async fn workspace_sync(
        &self,
        org: String,
        forge: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let sync_service = self.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            // Parse forge
            let target_forge = match forge.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge),
                    };
                    return;
                }
            };

            // Get forge adapter
            let auth = Arc::new(KeychainAuthProvider::new(org.clone()));
            let adapter: Arc<dyn ForgePort> = match target_forge {
                Forge::GitHub => {
                    match GitHubAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitHub adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::Codeberg => {
                    match CodebergAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create Codeberg adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::GitLab => {
                    match GitLabAdapter::new(auth) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitLab adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
            };

            // Get local forge
            let local = hub.get_local_forge(&org).await;

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("[DRY RUN] Computing sync operations for {}...", forge),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Syncing to {}...", forge),
                };
            }

            // Execute sync
            match sync_service.sync(local, adapter, &org, is_dry_run).await {
                Ok(diff) => {
                    let created = diff.to_create().len();
                    let updated = diff.to_update().len();
                    let deleted = diff.to_delete().len();
                    let in_sync = diff.in_sync().len();

                    yield HyperforgeEvent::Info {
                        message: format!(
                            "{} sync complete: {} created, {} updated, {} deleted, {} in sync",
                            if is_dry_run { "[DRY RUN]" } else { "" },
                            created,
                            updated,
                            deleted,
                            in_sync
                        ),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Sync failed: {}", e),
                    };
                }
            }
        }
    }
}
