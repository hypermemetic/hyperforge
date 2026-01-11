//! HyperforgeHub - Root hub that ties all activations together

use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError, ChildSummary,
};
use hub_macro::hub_methods;

use crate::storage::{HyperforgePaths, GlobalConfig};
use crate::activations::{
    OrgActivation,
    ForgeActivation,
    WorkspaceActivation,
};

/// Root hub event type
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeEvent {
    Status {
        version: String,
        config_dir: String,
        default_org: Option<String>,
        org_count: usize,
    },
    Info {
        message: String,
    },
}

#[derive(Clone)]
pub struct HyperforgeHub {
    paths: Arc<HyperforgePaths>,
}

impl HyperforgeHub {
    pub fn new() -> Self {
        Self {
            paths: Arc::new(HyperforgePaths::new()),
        }
    }

    pub fn with_paths(paths: HyperforgePaths) -> Self {
        Self {
            paths: Arc::new(paths),
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
    description = "Multi-forge infrastructure management",
    crate_path = "hub_core",
    hub
)]
impl HyperforgeHub {
    /// Show hyperforge status
    #[hub_method(description = "Show hyperforge status and configuration")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            match GlobalConfig::load(&paths).await {
                Ok(config) => {
                    yield HyperforgeEvent::Status {
                        version: env!("CARGO_PKG_VERSION").to_string(),
                        config_dir: paths.config_dir.display().to_string(),
                        default_org: config.default_org.clone(),
                        org_count: config.organizations.len(),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Info {
                        message: format!("Error loading config: {}", e),
                    };
                }
            }
        }
    }

    /// Show version info
    #[hub_method(description = "Show version information")]
    pub async fn version(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Info {
                message: format!(
                    "hyperforge {} (hub architecture)",
                    env!("CARGO_PKG_VERSION")
                ),
            };
        }
    }

    /// List child summaries for schema
    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![
            ChildSummary {
                namespace: "org".into(),
                description: "Organization management".into(),
                hash: "org".into(),
            },
            ChildSummary {
                namespace: "forge".into(),
                description: "Direct forge API access".into(),
                hash: "forge".into(),
            },
            ChildSummary {
                namespace: "workspace".into(),
                description: "Workspace binding management".into(),
                hash: "workspace".into(),
            },
        ]
    }
}

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
            "org" => Some(Box::new(OrgActivation::new(self.paths.clone()))),
            "forge" => Some(Box::new(ForgeActivation::new(self.paths.clone()))),
            "workspace" => Some(Box::new(WorkspaceActivation::new(self.paths.clone()))),
            _ => None,
        }
    }
}
