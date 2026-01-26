//! HyperforgeHub - Root activation for hyperforge

use async_stream::stream;
use futures::Stream;
use hub_macro::hub_methods;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::{ForgePort, LocalForge};
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
}
