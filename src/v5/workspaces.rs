//! `WorkspacesHub` — v5 workspaces namespace.
//!
//! V5CORE-8 shipped the empty static child. V5WS-2 attaches the first
//! real method (`list`); subsequent V5WS tickets layer in the rest of
//! the workspace lifecycle.
//!
//! Writes are atomic per D8; mutating methods default `dry_run: false`
//! per D7; error events follow D9 (`type: "error"` plus `code` and
//! `message`).

use std::path::PathBuf;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::config::load_workspaces;

/// Workspaces namespace. Each method reads/writes `workspaces/*.yaml`
/// under the daemon's config directory.
#[derive(Clone)]
pub struct WorkspacesHub {
    config_dir: PathBuf,
}

impl WorkspacesHub {
    #[must_use]
    pub const fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}

/// Events emitted by `WorkspacesHub` methods.
///
/// Serialized with `#[serde(tag = "type")]` per D9; every variant maps
/// to a `type` discriminator in `snake_case`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspacesEvent {
    /// One summary per workspace (V5WS-2 `list`).
    WorkspaceSummary {
        name: String,
        path: String,
        repo_count: u32,
    },
    /// Generic error. `code` is a `snake_case` discriminator drawn from
    /// the emitting method's closed error set.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

/// Workspaces CRUD + reconcile + sync.
#[plexus_macros::activation(
    namespace = "workspaces",
    description = "Workspaces CRUD",
    crate_path = "plexus_core"
)]
impl WorkspacesHub {
    /// List every workspace declared under `$HF_CONFIG/workspaces/`.
    ///
    /// Emits one `workspace_summary` event per parsed yaml file in
    /// ascending-name order. If `workspaces/` is absent the stream is
    /// empty (not an error). Parse failures surface as typed error
    /// events; other workspaces still stream.
    #[plexus_macros::method]
    pub async fn list(&self) -> impl Stream<Item = WorkspacesEvent> + Send + 'static {
        let ws_dir = self.config_dir.join("workspaces");
        stream! {
            if !ws_dir.is_dir() {
                return;
            }
            match load_workspaces(&ws_dir) {
                Ok(map) => {
                    for (_name, cfg) in map {
                        let count = u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX);
                        yield WorkspacesEvent::WorkspaceSummary {
                            name: cfg.name.as_str().to_string(),
                            path: cfg.path.as_str().to_string(),
                            repo_count: count,
                        };
                    }
                }
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_config".into()),
                        message: e.to_string(),
                    };
                }
            }
        }
    }
}
