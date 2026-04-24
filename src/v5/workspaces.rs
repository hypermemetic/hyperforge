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
    /// Delete acknowledgement (V5WS-5 `delete`).
    WorkspaceDeleted { name: String },
    /// Per-member cascade event for `delete_remote: true` flows.
    /// `status` is `forge_deleted` or `forge_delete_failed`; `message`
    /// is a free-text diagnostic on failure.
    ForgeDeleteResult {
        #[serde(rename = "ref")]
        reference: crate::v5::config::RepoRef,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// One reconcile observation (V5WS-8). `kind` is one of
    /// `matched`, `renamed`, `removed`, `new_matched`, `ambiguous`.
    ReconcileEvent {
        kind: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<crate::v5::config::RepoRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dir: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
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

/// Parse `<org>/<name>` string form. Returns `None` if the input does
/// not look like exactly one forward slash separating two non-empty
/// tokens that individually pass name validation.
fn parse_repo_ref_string(
    s: &str,
) -> Option<(crate::v5::config::OrgName, crate::v5::config::RepoName)> {
    let (org, name) = s.split_once('/')?;
    if !is_valid_name(org) || !is_valid_name(name) {
        return None;
    }
    Some((org.into(), name.into()))
}

/// Parse a `RepoRef` argument: accepts either `<org>/<name>` string
/// form or a JSON object `{"org":"<o>","name":"<n>"}`.
fn parse_repo_ref_arg(raw: &str) -> Option<crate::v5::config::RepoRef> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let org = parsed.get("org")?.as_str()?;
        let name = parsed.get("name")?.as_str()?;
        if is_valid_name(org) && is_valid_name(name) {
            return Some(crate::v5::config::RepoRef {
                org: org.into(),
                name: name.into(),
            });
        }
        return None;
    }
    let (org, name) = parse_repo_ref_string(trimmed)?;
    Some(crate::v5::config::RepoRef { org, name })
}

/// Ref key for comparing `WorkspaceRepo` entries regardless of shape.
fn ref_key(entry: &WorkspaceRepo) -> Option<(String, String)> {
    match entry {
        WorkspaceRepo::Shorthand(s) => {
            let (o, n) = s.split_once('/')?;
            Some((o.to_string(), n.to_string()))
        }
        WorkspaceRepo::Object { reference, .. } => Some((
            reference.org.as_str().to_string(),
            reference.name.as_str().to_string(),
        )),
    }
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

    /// Delete a workspace yaml, optionally cascading to forge-side
    /// repo deletion for each member.
    ///
    /// Cascade mode (`delete_remote: true`) emits one
    /// `forge_delete_result` event per member before the yaml is
    /// removed; the batch continues past per-member failures. v1 has
    /// no adapter path — cascade events report
    /// `forge_delete_failed` on real runs (or `forge_deleted` on
    /// `dry_run: true` so the preview shape is stable). V5REPOS-13
    /// wires the real adapter path; post-V5REPOS this method picks it
    /// up without signature changes.
    #[plexus_macros::method(params(
        name = "Workspace to delete",
        delete_remote = "Cascade forge-side deletion for each member; default false",
        dry_run = "Preview without writing; default false",
    ))]
    pub async fn delete(
        &self,
        name: String,
        delete_remote: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = WorkspacesEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        let dry = dry_run.unwrap_or(false);
        let cascade = delete_remote.unwrap_or(false);
        stream! {
            if !is_valid_name(&name) {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_name".into()),
                    message: format!("invalid workspace name: {name}"),
                };
                return;
            }
            let ws_dir = config_dir.join("workspaces");
            let target = ws_dir.join(format!("{name}.yaml"));
            let raw = match std::fs::read_to_string(&target) {
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
                        message: format!("read {}: {e}", target.display()),
                    };
                    return;
                }
            };
            let cfg: crate::v5::config::WorkspaceConfig = match serde_yaml::from_str(&raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", target.display()),
                    };
                    return;
                }
            };
            if cascade {
                for entry in &cfg.repos {
                    let Some((org, rname)) = ref_key(entry) else { continue };
                    let rref = crate::v5::config::RepoRef {
                        org: org.into(),
                        name: rname.into(),
                    };
                    let (status, msg) = if dry {
                        ("forge_deleted".to_string(), None)
                    } else {
                        (
                            "forge_delete_failed".to_string(),
                            Some("no forge adapter registered (V5REPOS-13 pending)".into()),
                        )
                    };
                    yield WorkspacesEvent::ForgeDeleteResult {
                        reference: rref,
                        status,
                        message: msg,
                    };
                }
            }
            if !dry {
                if let Err(e) = std::fs::remove_file(&target) {
                    yield WorkspacesEvent::Error {
                        code: Some("io_error".into()),
                        message: format!("remove {}: {e}", target.display()),
                    };
                    return;
                }
            }
            yield WorkspacesEvent::WorkspaceDeleted { name };
        }
    }

    /// Append one `<org>/<name>` repo ref to a workspace. The ref must
    /// resolve against `orgs/<org>.yaml`; duplicates (regardless of
    /// the existing entry's shape) fail without a write. Pinned here:
    /// the canonical string form `<org>/<name>` — one forward slash
    /// separating two tokens, each rejecting `/`.
    #[plexus_macros::method(params(
        name = "Workspace to extend",
        repo_ref = "Repo ref in '<org>/<name>' form (or {org,name} object)",
        dry_run = "Preview without writing; default false",
    ))]
    pub async fn add_repo(
        &self,
        name: String,
        repo_ref: String,
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
            let Some(rref) = parse_repo_ref_arg(&repo_ref) else {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_ref".into()),
                    message: format!("invalid repo_ref: {repo_ref}"),
                };
                return;
            };
            let ws_dir = config_dir.join("workspaces");
            let target = ws_dir.join(format!("{name}.yaml"));
            let raw = match std::fs::read_to_string(&target) {
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
                        message: format!("read {}: {e}", target.display()),
                    };
                    return;
                }
            };
            let mut cfg: crate::v5::config::WorkspaceConfig = match serde_yaml::from_str(&raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", target.display()),
                    };
                    return;
                }
            };
            // Validate against its org yaml.
            let orgs_dir = config_dir.join("orgs");
            let org_file = orgs_dir.join(format!("{}.yaml", rref.org));
            let org_raw = match std::fs::read_to_string(&org_file) {
                Ok(r) => r,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    yield WorkspacesEvent::Error {
                        code: Some("org_not_found".into()),
                        message: format!(
                            "org not found: {} (ref {}/{})",
                            rref.org, rref.org, rref.name
                        ),
                    };
                    return;
                }
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("io_error".into()),
                        message: format!("read {}: {e}", org_file.display()),
                    };
                    return;
                }
            };
            let org_cfg: crate::v5::config::OrgConfig = match serde_yaml::from_str(&org_raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", org_file.display()),
                    };
                    return;
                }
            };
            if !org_cfg
                .repos
                .iter()
                .any(|r| r.name.as_str() == rref.name.as_str())
            {
                yield WorkspacesEvent::Error {
                    code: Some("repo_not_found".into()),
                    message: format!("repo not found in org: {}/{}", rref.org, rref.name),
                };
                return;
            }
            let key = (rref.org.as_str().to_string(), rref.name.as_str().to_string());
            let already = cfg
                .repos
                .iter()
                .any(|e| ref_key(e).as_ref() == Some(&key));
            if already {
                yield WorkspacesEvent::Error {
                    code: Some("already_member".into()),
                    message: format!("already a member: {}/{}", rref.org, rref.name),
                };
                return;
            }
            cfg.repos
                .push(WorkspaceRepo::Shorthand(format!("{}/{}", rref.org, rref.name)));
            let count = u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX);
            let path_str = cfg.path.as_str().to_string();
            if !dry {
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
                path: path_str,
                repo_count: count,
            };
        }
    }

    /// Drop a member ref from a workspace, optionally cascading to
    /// forge-side deletion. An object-form entry `{ref, dir}` is
    /// matched by ref (the `dir` is not material); duplicate entries
    /// drop the first match. Cascade events follow the same v1
    /// skeletal shape as `delete` — real adapter path lands in
    /// V5REPOS-13.
    #[plexus_macros::method(params(
        name = "Workspace to modify",
        repo_ref = "Repo ref in '<org>/<name>' form (or {org,name} object)",
        delete_remote = "Cascade forge-side deletion of the ref; default false",
        dry_run = "Preview without writing; default false",
    ))]
    pub async fn remove_repo(
        &self,
        name: String,
        repo_ref: String,
        delete_remote: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = WorkspacesEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        let dry = dry_run.unwrap_or(false);
        let cascade = delete_remote.unwrap_or(false);
        stream! {
            if !is_valid_name(&name) {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_name".into()),
                    message: format!("invalid workspace name: {name}"),
                };
                return;
            }
            let Some(rref) = parse_repo_ref_arg(&repo_ref) else {
                yield WorkspacesEvent::Error {
                    code: Some("invalid_ref".into()),
                    message: format!("invalid repo_ref: {repo_ref}"),
                };
                return;
            };
            let ws_dir = config_dir.join("workspaces");
            let target = ws_dir.join(format!("{name}.yaml"));
            let raw = match std::fs::read_to_string(&target) {
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
                        message: format!("read {}: {e}", target.display()),
                    };
                    return;
                }
            };
            let mut cfg: crate::v5::config::WorkspaceConfig = match serde_yaml::from_str(&raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", target.display()),
                    };
                    return;
                }
            };
            let key = (rref.org.as_str().to_string(), rref.name.as_str().to_string());
            let idx = cfg
                .repos
                .iter()
                .position(|e| ref_key(e).as_ref() == Some(&key));
            let Some(idx) = idx else {
                yield WorkspacesEvent::Error {
                    code: Some("not_a_member".into()),
                    message: format!("not a member: {}/{}", rref.org, rref.name),
                };
                return;
            };
            if cascade {
                let (status, msg) = if dry {
                    ("forge_deleted".to_string(), None)
                } else {
                    (
                        "forge_delete_failed".to_string(),
                        Some("no forge adapter registered (V5REPOS-13 pending)".into()),
                    )
                };
                yield WorkspacesEvent::ForgeDeleteResult {
                    reference: rref.clone(),
                    status,
                    message: msg,
                };
            }
            cfg.repos.remove(idx);
            let count = u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX);
            let path_str = cfg.path.as_str().to_string();
            if !dry {
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
                path: path_str,
                repo_count: count,
            };
        }
    }

    /// Reconcile the workspace yaml with disk.
    ///
    /// Scans `path` in ASCII-ascending dir order, reads each git
    /// clone's `origin` URL out of `.git/config`, and matches against
    /// org-yaml remotes. Emits one `reconcile_event` per observation.
    /// Kinds: `matched`, `renamed`, `removed`, `new_matched`,
    /// `ambiguous` (CONTRACTS §types). Under D5, when multiple dirs
    /// share a URL the alphabetically-first wins; other candidates
    /// emit `ambiguous`. Non-dry runs rewrite the workspace yaml
    /// atomically (D8) to reflect `renamed` + `removed`; the scan is
    /// strictly read-only — no filesystem mutation under the
    /// workspace path and no forge contact.
    #[plexus_macros::method(params(
        name = "Workspace to reconcile",
        dry_run = "Preview without writing; default false",
    ))]
    pub async fn reconcile(
        &self,
        name: String,
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
            let ws_dir = config_dir.join("workspaces");
            let target = ws_dir.join(format!("{name}.yaml"));
            let raw = match std::fs::read_to_string(&target) {
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
                        message: format!("read {}: {e}", target.display()),
                    };
                    return;
                }
            };
            let mut cfg: crate::v5::config::WorkspaceConfig = match serde_yaml::from_str(&raw) {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspacesEvent::Error {
                        code: Some("invalid_yaml".into()),
                        message: format!("parse {}: {e}", target.display()),
                    };
                    return;
                }
            };
            // Load orgs for URL → ref lookup.
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
            // URL → sorted (org, name) candidates. Multiple refs may
            // share a URL (mirror repos); tiebreaker is alphabetical
            // `<org>/<name>`.
            let mut url_to_refs: std::collections::BTreeMap<
                String,
                Vec<(String, String)>,
            > = std::collections::BTreeMap::new();
            for (org_name, org_cfg) in &orgs {
                for repo in &org_cfg.repos {
                    for remote in &repo.remotes {
                        url_to_refs
                            .entry(remote.url.as_str().to_string())
                            .or_default()
                            .push((
                                org_name.as_str().to_string(),
                                repo.name.as_str().to_string(),
                            ));
                    }
                }
            }
            for v in url_to_refs.values_mut() {
                v.sort();
                v.dedup();
            }
            // Per-member info: reference, declared dir, known remotes.
            struct Member {
                reference: crate::v5::config::RepoRef,
                declared_dir: String,
                remote_urls: Vec<String>,
            }
            let mut members: Vec<Member> = Vec::new();
            for entry in &cfg.repos {
                let Some((org_s, name_s)) = ref_key(entry) else { continue };
                let reference = crate::v5::config::RepoRef {
                    org: org_s.clone().into(),
                    name: name_s.clone().into(),
                };
                let declared_dir = match entry {
                    WorkspaceRepo::Shorthand(_) => name_s.clone(),
                    WorkspaceRepo::Object { dir, .. } => dir.clone(),
                };
                let remote_urls = orgs
                    .get(&org_s.clone().into())
                    .and_then(|o| o.repos.iter().find(|r| r.name.as_str() == name_s))
                    .map(|r| {
                        r.remotes
                            .iter()
                            .map(|rem| rem.url.as_str().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                members.push(Member {
                    reference,
                    declared_dir,
                    remote_urls,
                });
            }
            // Scan the workspace path in ASCII-ascending dir order.
            let scan_root = std::path::PathBuf::from(cfg.path.as_str());
            let mut entries: Vec<(String, std::path::PathBuf)> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&scan_root) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if !p.is_dir() {
                        continue;
                    }
                    let Some(nm) = p.file_name().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    entries.push((nm.to_string(), p));
                }
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            // For each dir, resolve its origin URL.
            let mut dir_origin: Vec<(String, Option<String>)> = Vec::new();
            for (dname, dpath) in &entries {
                let origin = read_git_origin(dpath);
                dir_origin.push((dname.clone(), origin));
            }
            // Decisions in alphabetical-by-ref order satisfy D5's
            // "first scanned wins" when multiple dirs match one ref.
            #[derive(Clone)]
            struct MemberDecision {
                idx: usize,
                kind: String,
                dir: Option<String>,
            }
            let mut decisions: Vec<MemberDecision> = Vec::new();
            let mut consumed_dirs: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            let mut order: Vec<usize> = (0..members.len()).collect();
            order.sort_by(|&a, &b| {
                let ka = (
                    members[a].reference.org.as_str(),
                    members[a].reference.name.as_str(),
                );
                let kb = (
                    members[b].reference.org.as_str(),
                    members[b].reference.name.as_str(),
                );
                ka.cmp(&kb)
            });
            for &mi in &order {
                let m = &members[mi];
                let mut candidates: Vec<String> = Vec::new();
                for (dname, origin) in &dir_origin {
                    if consumed_dirs.contains(dname) {
                        continue;
                    }
                    let Some(url) = origin else { continue };
                    if m.remote_urls.iter().any(|u| u == url) {
                        candidates.push(dname.clone());
                    }
                }
                if candidates.is_empty() {
                    decisions.push(MemberDecision {
                        idx: mi,
                        kind: "removed".into(),
                        dir: None,
                    });
                    continue;
                }
                // Prefer declared dir if it's among candidates.
                let (winner, rest): (String, Vec<String>) =
                    if let Some(pos) =
                        candidates.iter().position(|d| *d == m.declared_dir)
                    {
                        let w = candidates.remove(pos);
                        (w, candidates)
                    } else {
                        let mut it = candidates.into_iter();
                        (it.next().unwrap(), it.collect())
                    };
                consumed_dirs.insert(winner.clone());
                let kind = if winner == m.declared_dir {
                    "matched"
                } else {
                    "renamed"
                };
                decisions.push(MemberDecision {
                    idx: mi,
                    kind: kind.into(),
                    dir: Some(winner),
                });
                for other in rest {
                    decisions.push(MemberDecision {
                        idx: mi,
                        kind: "ambiguous".into(),
                        dir: Some(other),
                    });
                }
            }
            // `new_matched`: leftover dirs whose origin matches a
            // known org repo that isn't a current member.
            let declared_keys: std::collections::BTreeSet<(String, String)> = members
                .iter()
                .map(|m| {
                    (
                        m.reference.org.as_str().to_string(),
                        m.reference.name.as_str().to_string(),
                    )
                })
                .collect();
            let mut new_matched: Vec<(String, crate::v5::config::RepoRef)> = Vec::new();
            for (dname, origin) in &dir_origin {
                if consumed_dirs.contains(dname) {
                    continue;
                }
                let Some(url) = origin else { continue };
                let Some(refs) = url_to_refs.get(url) else { continue };
                for (o, n) in refs {
                    if !declared_keys.contains(&(o.clone(), n.clone())) {
                        new_matched.push((
                            dname.clone(),
                            crate::v5::config::RepoRef {
                                org: o.clone().into(),
                                name: n.clone().into(),
                            },
                        ));
                        break;
                    }
                }
            }
            for d in &decisions {
                let reference = Some(members[d.idx].reference.clone());
                yield WorkspacesEvent::ReconcileEvent {
                    kind: d.kind.clone(),
                    reference,
                    dir: d.dir.clone(),
                    detail: None,
                };
            }
            for (dname, rref) in &new_matched {
                yield WorkspacesEvent::ReconcileEvent {
                    kind: "new_matched".into(),
                    reference: Some(rref.clone()),
                    dir: Some(dname.clone()),
                    detail: None,
                };
            }
            if !dry {
                let mut changed = false;
                let mut primary: std::collections::BTreeMap<usize, &MemberDecision> =
                    std::collections::BTreeMap::new();
                for d in &decisions {
                    if d.kind == "ambiguous" {
                        continue;
                    }
                    primary.entry(d.idx).or_insert(d);
                }
                let mut new_repos: Vec<WorkspaceRepo> = Vec::with_capacity(cfg.repos.len());
                for (mi, entry) in cfg.repos.iter().enumerate() {
                    let Some(d) = primary.get(&mi) else {
                        new_repos.push(entry.clone());
                        continue;
                    };
                    match d.kind.as_str() {
                        "matched" => new_repos.push(entry.clone()),
                        "renamed" => {
                            let reference = members[mi].reference.clone();
                            let dir = d.dir.clone().unwrap_or_default();
                            new_repos.push(WorkspaceRepo::Object { reference, dir });
                            changed = true;
                        }
                        "removed" => {
                            changed = true;
                        }
                        _ => new_repos.push(entry.clone()),
                    }
                }
                if changed {
                    cfg.repos = new_repos;
                    if let Err(e) = crate::v5::config::save_workspace(&ws_dir, &cfg) {
                        yield WorkspacesEvent::Error {
                            code: Some("io_error".into()),
                            message: e.to_string(),
                        };
                        return;
                    }
                }
            }
        }
    }
}

/// Read `origin` URL out of a dir's `.git/config` without shelling
/// out. Returns `None` if the dir is not a git working tree or the
/// config does not declare `remote "origin"`.
fn read_git_origin(dir: &std::path::Path) -> Option<String> {
    let git_dir = dir.join(".git");
    // `.git` may be a dir (classic) or a file containing `gitdir: <path>`
    // (worktree / submodule). Resolve it.
    let cfg_path = if git_dir.is_file() {
        let txt = std::fs::read_to_string(&git_dir).ok()?;
        let rest = txt.trim().strip_prefix("gitdir:")?.trim();
        std::path::PathBuf::from(rest).join("config")
    } else if git_dir.is_dir() {
        git_dir.join("config")
    } else {
        return None;
    };
    let raw = std::fs::read_to_string(&cfg_path).ok()?;
    // Minimal INI parse: look for `[remote "origin"]` section, then
    // its `url = …` entry until the next `[…]` header.
    let mut in_origin = false;
    for line in raw.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_origin = l == "[remote \"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some(rest) = l.strip_prefix("url") {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            return Some(rest.to_string());
        }
    }
    None
}
