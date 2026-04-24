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

use crate::v5::config::{load_workspaces, WorkspaceRepo};

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
    /// Full workspace detail (V5WS-3 `get`). `repos` preserves each
    /// entry's original on-disk shape (string shorthand or `{ref,dir}`
    /// object) and source order.
    WorkspaceDetail {
        name: String,
        path: String,
        repos: Vec<WorkspaceRepo>,
    },
    /// Generic error. `code` is a `snake_case` discriminator drawn from
    /// the emitting method's closed error set.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

/// Validate a `WorkspaceName`: ≤64 chars, ASCII, no `/`, no leading `.`.
fn is_valid_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    if s.starts_with('.') {
        return false;
    }
    if !s.is_ascii() {
        return false;
    }
    !s.contains('/')
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

    /// Return one workspace's full detail.
    ///
    /// Emits a single `workspace_detail` event whose `repos` preserves
    /// the on-disk mix of string-form and object-form entries in
    /// source order. Missing workspace → typed not-found.
    #[plexus_macros::method(params(name = "Workspace name to fetch"))]
    pub async fn get(
        &self,
        name: String,
    ) -> impl Stream<Item = WorkspacesEvent> + Send + 'static {
        let ws_dir = self.config_dir.join("workspaces");
        stream! {
            if name.is_empty() {
                yield WorkspacesEvent::Error {
                    code: Some("missing_param".into()),
                    message: "required parameter 'name' is missing".into(),
                };
                return;
            }
            if !is_valid_name(&name) {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_name".into()),
                    message: format!("invalid workspace name: {name}"),
                };
                return;
            }
            let path = ws_dir.join(format!("{name}.yaml"));
            let raw = match std::fs::read_to_string(&path) {
                Ok(r) => r,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    yield WorkspacesEvent::Error {
                        code: Some("not_found".into()),
                        message: format!("workspace not found: {name}"),
                    };
                    return;
                }
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("io_error".into()),
                        message: format!("read {}: {e}", path.display()),
                    };
                    return;
                }
            };
            let cfg: crate::v5::config::WorkspaceConfig = match serde_yaml::from_str(&raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", path.display()),
                    };
                    return;
                }
            };
            yield WorkspacesEvent::WorkspaceDetail {
                name: cfg.name.as_str().to_string(),
                path: cfg.path.as_str().to_string(),
                repos: cfg.repos,
            };
        }
    }
}
