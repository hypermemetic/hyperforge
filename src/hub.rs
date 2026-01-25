//! HyperforgeHub - Root activation for hyperforge

use async_stream::stream;
use futures::Stream;
use hub_macro::hub_methods;
use serde::{Deserialize, Serialize};

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
}

/// Root hub for hyperforge operations
#[derive(Clone)]
pub struct HyperforgeHub {}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        Self {}
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
}
