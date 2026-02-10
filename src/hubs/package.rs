//! PackageHub - Package publishing lifecycle

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;

use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;

/// Sub-hub for package publishing lifecycle
#[derive(Clone)]
pub struct PackageHub {
    #[allow(dead_code)]
    pub(crate) state: HyperforgeState,
}

impl PackageHub {
    pub fn new(state: HyperforgeState) -> Self {
        Self { state }
    }
}

#[plexus_macros::hub_methods(
    namespace = "package",
    version = "3.0.0",
    description = "Package publishing lifecycle",
    crate_path = "plexus_core"
)]
impl PackageHub {
    /// Show package hub status
    #[plexus_macros::hub_method(description = "Show package publishing status")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Info {
                message: "Package hub ready. Use 'bump', 'publish', 'publish_all' etc.".to_string(),
            };
        }
    }
}

#[async_trait]
impl ChildRouter for PackageHub {
    fn router_namespace(&self) -> &str {
        "package"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
