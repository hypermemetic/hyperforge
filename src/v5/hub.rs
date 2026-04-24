//! `HyperforgeHub` (v5) — root activation for the v5 rewrite.
//!
//! V5CORE-2 baseline: minimal scaffold with a `status` method that
//! returns the daemon version. Later V5CORE tickets refine the event
//! shape (V5CORE-5), attach child stubs (V5CORE-6/7/8), and add the
//! `resolve_secret` capability (V5CORE-4).
//!
//! plexus-macros 0.5 rejects activations with zero `#[method]`
//! functions, so `status` ships from V5CORE-2 onwards.

use std::path::PathBuf;
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Events emitted by the v5 root hub.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeV5Event {
    /// Daemon self-report. Additional fields land in V5CORE-5.
    Status { version: String },
    /// Generic error event.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

/// Root activation for hyperforge v5.
#[derive(Clone)]
pub struct HyperforgeHub {
    state: Arc<HubState>,
}

/// Shared read-only state the root hub threads into methods.
#[derive(Debug)]
pub struct HubState {
    /// Absolute, expanded config directory.
    pub config_dir: PathBuf,
}

impl HyperforgeHub {
    /// Construct a hub rooted at the given config directory.
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(HubState { config_dir }),
        }
    }
}

/// Hyperforge v5 root — minimal scaffold.
#[plexus_macros::activation(
    namespace = "hyperforge",
    description = "Hyperforge v5 root",
    crate_path = "plexus_core"
)]
impl HyperforgeHub {
    /// Daemon self-report.
    #[plexus_macros::method]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let _ = &self.state;
        stream! {
            yield HyperforgeV5Event::Status { version };
        }
    }
}
