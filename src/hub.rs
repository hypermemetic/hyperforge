//! HyperforgeHub - Root activation for hyperforge

use async_stream::stream;
use futures::Stream;
use hub_macro::hub_methods;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        Self {
            sync_service: Arc::new(SymmetricSyncService::new()),
        }
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
}
