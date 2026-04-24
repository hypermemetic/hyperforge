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
    DriftFieldKind, ForgeMetadata, ForgePortError,
    MetadataFields, ProviderVisibility,
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
    /// Acknowledgement of an added repo (V5PROV-6). Emitted after
    /// the local entry is written (and, when `create_remote=true`,
    /// after `repo_created`).
    RepoAdded {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        remotes: Vec<RemoteWire>,
    },
    /// Emitted by `repos.add --create_remote true` on successful
    /// `adapter.create_repo` (V5PROV-6). `url` is the first remote.
    RepoCreated {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        url: String,
    },
    /// Emitted by `repos.delete` (V5PROV-7) after the local entry is
    /// dropped. Distinct from `RepoRemoved` (V5REPOS-6) — both mean
    /// local success, but `repos.delete` is the V5PROV-flow method
    /// and callers match on this type.
    RepoDeleted {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
    },
    /// Emitted by `repos.delete --delete_remote true` on successful
    /// `adapter.delete_repo` (V5PROV-7). `url` is the first remote.
    RemoteDeleted {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        url: String,
    },
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
    // V5LIFECYCLE-6/7/8/9 events -----------------------------------------
    /// Emitted by `repos.delete` per-provider when privatization succeeds.
    ForgePrivatized {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        provider: String,
        url: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
    },
    /// Emitted by `repos.delete` per-provider when privatization fails.
    PrivatizeError {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        provider: String,
        error_class: String,
        message: String,
    },
    /// Emitted at the end of a successful `repos.delete` flow.
    RepoDismissed {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        privatized_on: Vec<String>,
        already: bool,
    },
    /// Emitted by `repos.purge` per-provider when forge delete succeeds.
    ForgeDeleted {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        provider: String,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Emitted by `repos.purge` per-provider on forge error.
    PurgeError {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        provider: String,
        error_class: String,
        message: String,
    },
    /// Emitted at the end of a successful `repos.purge`.
    RepoPurged {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
    },
    /// Emitted by `repos.protect`.
    RepoProtectionSet {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        protected: bool,
    },
    /// Emitted by `repos.init`.
    HyperforgeConfigWritten {
        path: String,
        repo_name: String,
        org: String,
    },
    /// Emitted by `repos.import` per repo that was registered into the
    /// org yaml.
    RepoImported {
        #[serde(rename = "ref")]
        reference: RepoRefWire,
        url: String,
    },
    /// Emitted at the end of `repos.import`.
    ImportSummary {
        org: String,
        total: u32,
        added: u32,
        skipped: u32,
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

// V5LIFECYCLE-3: relocated to `crate::v5::ops::repo::derive_provider`.
// Re-exported here so existing callsites in this module and
// `workspaces.rs` keep their short name without reintroducing a
// duplicate implementation.
pub(crate) use crate::v5::ops::repo::derive_provider;

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

    /// V5REPOS-5 + V5PROV-6: register a new repo with initial remotes.
    ///
    /// When `create_remote=true` is set, the adapter's `create_repo`
    /// is called after the local entry is written. The pinned order
    /// (per V5PROV-1 R2): validate → write local → call `repo_exists`
    /// (conflict if present) → call `create_repo` (on failure, roll
    /// back local entry) → emit `repo_created` + `repo_added`.
    ///
    /// When `create_remote=false` (default), the method is backward
    /// compatible with V5REPOS-5 and emits `repo_detail` + `repo_added`
    /// after writing the local entry (no forge contact).
    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        remotes = "JSON array of remotes",
        create_remote = "Also create the repo on the remote forge (default false)",
        visibility = "Visibility for `create_remote`: public | private | internal (default private)",
        description = "Description passed to `create_remote` (default empty)",
        dry_run = "Preview without writing"
    ))]
    pub async fn add(
        &self,
        org: String,
        name: String,
        remotes: Value,
        create_remote: Option<Value>,
        visibility: Option<String>,
        description: Option<String>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir: Result<PathBuf, String> = Ok(self.config_dir.clone());
        stream! {
            let dir = match config_dir {
                Ok(d) => d,
                Err(e) => { yield RepoEvent::Error { code: Some("config_error".into()), error_class: None, message: e }; return; }
            };
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            let forge_create = create_remote.as_ref().is_some_and(|v| to_bool(v, false));
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
            // Parse visibility. On `create_remote=false` the value is
            // still parsed for validation — a garbage input fails
            // fast rather than being silently ignored.
            let vis_raw = visibility
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("private");
            let vis = match ProviderVisibility::parse(vis_raw) {
                Ok(v) => v,
                Err(e) => { yield validation_event(e); return; }
            };
            let desc = description.unwrap_or_default();

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
                remotes: parsed_remotes.clone(),
                metadata: None,
            });
            let orgs_dir = dir.join("orgs");
            if !dry {
                if let Err(e) = save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e);
                    return;
                }
            }
            // ---------- create_remote flow ----------
            if forge_create {
                let first = &parsed_remotes[0];
                let provider = match derive_provider(first, &provider_map) {
                    Ok(p) => p,
                    Err(e) => {
                        // Unreachable: we already validated above, but
                        // defend defensively + roll back.
                        if !dry {
                            let rolled_back = existing.clone();
                            let _ = save_org(&orgs_dir, &rolled_back);
                        }
                        yield validation_event(e);
                        return;
                    }
                };
                let repo_ref = RepoRef {
                    org: OrgName::from(org.as_str()),
                    name: RepoName::from(name.as_str()),
                };
                let repo_ref_wire = RepoRefWire::from(&repo_ref);
                let url_s = first.url.as_str().to_string();

                if dry {
                    // Dry run emits the success event stream without
                    // any forge or disk contact.
                    yield RepoEvent::RepoCreated {
                        reference: repo_ref_wire.clone(),
                        url: url_s,
                    };
                    match repo_detail_event(
                        org.clone(),
                        name.clone(),
                        updated.repos.last().unwrap(),
                        &provider_map,
                    ) {
                        Ok(ev) => yield ev,
                        Err(msg) => { yield validation_event(msg); return; }
                    }
                    // And the RepoAdded ack.
                    let wires: Result<Vec<RemoteWire>, String> = parsed_remotes
                        .iter()
                        .map(|r| remote_to_wire(r, &provider_map))
                        .collect();
                    match wires {
                        Ok(ws) => yield RepoEvent::RepoAdded {
                            reference: repo_ref_wire,
                            remotes: ws,
                        },
                        Err(msg) => yield validation_event(msg),
                    }
                    return;
                }

                // V5LIFECYCLE-4: route through ops::repo wrappers.
                let resolver = YamlSecretStore::new(&dir);
                let token_ref = crate::v5::ops::repo::token_ref_for(existing);
                let _ = provider; // provider is still derived for logging but we no longer need the adapter handle here
                match crate::v5::ops::repo::exists_on_forge(
                    first, &repo_ref, &loaded.global.provider_map, &resolver, token_ref,
                ).await {
                    Ok(true) => {
                        let rolled_back = existing.clone();
                        if let Err(e) = save_org(&orgs_dir, &rolled_back) {
                            yield cfg_error_event(e);
                        }
                        yield RepoEvent::Error {
                            code: Some("conflict".into()),
                            error_class: Some("conflict".into()),
                            message: format!("repo '{}/{}' already exists on remote", org, name),
                        };
                        return;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let rolled_back = existing.clone();
                        if let Err(save_err) = save_org(&orgs_dir, &rolled_back) {
                            yield cfg_error_event(save_err);
                        }
                        yield RepoEvent::Error {
                            code: Some(e.class.as_str().into()),
                            error_class: Some(e.class.as_str().into()),
                            message: format!("repo_exists probe failed: {}", e.message),
                        };
                        return;
                    }
                }
                match crate::v5::ops::repo::create_on_forge(
                    first, &repo_ref, vis, &desc, &loaded.global.provider_map, &resolver, token_ref,
                ).await {
                    Ok(()) => {
                        yield RepoEvent::RepoCreated {
                            reference: repo_ref_wire.clone(),
                            url: url_s,
                        };
                    }
                    Err(e) => {
                        // Roll back local write on forge error.
                        let rolled_back = existing.clone();
                        if let Err(save_err) = save_org(&orgs_dir, &rolled_back) {
                            yield cfg_error_event(save_err);
                        }
                        yield RepoEvent::Error {
                            code: Some(e.class.as_str().into()),
                            error_class: Some(e.class.as_str().into()),
                            message: e.message,
                        };
                        return;
                    }
                }
            }

            // Success: emit RepoDetail (V5REPOS-5 backward compat) +
            // RepoAdded (V5PROV-6 ack).
            let new_repo = updated.repos.last().unwrap();
            let repo_ref_wire = RepoRefWire {
                org: org.clone(),
                name: name.clone(),
            };
            let wires: Result<Vec<RemoteWire>, String> = parsed_remotes
                .iter()
                .map(|r| remote_to_wire(r, &provider_map))
                .collect();
            match repo_detail_event(org, name, new_repo, &provider_map) {
                Ok(ev) => yield ev,
                Err(msg) => { yield validation_event(msg); return; }
            }
            match wires {
                Ok(ws) => yield RepoEvent::RepoAdded {
                    reference: repo_ref_wire,
                    remotes: ws,
                },
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
            // Remote filter validation (ops::repo::sync_one handles
            // the filter internally; we only validate-for-error here).
            if let Some(filter_url) = remote.as_ref().filter(|s| !s.is_empty()) {
                if !repo.remotes.iter().any(|r| r.url.as_str() == filter_url) {
                    yield not_found_event(format!("remote url '{filter_url}' not present on repo"));
                    return;
                }
            }
            let resolver = YamlSecretStore::new(&dir);
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };
            // V5LIFECYCLE-3: delegate to the single sync primitive.
            let outcomes = crate::v5::ops::repo::sync_one(
                repo,
                org_cfg,
                &loaded.global.provider_map,
                &resolver,
                remote.as_deref(),
            ).await;
            for o in outcomes {
                // Translate per-remote outcome into the RepoEvent::SyncDiff
                // wire shape (per-remote event for `repos.sync` per V5REPOS-13).
                yield RepoEvent::SyncDiff {
                    reference: (&repo_ref).into(),
                    url: o.remote.url.as_str().to_string(),
                    status: o.status.as_str().to_string(),
                    drift: o.drift.into_iter().map(|d| DriftField {
                        field: d.field,
                        local: d.local,
                        remote: d.remote,
                    }).collect(),
                    error_class: o.error_class.map(|e| e.as_str().to_string()),
                    remote: o.metadata,
                };
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
                // V5LIFECYCLE-4: write via ops::repo helper.
                let _ = provider; // logging placeholder; provider is re-derived inside the helper
                match crate::v5::ops::repo::write_metadata_on_forge(
                    r, &repo_ref, &to_apply, &loaded.global.provider_map, &resolver, token_ref.as_deref(),
                ).await {
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

    // ==================================================================
    // V5LIFECYCLE-6: repos.delete — soft (privatize + mark dismissed).
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        dry_run = "Preview without writing"
    ))]
    pub async fn delete(
        &self,
        org: String,
        name: String,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match crate::v5::ops::state::load_all(&config_dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found")); return;
            };
            let Some(repo) = crate::v5::ops::state::find_repo(existing, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'")); return;
            };
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };
            let wire = RepoRefWire::from(&repo_ref);
            // Protection guard.
            if repo.metadata.as_ref().and_then(|m| m.protected).unwrap_or(false) {
                yield RepoEvent::Error {
                    code: Some("protected".into()),
                    error_class: None,
                    message: format!("repo '{name}' is protected; toggle via repos.protect first"),
                };
                return;
            }
            // Already-dismissed idempotency.
            let already = repo.metadata.as_ref().and_then(|m| m.lifecycle)
                == Some(crate::v5::config::RepoLifecycle::Dismissed);
            if already {
                let prev: Vec<String> = repo.metadata.as_ref()
                    .map(|m| m.privatized_on.iter().map(|p| match p {
                        ProviderKind::Github => "github".to_string(),
                        ProviderKind::Codeberg => "codeberg".to_string(),
                        ProviderKind::Gitlab => "gitlab".to_string(),
                    }).collect())
                    .unwrap_or_default();
                yield RepoEvent::RepoDismissed { reference: wire, privatized_on: prev, already: true };
                return;
            }
            // Privatize on every remote.
            let resolver = YamlSecretStore::new(&config_dir);
            let token_ref = crate::v5::ops::repo::token_ref_for(existing);
            let mut privatized: std::collections::BTreeSet<ProviderKind> = std::collections::BTreeSet::new();
            for r in &repo.remotes {
                let provider = match crate::v5::ops::repo::derive_provider(r, &loaded.global.provider_map) {
                    Ok(p) => p,
                    Err(e) => { yield validation_event(e); continue; }
                };
                let provider_s = match provider {
                    ProviderKind::Github => "github".to_string(),
                    ProviderKind::Codeberg => "codeberg".to_string(),
                    ProviderKind::Gitlab => "gitlab".to_string(),
                };
                let url_s = r.url.as_str().to_string();
                if dry {
                    yield RepoEvent::ForgePrivatized { reference: wire.clone(), provider: provider_s.clone(), url: url_s, dry_run: true };
                    privatized.insert(provider);
                    continue;
                }
                match crate::v5::ops::repo::privatize_on_forge(r, &repo_ref, &loaded.global.provider_map, &resolver, token_ref).await {
                    Ok(()) => {
                        privatized.insert(provider);
                        yield RepoEvent::ForgePrivatized { reference: wire.clone(), provider: provider_s, url: url_s, dry_run: false };
                    }
                    Err(e) => {
                        yield RepoEvent::PrivatizeError {
                            reference: wire.clone(),
                            provider: provider_s,
                            error_class: e.class.as_str().to_string(),
                            message: e.message,
                        };
                    }
                }
            }
            let priv_list: Vec<String> = privatized.iter().map(|p| match p {
                ProviderKind::Github => "github".to_string(),
                ProviderKind::Codeberg => "codeberg".to_string(),
                ProviderKind::Gitlab => "gitlab".to_string(),
            }).collect();
            if !dry {
                let mut updated = existing.clone();
                if let Some(mr) = crate::v5::ops::state::find_repo_mut(&mut updated, &name) {
                    crate::v5::ops::repo::dismiss(mr, privatized);
                }
                let orgs_dir = config_dir.join("orgs");
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e); return;
                }
            }
            yield RepoEvent::RepoDismissed { reference: wire, privatized_on: priv_list, already: false };
        }
    }

    // ==================================================================
    // V5LIFECYCLE-7: repos.purge — hard-delete, gated on dismissed.
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        dry_run = "Preview without writing"
    ))]
    pub async fn purge(
        &self,
        org: String,
        name: String,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match crate::v5::ops::state::load_all(&config_dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found")); return;
            };
            let Some(repo) = crate::v5::ops::state::find_repo(existing, &name) else {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'")); return;
            };
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };
            let wire = RepoRefWire::from(&repo_ref);
            if repo.metadata.as_ref().and_then(|m| m.protected).unwrap_or(false) {
                yield RepoEvent::Error {
                    code: Some("protected".into()),
                    error_class: None,
                    message: format!("repo '{name}' is protected"),
                };
                return;
            }
            if repo.metadata.as_ref().and_then(|m| m.lifecycle) != Some(crate::v5::config::RepoLifecycle::Dismissed) {
                yield RepoEvent::Error {
                    code: Some("not_dismissed".into()),
                    error_class: None,
                    message: "purge requires lifecycle: dismissed; run repos.delete first".into(),
                };
                return;
            }
            // Forge-delete every remote.
            let resolver = YamlSecretStore::new(&config_dir);
            let token_ref = crate::v5::ops::repo::token_ref_for(existing);
            for r in &repo.remotes {
                let provider = match crate::v5::ops::repo::derive_provider(r, &loaded.global.provider_map) {
                    Ok(p) => p,
                    Err(e) => { yield validation_event(e); continue; }
                };
                let provider_s = match provider {
                    ProviderKind::Github => "github".to_string(),
                    ProviderKind::Codeberg => "codeberg".to_string(),
                    ProviderKind::Gitlab => "gitlab".to_string(),
                };
                let url_s = r.url.as_str().to_string();
                if dry {
                    yield RepoEvent::ForgeDeleted { reference: wire.clone(), provider: provider_s, url: url_s, note: Some("dry_run".into()) };
                    continue;
                }
                match crate::v5::ops::repo::delete_on_forge(r, &repo_ref, &loaded.global.provider_map, &resolver, token_ref).await {
                    Ok(()) => yield RepoEvent::ForgeDeleted { reference: wire.clone(), provider: provider_s, url: url_s, note: None },
                    Err(e) if matches!(e.class, crate::v5::adapters::ForgeErrorClass::NotFound) => {
                        yield RepoEvent::ForgeDeleted { reference: wire.clone(), provider: provider_s, url: url_s, note: Some("already gone".into()) };
                    }
                    Err(e) => {
                        yield RepoEvent::PurgeError {
                            reference: wire.clone(),
                            provider: provider_s,
                            error_class: e.class.as_str().to_string(),
                            message: e.message,
                        };
                    }
                }
            }
            if !dry {
                let mut updated = existing.clone();
                let _ = crate::v5::ops::repo::purge(&mut updated, &RepoName::from(name.as_str()));
                let orgs_dir = config_dir.join("orgs");
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e); return;
                }
            }
            yield RepoEvent::RepoPurged { reference: wire };
        }
    }

    // ==================================================================
    // V5LIFECYCLE-8: repos.protect — toggle protection bit.
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        protected = "Target state",
        dry_run = "Preview without writing"
    ))]
    pub async fn protect(
        &self,
        org: String,
        name: String,
        protected: Option<Value>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            let target = protected.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() || name.is_empty() {
                yield validation_event("missing required parameter 'org' or 'name'");
                return;
            }
            let loaded = match crate::v5::ops::state::load_all(&config_dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(existing) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found")); return;
            };
            if crate::v5::ops::state::find_repo(existing, &name).is_none() {
                yield not_found_event(format!("repo '{name}' not found under org '{org}'")); return;
            }
            let repo_ref = RepoRef { org: OrgName::from(org.as_str()), name: RepoName::from(name.as_str()) };
            let wire = RepoRefWire::from(&repo_ref);
            if !dry {
                let mut updated = existing.clone();
                if let Some(mr) = crate::v5::ops::state::find_repo_mut(&mut updated, &name) {
                    let md = mr.metadata.get_or_insert_with(RepoMetadataLocal::default);
                    md.protected = if target { Some(true) } else { None };
                }
                let orgs_dir = config_dir.join("orgs");
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e); return;
                }
            }
            yield RepoEvent::RepoProtectionSet { reference: wire, protected: target };
        }
    }

    // ==================================================================
    // V5LIFECYCLE-9: repos.init — write .hyperforge/config.toml.
    // ==================================================================

    #[plexus_macros::method(params(
        target_path = "Repo checkout directory (note: named target_path to avoid synapse's path-autoexpansion)",
        org = "Owning org",
        repo_name = "Repo identifier",
        forges = "JSON array of provider names",
        default_branch = "Default branch (defaults to main)",
        visibility = "private|public|internal (default private)",
        description = "Free-text description",
        force = "Overwrite existing .hyperforge/config.toml",
        dry_run = "Preview without writing"
    ))]
    pub async fn init(
        &self,
        target_path: String,
        org: String,
        repo_name: String,
        forges: Option<Value>,
        default_branch: Option<String>,
        visibility: Option<String>,
        description: Option<String>,
        force: Option<Value>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            let force_b = force.as_ref().is_some_and(|v| to_bool(v, false));
            if target_path.is_empty() || org.is_empty() || repo_name.is_empty() {
                yield validation_event("missing required parameter 'target_path', 'org', or 'repo_name'");
                return;
            }
            let forges_list: Vec<ProviderKind> = match forges.as_ref() {
                None => vec![ProviderKind::Github],
                Some(v) => {
                    let arr = if let Some(s) = v.as_str() {
                        serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                    } else if let Some(a) = v.as_array() {
                        a.iter().filter_map(|e| e.as_str().map(String::from)).collect()
                    } else { vec![] };
                    arr.into_iter().filter_map(|s| match s.as_str() {
                        "github" => Some(ProviderKind::Github),
                        "codeberg" => Some(ProviderKind::Codeberg),
                        "gitlab" => Some(ProviderKind::Gitlab),
                        _ => None,
                    }).collect()
                }
            };
            let cfg = crate::v5::ops::fs::HyperforgeRepoConfig {
                repo_name: repo_name.clone(),
                org: OrgName::from(org.as_str()),
                forges: forges_list,
                default_branch: default_branch.or_else(|| Some("main".into())),
                visibility,
                description,
            };
            let path = std::path::PathBuf::from(&target_path);
            if dry {
                yield RepoEvent::HyperforgeConfigWritten {
                    path: path.join(".hyperforge").join("config.toml").display().to_string(),
                    repo_name: repo_name.clone(),
                    org: org.clone(),
                };
                return;
            }
            match crate::v5::ops::fs::write_hyperforge_config(&path, &cfg, force_b) {
                Ok(written_path) => {
                    yield RepoEvent::HyperforgeConfigWritten {
                        path: written_path.display().to_string(),
                        repo_name,
                        org,
                    };
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        code: Some(e.code().into()),
                        error_class: None,
                        message: e.to_string(),
                    };
                }
            }
        }
    }

    // ==================================================================
    // V5PARITY-2: repos.import — walk a forge and register missing repos.
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        forge = "Optional provider filter (github|codeberg|gitlab); default = org's declared forge",
        dry_run = "Preview without writing"
    ))]
    pub async fn import(
        &self,
        org: String,
        forge: Option<String>,
        dry_run: Option<Value>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| to_bool(v, false));
            if org.is_empty() {
                yield validation_event("missing required parameter 'org'");
                return;
            }
            let loaded = match crate::v5::ops::state::load_all(&config_dir) {
                Ok(l) => l,
                Err(e) => { yield cfg_error_event(e); return; }
            };
            let Some(org_cfg) = loaded.orgs.get(&OrgName::from(org.as_str())) else {
                yield not_found_event(format!("org '{org}' not found"));
                return;
            };
            // Pick the provider: explicit --forge wins; otherwise the org's declared forge.provider.
            let provider = if let Some(f) = forge.as_deref().filter(|s| !s.is_empty()) {
                match f {
                    "github" => ProviderKind::Github,
                    "codeberg" => ProviderKind::Codeberg,
                    "gitlab" => ProviderKind::Gitlab,
                    other => { yield validation_event(format!("unknown provider: {other}")); return; }
                }
            } else {
                org_cfg.forge.provider
            };
            let resolver = YamlSecretStore::new(&config_dir);
            let token_ref = crate::v5::ops::repo::token_ref_for(org_cfg);
            let remote_repos = match crate::v5::ops::repo::list_on_forge(
                provider, &OrgName::from(org.as_str()), &resolver, token_ref,
            ).await {
                Ok(v) => v,
                Err(e) => {
                    yield RepoEvent::Error {
                        code: Some(e.class.as_str().into()),
                        error_class: Some(e.class.as_str().into()),
                        message: e.message,
                    };
                    return;
                }
            };
            let total = u32::try_from(remote_repos.len()).unwrap_or(u32::MAX);
            let mut added: u32 = 0;
            let mut skipped: u32 = 0;
            // Clone the org for mutation.
            let mut updated = org_cfg.clone();
            for rr in &remote_repos {
                // Skip if already registered.
                let already = updated.repos.iter().any(|r| r.name.as_str() == rr.name);
                if already { skipped += 1; continue; }
                // Append.
                let new_repo = crate::v5::config::OrgRepo {
                    name: RepoName::from(rr.name.as_str()),
                    remotes: vec![crate::v5::config::Remote {
                        url: crate::v5::config::RemoteUrl::from(rr.url.as_str()),
                        provider: None,
                    }],
                    metadata: None,
                };
                updated.repos.push(new_repo);
                added += 1;
                yield RepoEvent::RepoImported {
                    reference: RepoRefWire {
                        org: org.clone(),
                        name: rr.name.clone(),
                    },
                    url: rr.url.clone(),
                };
            }
            if !dry && added > 0 {
                let orgs_dir = config_dir.join("orgs");
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &updated) {
                    yield cfg_error_event(e); return;
                }
            }
            yield RepoEvent::ImportSummary {
                org: org.clone(),
                total,
                added,
                skipped,
            };
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

// V5LIFECYCLE-3: `compute_drift` relocated to `crate::v5::ops::repo`.
// No in-module callers remain after the migration.

// Silence unused-import lint if adapters are only used indirectly.
#[allow(dead_code)]
struct _KeepLinkedTypes(Arc<dyn SecretResolver>, ForgePortError, RemoteUrl);
