//! ConfigHub - Org-level configuration

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;

use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;

/// Sub-hub for org-level configuration
#[derive(Clone)]
pub struct ConfigHub {
    #[allow(dead_code)]
    pub(crate) state: HyperforgeState,
}

impl ConfigHub {
    pub fn new(state: HyperforgeState) -> Self {
        Self { state }
    }
}

#[plexus_macros::hub_methods(
    namespace = "config",
    version = "3.1.0",
    description = "Org-level configuration",
    crate_path = "plexus_core"
)]
impl ConfigHub {
    /// Show org configuration
    #[plexus_macros::hub_method(description = "Show org-level configuration")]
    pub async fn show(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let config_dir = self.state.config_dir.clone();

        stream! {
            yield HyperforgeEvent::Info {
                message: format!("Config directory: {}", config_dir.display()),
            };
        }
    }
}

#[async_trait]
impl ChildRouter for ConfigHub {
    fn router_namespace(&self) -> &str {
        "config"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
