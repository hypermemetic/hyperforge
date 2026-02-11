//! HyperforgeHub - Root activation for hyperforge
//!
//! This is a hub plugin that routes to child sub-hubs:
//! - repo: Single-repo operations and registry CRUD
//! - workspace: Multi-repo workspace orchestration
//! - package: Package publishing lifecycle
//! - config: Org-level configuration

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, ChildSummary, PlexusError, PlexusStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::adapters::{ForgePort, LocalForge};
use crate::hubs::{ConfigHub, HyperforgeState, PackageHub, RepoHub, WorkspaceHub};
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
    /// Per-repo check result (branch + clean status)
    RepoCheck {
        repo_name: String,
        path: String,
        branch: String,
        expected_branch: String,
        is_clean: bool,
        on_correct_branch: bool,
    },
    /// Per-repo push result
    RepoPush {
        repo_name: String,
        path: String,
        forge: String,
        success: bool,
        error: Option<String>,
    },
    /// Workspace-level summary
    WorkspaceSummary {
        total_repos: usize,
        configured_repos: usize,
        unconfigured_repos: usize,
        clean_repos: Option<usize>,
        dirty_repos: Option<usize>,
        wrong_branch_repos: Option<usize>,
        push_success: Option<usize>,
        push_failed: Option<usize>,
    },
}

/// Root hub for hyperforge operations
#[derive(Clone)]
pub struct HyperforgeHub {
    pub(crate) state: HyperforgeState,
}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        Self {
            state: HyperforgeState::new(),
        }
    }
}

impl Default for HyperforgeHub {
    fn default() -> Self {
        Self::new()
    }
}

#[plexus_macros::hub_methods(
    namespace = "hyperforge",
    version = "3.1.0",
    description = "Multi-forge repository management",
    crate_path = "plexus_core",
    hub
)]
impl HyperforgeHub {
    /// Show hyperforge status
    #[plexus_macros::hub_method(description = "Show hyperforge status and version")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Status {
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "Multi-forge repository management (LFORGE2)".to_string(),
            };
        }
    }

    /// Show version info
    #[plexus_macros::hub_method(description = "Show version information")]
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
    #[plexus_macros::hub_method(description = "Test workspace diff with sample data")]
    pub async fn test_diff(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let sync_service = self.state.sync_service.clone();

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

    /// Get child plugin summaries for the hub schema
    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        let repo = RepoHub::new(self.state.clone());
        let workspace = WorkspaceHub::new(self.state.clone());
        let package = PackageHub::new(self.state.clone());
        let config = ConfigHub::new(self.state.clone());

        vec![
            child_summary(&repo),
            child_summary(&workspace),
            child_summary(&package),
            child_summary(&config),
        ]
    }
}

/// Extract a ChildSummary from any Activation
fn child_summary<T: Activation>(activation: &T) -> ChildSummary {
    let schema = activation.plugin_schema();
    ChildSummary {
        namespace: schema.namespace,
        description: schema.description,
        hash: schema.hash,
    }
}

/// ChildRouter implementation for nested method routing
#[async_trait]
impl ChildRouter for HyperforgeHub {
    fn router_namespace(&self) -> &str {
        "hyperforge"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "repo" => Some(Box::new(RepoHub::new(self.state.clone()))),
            "workspace" => Some(Box::new(WorkspaceHub::new(self.state.clone()))),
            "package" => Some(Box::new(PackageHub::new(self.state.clone()))),
            "config" => Some(Box::new(ConfigHub::new(self.state.clone()))),
            _ => None,
        }
    }
}
