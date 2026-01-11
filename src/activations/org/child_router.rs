use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError, ChildSummary,
};
use hub_macro::hub_methods;

use crate::storage::{HyperforgePaths, OrgConfig};
use crate::activations::{ReposActivation, SecretsActivation};
use crate::events::OrgEvent;

/// Child router for a specific organization (e.g., org.hypermemetic)
///
/// This router provides access to org-specific children like repos and secrets.
/// Stores org-level configuration to pass down to child activations.
pub struct OrgChildRouter {
    paths: Arc<HyperforgePaths>,
    org_name: String,
    org_config: OrgConfig,
}

impl OrgChildRouter {
    pub fn new(paths: Arc<HyperforgePaths>, org_name: String, org_config: OrgConfig) -> Self {
        Self { paths, org_name, org_config }
    }

    /// Get the organization name this router represents
    pub fn org_name(&self) -> &str {
        &self.org_name
    }

    /// Get the paths configuration
    pub fn paths(&self) -> &Arc<HyperforgePaths> {
        &self.paths
    }

    /// Get the organization configuration
    pub fn org_config(&self) -> &OrgConfig {
        &self.org_config
    }
}

#[hub_methods(
    namespace = "org_child",
    version = "1.0.0",
    description = "Organization namespace",
    crate_path = "hub_core",
    hub
)]
impl OrgChildRouter {
    /// Show organization info
    #[hub_method(description = "Show organization information")]
    pub async fn info(&self) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        stream! {
            yield OrgEvent::Info {
                name: org_name,
                message: "Organization namespace - use repos or secrets subcommands".into(),
            };
        }
    }

    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![
            ChildSummary {
                namespace: "repos".into(),
                description: "Repository management".into(),
                hash: "repos".into(),
            },
            ChildSummary {
                namespace: "secrets".into(),
                description: "Secrets management".into(),
                hash: "secrets".into(),
            },
        ]
    }
}

#[async_trait]
impl ChildRouter for OrgChildRouter {
    fn router_namespace(&self) -> &str {
        &self.org_name
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "repos" => Some(Box::new(ReposActivation::new(
                self.paths.clone(),
                self.org_name.clone(),
                self.org_config.clone(),
            ))),
            "secrets" => Some(Box::new(SecretsActivation::new(
                self.paths.clone(),
                self.org_name.clone(),
                self.org_config.clone(),
            ))),
            _ => None,
        }
    }
}
