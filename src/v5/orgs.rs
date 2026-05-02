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
    load_orgs, save_org, CredentialEntry, CredentialType, ForgeProviderBlock, OrgConfig, OrgName,
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
        /// Deprecated — use `providers`. Kept for backward compat.
        provider: ProviderKind,
        /// All configured providers for this org (MFORGE-4).
        providers: Vec<ProviderKind>,
        repo_count: u32,
    },
    /// Org detail (§types). Emitted by `get`.
    OrgDetail {
        name: OrgName,
        /// Deprecated — use `providers`. Kept for backward compat.
        provider: ProviderKind,
        /// All configured providers for this org (MFORGE-4).
        providers: Vec<ProviderKind>,
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
        /// Which forge the credential was added to (MFORGE-4).
        forge: ProviderKind,
        entry: CredentialEntry,
        dry_run: bool,
    },
    /// Successful `set_credential` when the key already existed on the
    /// org — the entry at that index is swapped in place.
    CredentialReplaced {
        org: OrgName,
        /// Which forge the credential was replaced on (MFORGE-4).
        forge: ProviderKind,
        entry: CredentialEntry,
        dry_run: bool,
    },
    /// Successful `remove_credential`: names the affected org and the
    /// removed key. The secret store entry at that key is untouched.
    CredentialRemoved {
        org: OrgName,
        /// Which forge the credential was removed from (MFORGE-4).
        forge: ProviderKind,
        key: String,
        dry_run: bool,
    },
    /// Generic error event. `code` is drawn from a closed set per method.
    Error { code: String, message: String },
    // V5PARITY-21: orgs.bootstrap events.
    /// One stage of `bootstrap` succeeded — `secret_set` event from the
    /// underlying secret store (mirrored here for stream coherence).
    SecretSet {
        key: String,
        value_length: u32,
    },
    /// `bootstrap` reached `import` and got a count back.
    ImportSummary {
        org: OrgName,
        added: u32,
        skipped: u32,
        total: u32,
    },
    /// Final aggregate emitted by a successful bootstrap.
    BootstrapDone {
        org: OrgName,
        provider: ProviderKind,
        repos_added: u32,
    },
    /// Bootstrap failed at `stage`. Closed enum — see `BootstrapStage`.
    BootstrapFailed {
        stage: BootstrapStage,
        message: String,
    },
}

/// V5PARITY-21: closed enum for `bootstrap`'s stage discriminator.
/// Wire form is the snake-case variant string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStage {
    TokenResolve,
    Secret,
    OrgCreate,
    Credential,
    Import,
}

// ---------------------------------------------------------------------
// Helpers for building events with both `provider` and `providers`.
// ---------------------------------------------------------------------

/// Build an `OrgSummary` from an `OrgConfig`, populating both the
/// deprecated `provider` field and the new `providers` list.
fn org_summary(cfg: &OrgConfig) -> OrgsEvent {
    let providers: Vec<ProviderKind> = cfg.providers().collect();
    OrgsEvent::OrgSummary {
        name: cfg.name.clone(),
        provider: cfg.primary_provider().unwrap_or(ProviderKind::Github),
        providers,
        repo_count: u32::try_from(cfg.repos.len()).unwrap_or(u32::MAX),
    }
}

/// Resolve which forge to target for a credential operation. If `forge`
/// is explicitly provided, validates it exists on the org. If omitted
/// and exactly one forge is configured, uses it. Otherwise errors.
fn resolve_forge_target(
    cfg: &OrgConfig,
    forge: Option<ProviderKind>,
) -> Result<ProviderKind, (String, String)> {
    match forge {
        Some(f) => {
            if cfg.forges.contains_key(&f) {
                Ok(f)
            } else {
                Err(("forge_not_found".into(), format!("org {:?} has no forge {f:?}", cfg.name.as_str())))
            }
        }
        None => {
            if cfg.forges.len() == 1 {
                Ok(*cfg.forges.keys().next().unwrap())
            } else {
                Err(("ambiguous_forge".into(), format!(
                    "org {:?} has {} forges; pass --forge to disambiguate",
                    cfg.name.as_str(),
                    cfg.forges.len()
                )))
            }
        }
    }
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
                        yield org_summary(&cfg);
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
                    let providers: Vec<ProviderKind> = cfg.providers().collect();
                    yield OrgsEvent::OrgDetail {
                        name: cfg.name.clone(),
                        provider: cfg.primary_provider().unwrap_or(ProviderKind::Github),
                        providers,
                        credentials: cfg.all_credentials().map(|(_, c)| c.clone()).collect(),
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
                forges: {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(provider, ForgeProviderBlock { credentials: Vec::new() });
                    m
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
            yield org_summary(&cfg);
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

    /// `orgs.update` — patch the provider on an existing org. Supports
    /// legacy single-provider migration (`--provider`) and multi-forge
    /// add/remove (`--add_forge`, `--remove_forge`). Omitting every
    /// optional field is a typed no-op error, never a silent success.
    /// (V5ORGS-6, MFORGE-4)
    #[plexus_macros::method(
        description = "Patch org forges without touching credentials or repos",
        params(
            org = "Org name",
            provider = "New forge provider — legacy single-forge migration (optional)",
            add_forge = "Add a forge provider to this org (optional)",
            remove_forge = "Remove a forge provider from this org (optional)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn update(
        &self,
        org: Option<String>,
        provider: Option<ProviderKind>,
        add_forge: Option<ProviderKind>,
        remove_forge: Option<ProviderKind>,
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
            if provider.is_none() && add_forge.is_none() && remove_forge.is_none() {
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
            // Legacy single-provider migration: moves all credentials to the
            // new provider, dropping the old key.
            if let Some(p) = provider {
                let old_pk = cfg.primary_provider();
                let creds = old_pk
                    .and_then(|k| cfg.forges.remove(&k))
                    .map(|b| b.credentials)
                    .unwrap_or_default();
                cfg.forges.insert(p, ForgeProviderBlock { credentials: creds });
            }
            // MFORGE-4: add a forge with an empty credential block.
            if let Some(af) = add_forge {
                cfg.forges.entry(af).or_insert_with(|| ForgeProviderBlock { credentials: Vec::new() });
            }
            // MFORGE-4: remove a forge and all its credentials.
            if let Some(rf) = remove_forge {
                if !cfg.forges.contains_key(&rf) {
                    yield OrgsEvent::Error {
                        code: "forge_not_found".into(),
                        message: format!("org {org:?} has no forge {rf:?}"),
                    };
                    return;
                }
                if cfg.forges.len() <= 1 {
                    yield OrgsEvent::Error {
                        code: "last_forge".into(),
                        message: "cannot remove the last forge from an org".into(),
                    };
                    return;
                }
                cfg.forges.remove(&rf);
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
            yield org_summary(&cfg);
        }
    }

    /// `orgs.set_credential` — add or replace one credential entry by
    /// key on a specific forge. If `--forge` is omitted and the org has
    /// exactly one forge, that forge is used. If omitted with multiple
    /// forges, returns `ambiguous_forge` error. Keys MUST be `secrets://…`
    /// refs or absolute filesystem paths — plaintext secrets are
    /// rejected at the wire boundary. (V5ORGS-7, MFORGE-4)
    #[plexus_macros::method(
        description = "Add or replace one credential entry by key on a forge",
        params(
            org = "Org name",
            key = "Credential key (secrets:// ref or absolute path)",
            credential_type = "Credential kind (token, ssh_key)",
            forge = "Target forge provider (optional — inferred if org has one forge)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn set_credential(
        &self,
        org: Option<String>,
        key: Option<String>,
        credential_type: Option<CredentialType>,
        forge: Option<ProviderKind>,
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

            // MFORGE-4: resolve which forge to target.
            let target_forge = match resolve_forge_target(&cfg, forge) {
                Ok(f) => f,
                Err((code, message)) => {
                    yield OrgsEvent::Error { code, message };
                    return;
                }
            };

            let entry = CredentialEntry { key: key.clone(), cred_type };
            let creds = &mut cfg.forges.get_mut(&target_forge).expect("resolve_forge_target validated").credentials;
            let replaced = if let Some(existing) =
                creds.iter_mut().find(|c| c.key == key)
            {
                *existing = entry.clone();
                true
            } else {
                creds.push(entry.clone());
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
                OrgsEvent::CredentialReplaced { org: cfg.name, forge: target_forge, entry, dry_run: is_dry }
            } else {
                OrgsEvent::CredentialAdded { org: cfg.name, forge: target_forge, entry, dry_run: is_dry }
            };
        }
    }

    /// `orgs.remove_credential` — remove exactly the `CredentialEntry`
    /// whose `key` equals the input from a specific forge. If `--forge`
    /// is omitted and the org has exactly one forge, that forge is used.
    /// If omitted with multiple forges, returns `ambiguous_forge` error.
    /// The secret store entry at the removed `key` is untouched — that's
    /// a separate user action. (V5ORGS-8, MFORGE-4)
    #[plexus_macros::method(
        description = "Remove one credential entry by key from a forge",
        params(
            org = "Org name",
            key = "Credential key to remove",
            forge = "Target forge provider (optional — inferred if org has one forge)",
            dry_run = "Preview without writing (default false)"
        )
    )]
    pub async fn remove_credential(
        &self,
        org: Option<String>,
        key: Option<String>,
        forge: Option<ProviderKind>,
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

            // MFORGE-4: resolve which forge to target.
            let target_forge = match resolve_forge_target(&cfg, forge) {
                Ok(f) => f,
                Err((code, message)) => {
                    yield OrgsEvent::Error { code, message };
                    return;
                }
            };

            let creds = &mut cfg.forges.get_mut(&target_forge).expect("resolve_forge_target validated").credentials;
            let original_len = creds.len();
            creds.retain(|c| c.key != key);
            if creds.len() == original_len {
                yield OrgsEvent::Error {
                    code: "key_not_found".into(),
                    message: format!("credential key {key:?} not found on org {org:?} forge {target_forge:?}"),
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
                forge: target_forge,
                key,
                dry_run: is_dry,
            };
        }
    }

    // ==================================================================
    // V5PARITY-21: orgs.bootstrap — one-shot secret + org + cred + import.
    // ==================================================================

    #[plexus_macros::method(params(
        name = "Org name",
        provider = "github | codeberg | gitlab",
        token = "Token value or special form (gh-token://, env://VAR)",
        secret_key = "Override the default secret path (defaults to secrets://<provider>/<name>/token)",
        use_default_token = "If true, register the org with the provider-default credential ref instead of a per-org one (V5PARITY-24)",
        import = "Run repos.import after credentials wired (default: true)",
        dry_run = "Preview without writing"
    ))]
    pub async fn bootstrap(
        &self,
        name: Option<String>,
        provider: Option<ProviderKind>,
        token: Option<String>,
        secret_key: Option<String>,
        use_default_token: Option<bool>,
        import: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgsEvent> + Send + 'static {
        let this = self.clone();
        stream! {
            let Some(name) = name.filter(|s| !s.is_empty()) else {
                yield OrgsEvent::BootstrapFailed {
                    stage: BootstrapStage::OrgCreate,
                    message: "missing required parameter 'name'".into(),
                };
                return;
            };
            let Some(provider) = provider else {
                yield OrgsEvent::BootstrapFailed {
                    stage: BootstrapStage::OrgCreate,
                    message: "missing required parameter 'provider'".into(),
                };
                return;
            };
            let Some(token) = token.filter(|s| !s.is_empty()) else {
                yield OrgsEvent::BootstrapFailed {
                    stage: BootstrapStage::TokenResolve,
                    message: "missing required parameter 'token'".into(),
                };
                return;
            };
            let dry = dry_run.unwrap_or(false);
            let do_import = import.unwrap_or(true);
            let use_default = use_default_token.unwrap_or(false);

            // --- Stage: token_resolve. Special forms expand here. ---
            let token_value = match resolve_token_form(&token, provider).await {
                Ok(v) => v,
                Err(e) => {
                    yield OrgsEvent::BootstrapFailed {
                        stage: BootstrapStage::TokenResolve,
                        message: e,
                    };
                    return;
                }
            };

            // --- Stage: secret. Compute path, write to store. ---
            let provider_str = match provider {
                ProviderKind::Github => "github",
                ProviderKind::Codeberg => "codeberg",
                ProviderKind::Gitlab => "gitlab",
            };
            let secret_path = secret_key.unwrap_or_else(|| {
                if use_default {
                    format!("secrets://{provider_str}/_default/token")
                } else {
                    format!("secrets://{provider_str}/{name}/token")
                }
            });
            let token_len = u32::try_from(token_value.len()).unwrap_or(u32::MAX);
            if !dry {
                let store = crate::v5::secrets::YamlSecretStore::new(this.config_dir.as_ref());
                let parsed = match crate::v5::secrets::SecretRef::parse(&secret_path) {
                    Ok(p) => p,
                    Err(e) => {
                        yield OrgsEvent::BootstrapFailed {
                            stage: BootstrapStage::Secret,
                            message: format!("invalid secret path '{secret_path}': {e}"),
                        };
                        return;
                    }
                };
                if let Err(e) = store.put_secret(&parsed, &token_value) {
                    yield OrgsEvent::BootstrapFailed {
                        stage: BootstrapStage::Secret,
                        message: format!("write secret: {e}"),
                    };
                    return;
                }
            }
            yield OrgsEvent::SecretSet { key: secret_path.clone(), value_length: token_len };

            // --- Stage: org_create. Use the existing helper (idempotent on existing). ---
            // Build OrgConfig and write it (or detect existing).
            let org_name = OrgName::from(name.as_str());
            let orgs_dir = this.config_dir.join("orgs");
            let existing = if orgs_dir.is_dir() {
                match crate::v5::ops::state::load_orgs(&orgs_dir) {
                    Ok(map) => map.into_iter()
                        .find(|(n, _)| n.as_str() == name.as_str())
                        .map(|(_, v)| v),
                    Err(e) => {
                        yield OrgsEvent::BootstrapFailed {
                            stage: BootstrapStage::OrgCreate,
                            message: format!("load orgs: {e}"),
                        };
                        return;
                    }
                }
            } else {
                None
            };
            let mut cfg = existing.unwrap_or_else(|| crate::v5::config::OrgConfig {
                name: org_name.clone(),
                forges: {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(provider, ForgeProviderBlock { credentials: Vec::new() });
                    m
                },
                repos: Vec::new(),
            });
            // Ensure the target provider exists (may migrate from a different one).
            if !cfg.forges.contains_key(&provider) {
                cfg.forges.insert(provider, ForgeProviderBlock { credentials: Vec::new() });
            }

            // --- Stage: credential. Replace any existing token cred with the new ref. ---
            let creds = cfg.forges.get_mut(&provider).expect("just ensured");
            creds.credentials.retain(|c| !matches!(c.cred_type, CredentialType::Token));
            creds.credentials.push(CredentialEntry {
                key: secret_path,
                cred_type: CredentialType::Token,
            });

            if !dry {
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &cfg) {
                    yield OrgsEvent::BootstrapFailed {
                        stage: BootstrapStage::OrgCreate,
                        message: format!("write org yaml: {e}"),
                    };
                    return;
                }
            }
            yield org_summary(&cfg);
            yield OrgsEvent::CredentialAdded {
                org: org_name.clone(),
                forge: provider,
                entry: cfg.forges.get(&provider).expect("just ensured").credentials.last().expect("just pushed").clone(),
                dry_run: dry,
            };

            // --- Stage: import. Optional. Reuses the same routing as repos.import. ---
            if !do_import || dry {
                yield OrgsEvent::BootstrapDone {
                    org: org_name,
                    provider,
                    repos_added: 0,
                };
                return;
            }
            let resolver = crate::v5::secrets::YamlSecretStore::new(this.config_dir.as_ref());
            // MFORGE-6: use the bootstrapped provider's credential, not the
            // primary provider's (which may differ on a multi-forge org).
            let token_ref = crate::v5::ops::repo::token_ref_for_provider(&cfg, provider);
            let fallback = Some(crate::v5::ops::repo::default_token_ref_for_provider(provider));
            let remote_repos = match crate::v5::ops::repo::list_on_forge(
                provider, &org_name, &resolver, token_ref, fallback,
            ).await {
                Ok(v) => v,
                Err(e) => {
                    yield OrgsEvent::BootstrapFailed {
                        stage: BootstrapStage::Import,
                        message: format!("list_repos: {}", e.message),
                    };
                    return;
                }
            };
            let total = u32::try_from(remote_repos.len()).unwrap_or(0);
            let existing_names: std::collections::BTreeSet<String> = cfg.repos.iter()
                .map(|r| r.name.as_str().to_string())
                .collect();
            let mut added = 0u32;
            let mut skipped = 0u32;
            for r in remote_repos {
                if existing_names.contains(&r.name) {
                    skipped += 1;
                    continue;
                }
                cfg.repos.push(crate::v5::config::OrgRepo {
                    name: crate::v5::config::RepoName::from(r.name.as_str()),
                    remotes: vec![crate::v5::config::Remote {
                        url: crate::v5::config::RemoteUrl::from(r.url.as_str()),
                        provider: None,
                    }],
                    forges: None,
                    metadata: None,
                });
                added += 1;
            }
            if added > 0 {
                if let Err(e) = crate::v5::ops::state::save_org(&orgs_dir, &cfg) {
                    yield OrgsEvent::BootstrapFailed {
                        stage: BootstrapStage::Import,
                        message: format!("save org yaml: {e}"),
                    };
                    return;
                }
            }
            yield OrgsEvent::ImportSummary {
                org: org_name.clone(),
                added,
                skipped,
                total,
            };
            yield OrgsEvent::BootstrapDone {
                org: org_name,
                provider,
                repos_added: added,
            };
        }
    }
}

/// V5PARITY-21: resolve `token` argument to a concrete value. Recognized
/// special forms: `gh-token://` (calls `ops::external_auth::read_token`)
/// and `env://VAR`. Anything else is passed through as a raw token.
async fn resolve_token_form(
    token: &str,
    provider: ProviderKind,
) -> Result<String, String> {
    if let Some(scheme_rest) = token.strip_prefix("gh-token://") {
        let _ = scheme_rest; // currently no path component
        let ext_provider = match provider {
            ProviderKind::Github => crate::v5::ops::external_auth::ExternalAuthProvider::Github,
            ProviderKind::Codeberg => crate::v5::ops::external_auth::ExternalAuthProvider::Codeberg,
            ProviderKind::Gitlab => crate::v5::ops::external_auth::ExternalAuthProvider::Gitlab,
        };
        return crate::v5::ops::external_auth::read_token(ext_provider)
            .map_err(|e| e.to_string());
    }
    if let Some(var) = token.strip_prefix("env://") {
        return std::env::var(var)
            .map_err(|e| format!("env var '{var}': {e}"));
    }
    Ok(token.to_string())
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
