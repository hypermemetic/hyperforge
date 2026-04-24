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

/// Validate an `FsPath`: absolute, no `..`, no trailing `/`.
fn is_valid_fspath(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if !s.starts_with('/') {
        return false;
    }
    if s.len() > 1 && s.ends_with('/') {
        return false;
    }
    !s.split('/').any(|seg| seg == "..")
}

/// Parse the `repos` parameter as a JSON array string. Missing or
/// empty → `Ok(vec![])`. Entries may be bare strings (shorthand
/// `<org>/<name>`) or `{ref, dir}` objects.
fn parse_repos_arg(raw: &Option<String>) -> Result<Vec<WorkspaceRepo>, String> {
    let Some(raw) = raw.as_ref() else {
        return Ok(Vec::new());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<serde_json::Value> = serde_json::from_str(trimmed)
        .map_err(|e| format!("repos must be a JSON array: {e}"))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            serde_json::Value::String(s) => out.push(WorkspaceRepo::Shorthand(s)),
            serde_json::Value::Object(_) => {
                let entry: WorkspaceRepo = serde_json::from_value(item)
                    .map_err(|e| format!("malformed object entry: {e}"))?;
                out.push(entry);
            }
            other => return Err(format!("unsupported repos entry: {other}")),
        }
    }
    Ok(out)
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

    /// Create a new workspace yaml.
    ///
    /// Every repo ref in `repos` must resolve against its
    /// `orgs/<org>.yaml` — unresolved entries abort the write and
    /// surface as a typed error naming every offender. `ws_path` is
    /// named to avoid synapse's path-expansion of a parameter named
    /// `path`. D7: `dry_run` defaults false; on dry runs no file is
    /// written but the same event is emitted. D8: writes are atomic.
    #[plexus_macros::method(params(
        name = "New workspace name (filename-safe)",
        ws_path = "Absolute path where clones will live (named ws_path to avoid synapse path-expansion)",
        repos = "JSON array of repo refs (strings or {ref,dir} objects); default []",
        dry_run = "Preview without writing; default false",
    ))]
    pub async fn create(
        &self,
        name: String,
        ws_path: String,
        repos: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = WorkspacesEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        let dry = dry_run.unwrap_or(false);
        stream! {
            if !is_valid_name(&name) {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_name".into()),
                    message: format!("invalid workspace name: {name}"),
                };
                return;
            }
            if !is_valid_fspath(&ws_path) {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_path".into()),
                    message: format!("invalid ws_path: {ws_path}"),
                };
                return;
            }
            let parsed_repos = match parse_repos_arg(&repos) {
                Ok(r) => r,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_repos".into()),
                        message: e,
                    };
                    return;
                }
            };
            let ws_dir = config_dir.join("workspaces");
            let target = ws_dir.join(format!("{name}.yaml"));
            if target.exists() {
                yield WorkspacesEvent::Error {
                    code: Some("already_exists".into()),
                    message: format!("workspace already exists: {name}"),
                };
                return;
            }
            // Validate every ref against its org yaml.
            let orgs_dir = config_dir.join("orgs");
            let orgs = match crate::v5::config::load_orgs(&orgs_dir) {
                Ok(m) => m,
                Err(_) if !orgs_dir.exists() => std::collections::BTreeMap::new(),
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_config".into()),
                        message: e.to_string(),
                    };
                    return;
                }
            };
            let mut bad: Vec<String> = Vec::new();
            for entry in &parsed_repos {
                let (org_s, name_s) = match entry {
                    WorkspaceRepo::Shorthand(s) => {
                        let Some((o, n)) = s.split_once('/') else {
                            bad.push(s.clone());
                            continue;
                        };
                        (o.to_string(), n.to_string())
                    }
                    WorkspaceRepo::Object { reference, .. } => (
                        reference.org.as_str().to_string(),
                        reference.name.as_str().to_string(),
                    ),
                };
                let Some(org_cfg) = orgs.get(&org_s.clone().into()) else {
                    bad.push(format!("{org_s}/{name_s}"));
                    continue;
                };
                if !org_cfg.repos.iter().any(|r| r.name.as_str() == name_s) {
                    bad.push(format!("{org_s}/{name_s}"));
                }
            }
            if !bad.is_empty() {
                yield WorkspacesEvent::Error {
                    code: Some("unresolved_ref".into()),
                    message: format!("unresolved repo refs: {}", bad.join(", ")),
                };
                return;
            }
            let count = u32::try_from(parsed_repos.len()).unwrap_or(u32::MAX);
            let cfg = crate::v5::config::WorkspaceConfig {
                name: name.clone().into(),
                path: ws_path.clone().into(),
                repos: parsed_repos,
            };
            if !dry {
                if let Err(e) = std::fs::create_dir_all(&ws_dir) {
                    yield WorkspacesEvent::Error {
                        code: Some("io_error".into()),
                        message: format!("create {}: {e}", ws_dir.display()),
                    };
                    return;
                }
                if let Err(e) = crate::v5::config::save_workspace(&ws_dir, &cfg) {
                    yield WorkspacesEvent::Error {
                        code: Some("io_error".into()),
                        message: e.to_string(),
                    };
                    return;
                }
            }
            yield WorkspacesEvent::WorkspaceSummary {
                name,
                path: ws_path,
                repo_count: count,
            };
        }
    }
}
