//! `OrgsHub` — v5 orgs namespace. V5ORGS attaches CRUD + credential
//! management methods on top of the V5CORE-6 stub.
//!
//! Event envelope follows CONTRACTS D9: every event serializes with a
//! top-level `type` discriminator (`snake_case`). Errors use
//! `{type: "error", code, message}`. Secret redaction rule is enforced:
//! returned events carry `CredentialEntry` refs (key + type) only —
//! never resolved plaintext values.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::config::{
    load_orgs, save_org, CredentialEntry, CredentialType, ForgeBlock, OrgConfig, OrgName,
    ProviderKind, RepoName,
};

// ---------------------------------------------------------------------
// Event envelope (D9).
// ---------------------------------------------------------------------

/// Events emitted by `OrgsHub` methods. `type` is the wire discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrgsEvent {
    /// Org summary (§types). Emitted by `list`, `create`, `update`.
    OrgSummary {
        name: OrgName,
        provider: ProviderKind,
        repo_count: u32,
    },
    /// Org detail (§types). Emitted by `get`.
    OrgDetail {
        name: OrgName,
        provider: ProviderKind,
        credentials: Vec<CredentialEntry>,
        repos: Vec<RepoName>,
    },
    /// Successful `delete`: names the removed org. `dry_run` echoes the
    /// request so callers can distinguish preview from real deletion.
    OrgDeleted { name: OrgName, dry_run: bool },
    /// Successful `set_credential` when the key was not previously on
    /// the org. A distinct event `type` (not a sub-field) so callers can
    /// assert on the discriminator alone.
    CredentialAdded {
        org: OrgName,
        entry: CredentialEntry,
        dry_run: bool,
    },
    /// Successful `set_credential` when the key already existed on the
    /// org — the entry at that index is swapped in place.
    CredentialReplaced {
        org: OrgName,
        entry: CredentialEntry,
        dry_run: bool,
    },
    /// Successful `remove_credential`: names the affected org and the
    /// removed key. The secret store entry at that key is untouched.
    CredentialRemoved {
        org: OrgName,
        key: String,
        dry_run: bool,
    },
    /// Generic error event. `code` is drawn from a closed set per method.
    Error { code: String, message: String },
}

// ---------------------------------------------------------------------
// Hub.
// ---------------------------------------------------------------------

/// Orgs namespace. Holds the config directory shared with the root hub.
#[derive(Clone)]
pub struct OrgsHub {
    config_dir: Arc<PathBuf>,
}

impl OrgsHub {
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir: Arc::new(config_dir),
        }
    }

    fn orgs_dir(&self) -> PathBuf {
        self.config_dir.join("orgs")
    }

    fn org_path(&self, name: &str) -> PathBuf {
        self.orgs_dir().join(format!("{name}.yaml"))
    }
}

// ---------------------------------------------------------------------
// Activation.
// ---------------------------------------------------------------------

/// Orgs CRUD and credentials. Methods are attached by V5ORGS tickets.
#[plexus_macros::activation(
    namespace = "orgs",
    description = "Orgs CRUD",
    crate_path = "plexus_core"
)]
impl OrgsHub {
    /// `orgs.list` — stream one `OrgSummary` event per org on disk,
    /// ascending by `OrgName`. No inputs. Read-only. (V5ORGS-2)
    #[plexus_macros::method(description = "List orgs as OrgSummary events")]
    pub async fn list(&self) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let orgs_dir = self.orgs_dir();
        stream! {
            // Missing `orgs/` is not an error — empty fixtures exist.
            if !orgs_dir.is_dir() {
                return;
            }
            match load_orgs(&orgs_dir) {
                Ok(map) => {
                    // BTreeMap iterates ascending by key → deterministic.
                    for (_name, cfg) in map {
                        yield OrgsEvent::OrgSummary {
                            name: cfg.name.clone(),
                            provider: cfg.forge.provider,
                            repo_count: u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX),
                        };
                    }
                }
                Err(e) => {
                    yield OrgsEvent::Error {
                        code: "load_failed".into(),
                        message: format!("{e}"),
                    };
                }
            }
        }
    }

    /// `orgs.get` — return the `OrgDetail` for one org. Credentials are
    /// returned as refs only (key + type); resolved plaintext never
    /// appears in the event stream. (V5ORGS-3)
    #[plexus_macros::method(
        description = "Get an org's detail (never leaks secret values)",
        params(org = "Org name")
    )]
    pub async fn get(&self, org: Option<String>) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let Some(org) = org else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'org'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&org) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            match read_org(&config_dir, &org) {
                Ok(cfg) => {
                    yield OrgsEvent::OrgDetail {
                        name: cfg.name.clone(),
                        provider: cfg.forge.provider,
                        credentials: cfg.forge.credentials.clone(),
                        repos: cfg.repos.iter().map(|r| r.name.clone()).collect(),
                    };
                }
                Err(ReadOrgError::NotFound) => {
                    yield OrgsEvent::Error {
                        code: "not_found".into(),
                        message: format!("org {org:?} not found"),
                    };
                }
                Err(ReadOrgError::Io(msg)) => {
                    yield OrgsEvent::Error { code: "io_error".into(), message: msg };
                }
                Err(ReadOrgError::Parse(msg)) => {
                    yield OrgsEvent::Error { code: "parse_error".into(), message: msg };
                }
            }
        }
    }

    /// `orgs.create` — write a new `orgs/<name>.yaml` atomically. The
    /// org is created empty (no credentials, no repos); adding those is
    /// the job of V5ORGS-7 / V5REPOS. (V5ORGS-4)
    #[plexus_macros::method(
        description = "Create a new org yaml",
        params(
            name = "Org name (filename-safe)",
            provider = "Forge provider (github, codeberg, gitlab)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn create(
        &self,
        name: Option<String>,
        provider: Option<ProviderKind>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(name) = name else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'name'".into(),
                };
                return;
            };
            let Some(provider) = provider else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'provider'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&name) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            if this.org_path(&name).exists() {
                yield OrgsEvent::Error {
                    code: "already_exists".into(),
                    message: format!("org {name:?} already exists"),
                };
                return;
            }
            let cfg = OrgConfig {
                name: OrgName(name),
                forge: ForgeBlock {
                    provider,
                    credentials: Vec::new(),
                },
                repos: Vec::new(),
            };
            let is_dry = dry_run.unwrap_or(false);
            if !is_dry {
                if let Err(e) = save_org(&this.orgs_dir(), &cfg) {
                    yield OrgsEvent::Error {
                        code: "io_error".into(),
                        message: format!("{e}"),
                    };
                    return;
                }
            }
            yield OrgsEvent::OrgSummary {
                name: cfg.name,
                provider: cfg.forge.provider,
                repo_count: 0,
            };
        }
    }

    /// `orgs.delete` — remove an `orgs/<name>.yaml` from local disk.
    /// No forge-side deletion (README invariant 4). (V5ORGS-5)
    #[plexus_macros::method(
        description = "Delete an org yaml (local filesystem only)",
        params(
            org = "Org name",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn delete(
        &self,
        org: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(org) = org else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'org'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&org) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            let path = this.org_path(&org);
            if !path.is_file() {
                yield OrgsEvent::Error {
                    code: "not_found".into(),
                    message: format!("org {org:?} not found"),
                };
                return;
            }
            let is_dry = dry_run.unwrap_or(false);
            if !is_dry {
                if let Err(e) = std::fs::remove_file(&path) {
                    yield OrgsEvent::Error {
                        code: "io_error".into(),
                        message: format!("failed to delete {}: {e}", path.display()),
                    };
                    return;
                }
            }
            yield OrgsEvent::OrgDeleted {
                name: OrgName(org),
                dry_run: is_dry,
            };
        }
    }

    /// `orgs.update` — patch the provider on an existing org. Every
    /// other field (credentials, repos) is preserved byte-equivalent
    /// through the V5CORE-3 load/save round-trip. Omitting every
    /// optional field is a typed no-op error, never a silent success.
    /// (V5ORGS-6)
    #[plexus_macros::method(
        description = "Patch org provider without touching credentials or repos",
        params(
            org = "Org name",
            provider = "New forge provider (optional)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn update(
        &self,
        org: Option<String>,
        provider: Option<ProviderKind>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(org) = org else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'org'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&org) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            if provider.is_none() {
                yield OrgsEvent::Error {
                    code: "no_op".into(),
                    message: "orgs.update requires at least one optional field to change".into(),
                };
                return;
            }
            let mut cfg = match read_org(&this.config_dir, &org) {
                Ok(c) => c,
                Err(ReadOrgError::NotFound) => {
                    yield OrgsEvent::Error {
                        code: "not_found".into(),
                        message: format!("org {org:?} not found"),
                    };
                    return;
                }
                Err(ReadOrgError::Io(m)) => {
                    yield OrgsEvent::Error { code: "io_error".into(), message: m };
                    return;
                }
                Err(ReadOrgError::Parse(m)) => {
                    yield OrgsEvent::Error { code: "parse_error".into(), message: m };
                    return;
                }
            };
            if let Some(p) = provider {
                cfg.forge.provider = p;
            }
            let is_dry = dry_run.unwrap_or(false);
            if !is_dry {
                if let Err(e) = save_org(&this.orgs_dir(), &cfg) {
                    yield OrgsEvent::Error {
                        code: "io_error".into(),
                        message: format!("{e}"),
                    };
                    return;
                }
            }
            yield OrgsEvent::OrgSummary {
                name: cfg.name,
                provider: cfg.forge.provider,
                repo_count: u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX),
            };
        }
    }

    /// `orgs.set_credential` — add or replace one credential entry by
    /// key. If no existing entry matches `key`, append; otherwise
    /// replace in place preserving index. Keys MUST be `secrets://…`
    /// refs or absolute filesystem paths — plaintext secrets are
    /// rejected at the wire boundary. (V5ORGS-7)
    #[plexus_macros::method(
        description = "Add or replace one credential entry by key",
        params(
            org = "Org name",
            key = "Credential key (secrets:// ref or absolute path)",
            credential_type = "Credential kind (token, ssh_key)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn set_credential(
        &self,
        org: Option<String>,
        key: Option<String>,
        credential_type: Option<CredentialType>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(org) = org else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'org'".into(),
                };
                return;
            };
            let Some(key) = key else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'key'".into(),
                };
                return;
            };
            let Some(cred_type) = credential_type else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'credential_type'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&org) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            if let Err(e) = validate_credential_key(&key) {
                yield OrgsEvent::Error { code: "invalid_key".into(), message: e };
                return;
            }
            let mut cfg = match read_org(&this.config_dir, &org) {
                Ok(c) => c,
                Err(ReadOrgError::NotFound) => {
                    yield OrgsEvent::Error {
                        code: "org_not_found".into(),
                        message: format!("org {org:?} not found"),
                    };
                    return;
                }
                Err(ReadOrgError::Io(m)) => {
                    yield OrgsEvent::Error { code: "io_error".into(), message: m };
                    return;
                }
                Err(ReadOrgError::Parse(m)) => {
                    yield OrgsEvent::Error { code: "parse_error".into(), message: m };
                    return;
                }
            };

            let entry = CredentialEntry { key: key.clone(), cred_type };
            let replaced = if let Some(existing) =
                cfg.forge.credentials.iter_mut().find(|c| c.key == key)
            {
                *existing = entry.clone();
                true
            } else {
                cfg.forge.credentials.push(entry.clone());
                false
            };

            let is_dry = dry_run.unwrap_or(false);
            if !is_dry {
                if let Err(e) = save_org(&this.orgs_dir(), &cfg) {
                    yield OrgsEvent::Error {
                        code: "io_error".into(),
                        message: format!("{e}"),
                    };
                    return;
                }
            }
            yield if replaced {
                OrgsEvent::CredentialReplaced { org: cfg.name, entry, dry_run: is_dry }
            } else {
                OrgsEvent::CredentialAdded { org: cfg.name, entry, dry_run: is_dry }
            };
        }
    }

    /// `orgs.remove_credential` — remove exactly the `CredentialEntry`
    /// whose `key` equals the input. Order of remaining entries is
    /// preserved. The secret store entry at the removed `key` is
    /// untouched — that's a separate user action. (V5ORGS-8)
    #[plexus_macros::method(
        description = "Remove one credential entry by key",
        params(
            org = "Org name",
            key = "Credential key to remove",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn remove_credential(
        &self,
        org: Option<String>,
        key: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(org) = org else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'org'".into(),
                };
                return;
            };
            let Some(key) = key else {
                yield OrgsEvent::Error {
                    code: "missing_param".into(),
                    message: "missing required parameter 'key'".into(),
                };
                return;
            };
            if let Err(e) = validate_org_name(&org) {
                yield OrgsEvent::Error { code: "invalid_name".into(), message: e };
                return;
            }
            let mut cfg = match read_org(&this.config_dir, &org) {
                Ok(c) => c,
                Err(ReadOrgError::NotFound) => {
                    yield OrgsEvent::Error {
                        code: "org_not_found".into(),
                        message: format!("org {org:?} not found"),
                    };
                    return;
                }
                Err(ReadOrgError::Io(m)) => {
                    yield OrgsEvent::Error { code: "io_error".into(), message: m };
                    return;
                }
                Err(ReadOrgError::Parse(m)) => {
                    yield OrgsEvent::Error { code: "parse_error".into(), message: m };
                    return;
                }
            };
            let original_len = cfg.forge.credentials.len();
            cfg.forge.credentials.retain(|c| c.key != key);
            if cfg.forge.credentials.len() == original_len {
                yield OrgsEvent::Error {
                    code: "key_not_found".into(),
                    message: format!("credential key {key:?} not found on org {org:?}"),
                };
                return;
            }
            let is_dry = dry_run.unwrap_or(false);
            if !is_dry {
                if let Err(e) = save_org(&this.orgs_dir(), &cfg) {
                    yield OrgsEvent::Error {
                        code: "io_error".into(),
                        message: format!("{e}"),
                    };
                    return;
                }
            }
            yield OrgsEvent::CredentialRemoved {
                org: cfg.name,
                key,
                dry_run: is_dry,
            };
        }
    }
}

// ---------------------------------------------------------------------
// Disk + validation helpers.
// ---------------------------------------------------------------------

/// Validate a credential key: either `secrets://<non-empty>` (`SecretRef`)
/// or an absolute filesystem path (`FsPath`). Rejects bare plaintext so
/// org yaml cannot accidentally hold a token.
fn validate_credential_key(key: &str) -> Result<(), String> {
    if let Some(rest) = key.strip_prefix("secrets://") {
        if rest.is_empty() {
            return Err(format!("secret reference {key:?} has empty path"));
        }
        return Ok(());
    }
    if key.starts_with('/') && !key.contains("..") && !key.ends_with('/') {
        return Ok(());
    }
    Err(format!(
        "credential key {key:?} is not a 'secrets://…' reference or an absolute filesystem path"
    ))
}

/// Validate an `OrgName` per §types: filename-safe (no `/`, no leading
/// `.`, ≤64 chars, ASCII), non-empty.
fn validate_org_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    if name.len() > 64 {
        return Err(format!("name {name:?} exceeds 64 chars"));
    }
    if !name.is_ascii() {
        return Err(format!("name {name:?} is not ASCII"));
    }
    if name.starts_with('.') {
        return Err(format!("name {name:?} must not start with '.'"));
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(format!("name {name:?} contains a path separator"));
    }
    Ok(())
}

enum ReadOrgError {
    NotFound,
    #[allow(dead_code)]
    Io(String),
    Parse(String),
}

fn read_org(config_dir: &Path, name: &str) -> Result<OrgConfig, ReadOrgError> {
    // V5LIFECYCLE-2: route through ops::state instead of inline yaml.
    let orgs_dir = config_dir.join("orgs");
    if !config_dir.join("orgs").join(format!("{name}.yaml")).is_file() {
        return Err(ReadOrgError::NotFound);
    }
    let all = crate::v5::ops::state::load_orgs(&orgs_dir)
        .map_err(|e| ReadOrgError::Parse(e.to_string()))?;
    all.into_iter()
        .find(|(_, v)| v.name.as_str() == name)
        .map(|(_, v)| v)
        .ok_or(ReadOrgError::NotFound)
}
