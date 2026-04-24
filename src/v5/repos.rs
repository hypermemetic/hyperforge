//! `ReposHub` — v5 repos namespace (V5REPOS-2..14).
//!
//! Methods:
//! * `forge_port_schema` — wire-surfaced capability introspection (V5REPOS-2).
//! * `list`, `get`, `add`, `remove`, `add_remote`, `remove_remote`
//!   — CRUD over per-org YAML (V5REPOS-3..8).
//! * `sync`, `push` — metadata drift/push via `ForgePort` (V5REPOS-13, 14).
//!
//! Provider derivation (V5REPOS-12) runs on every call that resolves a
//! remote's provider — on the wire, every `Remote` event carries its
//! derived provider.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::v5::adapters::{
    extract_host, for_provider, DriftFieldKind, ForgeAuth, ForgeMetadata, ForgePortError,
    MetadataFields,
};
use crate::v5::config::{
    load_all, load_orgs, save_org, ConfigError, CredentialType, DomainName, OrgConfig, OrgName,
    OrgRepo, ProviderKind, Remote, RemoteUrl, RepoMetadataLocal, RepoName, RepoRef,
};
use crate::v5::secrets::{SecretResolver, YamlSecretStore};

// ---------------------------------------------------------------------
// Events.
// ---------------------------------------------------------------------

/// Event surface for the repos namespace. All events are flat
/// `snake_case` to match the harness's jq assertions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoEvent {
    /// Emitted by `forge_port_schema` (and harness capability probe).
    /// Names the exact four-field D3 intersection + the three
    /// lifecycle methods pinned by V5PROV-2 (D10).
    ForgePortSchema {
        fields: Vec<String>,
        methods: Vec<String>,
        error_classes: Vec<String>,
    },
    /// Capability alias emitted alongside `forge_port_schema` for
    /// harness discoverability; same payload.
    Capability {
        fields: Vec<String>,
        methods: Vec<String>,
        error_classes: Vec<String>,
    },
    /// One summary per repo (streamed by `list`).
    RepoSummary {
        org: String,
        name: String,
        remote_count: usize,
    },
    /// Full repo detail with derived remote providers.
    RepoDetail {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        remotes: Vec<RemoteWire>,
        /// Local metadata (echoed when declared); absent when no
        /// `metadata:` block on the repo entry.
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<RepoMetadataLocal>,
    },
    /// Acknowledgement of a removed repo.
    RepoRemoved { org: String, name: String },
    /// Per-remote forge metadata snapshot.
    ForgeMetadata {
        url: String,
        default_branch: String,
        description: String,
        archived: bool,
        visibility: String,
    },
    /// Drift report per remote.
    SyncDiff {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        url: String,
        status: String,
        drift: Vec<DriftField>,
        /// Present when `status == "errored"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        error_class: Option<String>,
        /// Snapshot of the four-field shape when the forge read
        /// succeeded. Callers reading a metadata event (V5REPOS-9/10/11
        /// AC1) can match on the `remote` field set.
        #[serde(skip_serializing_if = "Option::is_none")]
        remote: Option<ForgeMetadata>,
    },
    /// Per-remote push success.
    PushRemoteOk { url: String, fields: Vec<String> },
    /// Per-remote push failure. First failure aborts the remaining
    /// remotes per D4.
    PushRemoteError {
        url: String,
        error_class: String,
        message: String,
    },
    /// Final summary after a push run.
    PushSummary {
        succeeded: Vec<String>,
        errored: Vec<PushErrored>,
        aborted: bool,
    },
    /// Error event (typed). Always carries the emitting ticket's
    /// closed error-class set where applicable.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_class: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoRefWire {
    pub org: String,
    pub name: String,
}

impl From<&RepoRef> for RepoRefWire {
    fn from(r: &RepoRef) -> Self {
        Self {
            org: r.org.as_str().to_string(),
            name: r.name.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemoteWire {
    pub url: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DriftField {
    pub field: String,
    pub local: serde_json::Value,
    pub remote: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PushErrored {
    pub url: String,
    pub error_class: String,
    pub message: String,
}

// ---------------------------------------------------------------------
// Hub.
// ---------------------------------------------------------------------

/// Repos namespace. Methods attached here implement V5REPOS-{2..14}.
#[derive(Clone, Default)]
pub struct ReposHub {
    config_dir: PathBuf,
}

impl ReposHub {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            config_dir: PathBuf::new(),
        }
    }

    #[must_use]
    pub const fn with_config_dir(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}

// ---------------------------------------------------------------------
// Provider derivation (V5REPOS-12).
// ---------------------------------------------------------------------

pub(crate) fn derive_provider(
    remote: &Remote,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
) -> Result<ProviderKind, String> {
    if let Some(p) = remote.provider {
        return Ok(p);
    }
    let host = extract_host(remote.url.as_str())
        .ok_or_else(|| format!("cannot extract host from url '{}'", remote.url))?;
    provider_map
        .get(&DomainName::from(host.as_str()))
        .copied()
        .ok_or_else(|| {
            format!(
                "derivation failed for url '{}': host '{}' not in provider_map and no override",
                remote.url, host
            )
        })
}

fn remote_to_wire(
    remote: &Remote,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
) -> Result<RemoteWire, String> {
    let provider = derive_provider(remote, provider_map)?;
    Ok(RemoteWire {
        url: remote.url.as_str().to_string(),
        provider: match provider {
            ProviderKind::Github => "github",
            ProviderKind::Codeberg => "codeberg",
            ProviderKind::Gitlab => "gitlab",
        }
        .to_string(),
    })
}

// ---------------------------------------------------------------------
// Repo-entry lookup helpers.
// ---------------------------------------------------------------------

fn find_repo_mut<'a>(org: &'a mut OrgConfig, name: &str) -> Option<&'a mut OrgRepo> {
    org.repos.iter_mut().find(|r| r.name.as_str() == name)
}

fn find_repo<'a>(org: &'a OrgConfig, name: &str) -> Option<&'a OrgRepo> {
    org.repos.iter().find(|r| r.name.as_str() == name)
}

// ---------------------------------------------------------------------
// Param parsing helpers — synapse passes bools/structs as strings.
// ---------------------------------------------------------------------

fn to_bool(v: &Value, default: bool) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.to_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        Value::Null => default,
        _ => default,
    }
}

fn parse_remotes(raw: &Value) -> Result<Vec<Remote>, String> {
    let v = unwrap_json_string(raw);
    let arr = v
        .as_array()
        .ok_or_else(|| format!("remotes must be a JSON array, got: {v}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let r: Remote = serde_json::from_value(entry.clone())
            .map_err(|e| format!("invalid remote entry {entry}: {e}"))?;
        if r.url.as_str().is_empty() {
            return Err("remote url is empty".to_string());
        }
        out.push(r);
    }
    Ok(out)
}

fn parse_remote(raw: &Value) -> Result<Remote, String> {
    let v = unwrap_json_string(raw);
    let r: Remote = serde_json::from_value(v.clone())
        .map_err(|e| format!("invalid remote {v}: {e}"))?;
    if r.url.as_str().is_empty() {
        return Err("remote url is empty".to_string());
    }
    Ok(r)
}

fn parse_fields(raw: &Value) -> Result<MetadataFields, String> {
    let v = unwrap_json_string(raw);
    let map = v
        .as_object()
        .ok_or_else(|| format!("fields must be a JSON object, got: {v}"))?;
    let mut out = MetadataFields::new();
    for (k, val) in map {
        let kind = match k.as_str() {
            "default_branch" => DriftFieldKind::DefaultBranch,
            "description" => DriftFieldKind::Description,
            "archived" => DriftFieldKind::Archived,
            "visibility" => DriftFieldKind::Visibility,
            other => {
                return Err(format!(
                    "unsupported field '{other}'; allowed: default_branch, description, archived, visibility"
                ))
            }
        };
        out.insert(kind, val.clone());
    }
    Ok(out)
}

/// Synapse wraps structured params as `Value::String` of raw JSON.
/// Parse-if-string so callers receive the typed shape.
fn unwrap_json_string(raw: &Value) -> Value {
    if let Value::String(s) = raw {
        if let Ok(inner) = serde_json::from_str::<Value>(s) {
            return inner;
        }
    }
    raw.clone()
}

// ---------------------------------------------------------------------
// Error helpers.
// ---------------------------------------------------------------------

fn cfg_error_event(err: ConfigError) -> RepoEvent {
    RepoEvent::Error {
        code: Some("config_error".into()),
        error_class: None,
        message: err.to_string(),
    }
}

fn not_found_event(msg: impl Into<String>) -> RepoEvent {
    RepoEvent::Error {
        code: Some("not_found".into()),
        error_class: Some("not_found".into()),
        message: msg.into(),
    }
}

fn validation_event(msg: impl Into<String>) -> RepoEvent {
    RepoEvent::Error {
        code: Some("validation".into()),
        error_class: None,
        message: msg.into(),
    }
}

// ---------------------------------------------------------------------
// Activation.
// ---------------------------------------------------------------------

/// Repos CRUD + `ForgePort` surface.
#[plexus_macros::activation(
    namespace = "repos",
    description = "Repos CRUD + ForgePort",
    crate_path = "plexus_core"
)]
impl ReposHub {
    /// V5REPOS-2 / V5PROV-2 capability surface: announces the four D3
    /// fields, the five original error classes plus `conflict` and
    /// `unsupported_visibility`, and the seven trait method names
    /// (four metadata + three lifecycle).
    #[plexus_macros::method]
    pub async fn forge_port_schema(
        &self,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        stream! {
            let fields: Vec<String> = DriftFieldKind::all()
                .iter()
                .map(|k| k.as_str().to_string())
                .collect();
            let methods = vec![
                "create_repo".to_string(),
                "delete_repo".to_string(),
                "read_metadata".to_string(),
                "repo_exists".to_string(),
                "write_metadata".to_string(),
            ];
            let error_classes = vec![
                "auth".to_string(),
                "conflict".to_string(),
                "network".to_string(),
                "not_found".to_string(),
                "rate_limited".to_string(),
                "unsupported_field".to_string(),
                "unsupported_visibility".to_string(),
            ];
            yield RepoEvent::ForgePortSchema {
                fields: fields.clone(),
                methods: methods.clone(),
                error_classes: error_classes.clone(),
            };
            yield RepoEvent::Capability {
                fields,
                methods,
                error_classes,
            };
        }
    }

    /// V5REPOS-3: stream one `RepoSummary` per repo in the org.
    #[plexus_macros::method(params(org = "Org name"))]
    pub async fn list(&self, org: String) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            if org.is_empty() {
                yield validation_event("missing required parameter 'org'");
                return;
            }
            let orgs_dir = dir.join("orgs");
            let orgs = match load_orgs(&orgs_dir) {
                Ok(o) => o,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(org_cfg) = orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let mut entries: Vec<&OrgRepo> = org_cfg.repos.iter().collect();
            entries.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            for repo in entries {
                yield RepoEvent::RepoSummary {
                    org: org_cfg.name.as_str().to_string(),
                    name: repo.name.as_str().to_string(),
                    remote_count: repo.remotes.len(),
                };
            }
        }
    }

    /// V5REPOS-4: full `RepoDetail` including derived providers.
    #[plexus_macros::method(params(org = "Org name", name = "Repo name"))]
    pub async fn get(
        &self,
        org: String,
        name: String,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            if org.is_empty() {
                yield validation_event("missing required parameter 'org'");
                return;
            }
            if name.is_empty() {
                yield validation_event("missing required parameter 'name'");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(org_cfg) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let Some(repo) = find_repo(org_cfg, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            };
            match repo_detail_event(org, name, repo, &loaded.global.provider_map) {
                Ok(ev) => { yield ev; }
                Err(msg) => { yield validation_event(msg); }
            }
        }
    }

    /// V5REPOS-5: register a new repo with initial remotes.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        remotes = "JSON array of remotes",
        dry_run = "Preview without writing"
    ))]
    pub async fn add(
        &self,
        org: String,
        name: String,
        remotes: Value,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() {
                yield validation_event("missing required parameter 'org'");
                return;
            }
            if name.is_empty() {
                yield validation_event("missing required parameter 'name'");
                return;
            }
            let parsed_remotes = match parse_remotes(&remotes) {
                Ok(r) => r,
                Err(e) => { yield validation_event(e); return; }
            };
            if parsed_remotes.is_empty() {
                yield validation_event("remotes must contain at least one entry");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let provider_map = loaded.global.provider_map.clone();
            // Validate every remote's provider derives cleanly.
            for r in &parsed_remotes {
                if let Err(e) = derive_provider(r, &provider_map) {
                    yield validation_event(e);
                    return;
                }
            }
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            if existing.repos.iter().any(|r| r.name.as_str() == name) {
                yield validation_event(format!(
                    "repo '{name}' already exists under org '{org}'"
                ));
                return;
            }
            let mut updated = existing.clone();
            updated.repos.push(OrgRepo {
                name: RepoName::from(name.as_str()),
                remotes: parsed_remotes,
                metadata: None,
            });
            if !dry {
                let orgs_dir = dir.join("orgs");
                if let Err(e) = save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e);
                    return;
                }
            }
            let new_repo = updated.repos.last().unwrap();
            match repo_detail_event(org, name, new_repo, &provider_map) {
                Ok(ev) => yield ev,
                Err(msg) => yield validation_event(msg),
            }
        }
    }

    /// V5REPOS-6: drop the entry. `delete_remote=true` calls the
    /// adapter(s) first — any adapter failure aborts and leaves local.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        delete_remote = "Forge-side delete (default false)",
        dry_run = "Preview without writing"
    ))]
    pub async fn remove(
        &self,
        org: String,
        name: String,
        delete_remote: Option<Value>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            let forge_delete = delete_remote.as_ref().is_some_and(|v| to_bool(v, false));

            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            if find_repo(existing, &name).is_none() {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            }
            if forge_delete {
                // Resolve provider for every remote first; any derivation
                // failure aborts before any forge call.
                let provider_map = &loaded.global.provider_map;
                let repo = find_repo(existing, &name).unwrap();
                for r in &repo.remotes {
                    if let Err(e) = derive_provider(r, provider_map) {
                        yield validation_event(e);
                        return;
                    }
                }
                // Forge-side delete not implemented at the metadata
                // trait in v1 scope. Treat as adapter failure so the
                // local entry is preserved (per V5REPOS-6 AC4).
                yield RepoEvent::Error {
                    code: Some("adapter_error".into()),
                    error_class: Some("unsupported_field".into()),
                    message: "delete_remote=true requires forge-side delete; adapter capability not available in v1 ForgePort (local entry preserved)".to_string(),
                };
                return;
            }
            if !dry {
                let mut updated = existing.clone();
                updated.repos.retain(|r| r.name.as_str() != name);
                let orgs_dir = dir.join("orgs");
                if let Err(e) = save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e);
                    return;
                }
            }
            yield RepoEvent::RepoRemoved { org, name };
        }
    }

    /// V5REPOS-7: append a remote.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        remote = "JSON remote object",
        dry_run = "Preview without writing"
    ))]
    pub async fn add_remote(
        &self,
        org: String,
        name: String,
        remote: Value,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let new_remote = match parse_remote(&remote) {
                Ok(r) => r,
                Err(e) => { yield validation_event(e); return; }
            };
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let Some(repo) = find_repo(existing, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            };
            if repo.remotes.iter().any(|r| r.url == new_remote.url) {
                yield validation_event(format!("duplicate remote url '{}'", new_remote.url));
                return;
            }
            let provider_map = loaded.global.provider_map.clone();
            if let Err(e) = derive_provider(&new_remote, &provider_map) {
                yield validation_event(e);
                return;
            }
            let mut updated = existing.clone();
            if let Some(repo_mut) = find_repo_mut(&mut updated, &name) {
                repo_mut.remotes.push(new_remote);
            }
            if !dry {
                let orgs_dir = dir.join("orgs");
                if let Err(e) = save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e);
                    return;
                }
            }
            let repo_after = find_repo(&updated, &name).unwrap();
            match repo_detail_event(org, name, repo_after, &provider_map) {
                Ok(ev) => yield ev,
                Err(msg) => yield validation_event(msg),
            }
        }
    }

    /// V5REPOS-8: drop a remote by URL.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        url = "Remote URL to drop",
        dry_run = "Preview without writing"
    ))]
    pub async fn remove_remote(
        &self,
        org: String,
        name: String,
        url: String,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() || url.is_empty() {
                yield validation_event("missing required parameter 'org', 'name', or 'url'");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let Some(repo) = find_repo(existing, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            };
            if !repo.remotes.iter().any(|r| r.url.as_str() == url) {
                yield not_found_event(format!("remote url '{url}' not present on repo"));
                return;
            }
            if repo.remotes.len() == 1 {
                yield validation_event(format!(
                    "cannot remove last remote from repo '{name}'; use repos.remove to drop the entry"
                ));
                return;
            }
            let mut updated = existing.clone();
            if let Some(repo_mut) = find_repo_mut(&mut updated, &name) {
                if let Some(pos) = repo_mut.remotes.iter().position(|r| r.url.as_str() == url) {
                    repo_mut.remotes.remove(pos);
                }
            }
            if !dry {
                let orgs_dir = dir.join("orgs");
                if let Err(e) = save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e);
                    return;
                }
            }
            let provider_map = loaded.global.provider_map;
            let repo_after = find_repo(&updated, &name).unwrap();
            match repo_detail_event(org, name, repo_after, &provider_map) {
                Ok(ev) => yield ev,
                Err(msg) => yield validation_event(msg),
            }
        }
    }

    /// V5REPOS-13: read remote metadata, emit one `SyncDiff` per remote.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        remote = "Optional remote URL to limit scope"
    ))]
    pub async fn sync(
        &self,
        org: String,
        name: String,
        remote: Option<String>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(org_cfg) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let Some(repo) = find_repo(org_cfg, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            };
            let filtered: Vec<&Remote> = if let Some(filter_url) = remote.as_ref().filter(|s| !s.is_empty()) {
                let matches: Vec<&Remote> = repo.remotes.iter().filter(|r| r.url.as_str() == filter_url).collect();
                if matches.is_empty() {
                    yield not_found_event(format!("remote url '{filter_url}' not present on repo"));
                    return;
                }
                matches
            } else {
                repo.remotes.iter().collect()
            };
            let resolver = YamlSecretStore::new(&dir);
            let token_ref = org_cfg
                .forge
                .credentials
                .iter()
                .find(|c| matches!(c.cred_type, CredentialType::Token))
                .map(|c| c.key.clone());
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };
            let local = repo.metadata.clone();
            for r in filtered {
                let provider = match derive_provider(r, &loaded.global.provider_map) {
                    Ok(p) => p,
                    Err(e) => {
                        yield RepoEvent::SyncDiff {
                            reference: (&repo_ref).into(),
                            url: r.url.as_str().to_string(),
                            status: "errored".into(),
                            drift: vec![],
                            error_class: Some("network".into()),
                            remote: None,
                        };
                        yield validation_event(e);
                        continue;
                    }
                };
                let adapter = for_provider(provider);
                let auth = ForgeAuth {
                    token_ref: token_ref.as_deref(),
                    resolver: &resolver,
                };
                match adapter.read_metadata(r, &repo_ref, &auth).await {
                    Ok(meta) => {
                        let drift = compute_drift(&local, &meta);
                        let status = if drift.is_empty() { "in_sync" } else { "drifted" };
                        yield RepoEvent::SyncDiff {
                            reference: (&repo_ref).into(),
                            url: r.url.as_str().to_string(),
                            status: status.into(),
                            drift,
                            error_class: None,
                            remote: Some(meta),
                        };
                    }
                    Err(e) => {
                        yield RepoEvent::SyncDiff {
                            reference: (&repo_ref).into(),
                            url: r.url.as_str().to_string(),
                            status: "errored".into(),
                            drift: vec![],
                            error_class: Some(e.class.as_str().to_string()),
                            remote: None,
                        };
                    }
                }
            }
        }
    }

    /// V5REPOS-14: sequential per-remote metadata write per D4.
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        remote = "Optional single-remote scope",
        fields = "Optional JSON field override; defaults to local metadata",
        dry_run = "Preview without forge writes"
    ))]
    pub async fn push(
        &self,
        org: String,
        name: String,
        remote: Option<String>,
        fields: Option<Value>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match load_all(&dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(org_cfg) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            let Some(repo) = find_repo(org_cfg, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'"));
                return;
            };
            let to_apply = match fields {
                Some(v) => match parse_fields(&v) {
                    Ok(m) => m,
                    Err(e) => { yield validation_event(e); return; }
                },
                None => metadata_from_local(&repo.metadata),
            };
            if to_apply.is_empty() {
                yield validation_event("no fields to push; supply `fields` or declare repo.metadata locally");
                return;
            }
            let remotes: Vec<&Remote> = if let Some(filter_url) = remote.as_ref().filter(|s| !s.is_empty()) {
                let matched: Vec<&Remote> = repo.remotes.iter().filter(|r| r.url.as_str() == filter_url).collect();
                if matched.is_empty() {
                    yield not_found_event(format!("remote url '{filter_url}' not present on repo"));
                    return;
                }
                matched
            } else {
                repo.remotes.iter().collect()
            };
            let resolver = YamlSecretStore::new(&dir);
            let token_ref = org_cfg
                .forge
                .credentials
                .iter()
                .find(|c| matches!(c.cred_type, CredentialType::Token))
                .map(|c| c.key.clone());
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };

            let mut succeeded: Vec<String> = Vec::new();
            let mut errored: Vec<PushErrored> = Vec::new();
            let mut aborted = false;

            for r in remotes {
                let url_s = r.url.as_str().to_string();
                let provider = match derive_provider(r, &loaded.global.provider_map) {
                    Ok(p) => p,
                    Err(e) => {
                        let ev = PushErrored {
                            url: url_s.clone(),
                            error_class: "network".into(),
                            message: e.clone(),
                        };
                        yield RepoEvent::PushRemoteError {
                            url: url_s.clone(),
                            error_class: ev.error_class.clone(),
                            message: ev.message.clone(),
                        };
                        errored.push(ev);
                        aborted = true;
                        break;
                    }
                };
                if dry {
                    let names: Vec<String> = to_apply
                        .keys()
                        .map(|k| k.as_str().to_string())
                        .collect();
                    yield RepoEvent::PushRemoteOk { url: url_s.clone(), fields: names };
                    succeeded.push(url_s);
                    continue;
                }
                let adapter = for_provider(provider);
                let auth = ForgeAuth {
                    token_ref: token_ref.as_deref(),
                    resolver: &resolver,
                };
                match adapter
                    .write_metadata(r, &repo_ref, &to_apply, &auth)
                    .await
                {
                    Ok(applied) => {
                        let names: Vec<String> = applied
                            .keys()
                            .map(|k| k.as_str().to_string())
                            .collect();
                        yield RepoEvent::PushRemoteOk { url: url_s.clone(), fields: names };
                        succeeded.push(url_s);
                    }
                    Err(e) => {
                        let class = e.class.as_str().to_string();
                        let message = e.message.clone();
                        yield RepoEvent::PushRemoteError {
                            url: url_s.clone(),
                            error_class: class.clone(),
                            message: message.clone(),
                        };
                        errored.push(PushErrored { url: url_s, error_class: class, message });
                        aborted = true;
                        break;
                    }
                }
            }
            yield RepoEvent::PushSummary { succeeded, errored, aborted };
        }
    }
}

// ---------------------------------------------------------------------
// Helpers internal to the methods above.
// ---------------------------------------------------------------------

fn repo_detail_event(
    org: String,
    name: String,
    repo: &OrgRepo,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
) -> Result<RepoEvent, String> {
    let remotes: Result<Vec<RemoteWire>, String> = repo
        .remotes
        .iter()
        .map(|r| remote_to_wire(r, provider_map))
        .collect();
    let remotes = remotes?;
    Ok(RepoEvent::RepoDetail {
        reference: RepoRefWire { org, name },
        remotes,
        metadata: repo.metadata.clone(),
    })
}

fn metadata_from_local(local: &Option<RepoMetadataLocal>) -> MetadataFields {
    let mut out = MetadataFields::new();
    if let Some(m) = local {
        if let Some(v) = &m.default_branch {
            out.insert(DriftFieldKind::DefaultBranch, Value::String(v.clone()));
        }
        if let Some(v) = &m.description {
            out.insert(DriftFieldKind::Description, Value::String(v.clone()));
        }
        if let Some(v) = m.archived {
            out.insert(DriftFieldKind::Archived, Value::Bool(v));
        }
        if let Some(v) = &m.visibility {
            out.insert(DriftFieldKind::Visibility, Value::String(v.clone()));
        }
    }
    out
}

pub(crate) fn compute_drift(local: &Option<RepoMetadataLocal>, remote: &ForgeMetadata) -> Vec<DriftField> {
    let Some(local) = local else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(v) = &local.default_branch {
        if v != &remote.default_branch {
            out.push(DriftField {
                field: "default_branch".into(),
                local: Value::String(v.clone()),
                remote: Value::String(remote.default_branch.clone()),
            });
        }
    }
    if let Some(v) = &local.description {
        if v != &remote.description {
            out.push(DriftField {
                field: "description".into(),
                local: Value::String(v.clone()),
                remote: Value::String(remote.description.clone()),
            });
        }
    }
    if let Some(v) = local.archived {
        if v != remote.archived {
            out.push(DriftField {
                field: "archived".into(),
                local: Value::Bool(v),
                remote: Value::Bool(remote.archived),
            });
        }
    }
    if let Some(v) = &local.visibility {
        if v != &remote.visibility {
            out.push(DriftField {
                field: "visibility".into(),
                local: Value::String(v.clone()),
                remote: Value::String(remote.visibility.clone()),
            });
        }
    }
    out
}

// Silence unused-import lint if adapters are only used indirectly.
#[allow(dead_code)]
struct _KeepLinkedTypes(Arc<dyn SecretResolver>, ForgePortError, RemoteUrl);
