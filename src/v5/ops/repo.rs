//! `ops::repo` — repo-level operations, single source of truth for
//! every hub that touches a repo (V5LIFECYCLE-3, -4, -5).
//!
//! - `derive_provider`, `compute_drift` — relocated from `repos.rs`
//! - `sync_one` — pure sync inner loop; both `ReposHub::sync` and
//!   `WorkspacesHub::sync` iterate its output.
//! - `exists_on_forge`, `create_on_forge`, `delete_on_forge`,
//!   `privatize_on_forge` — the forge-call wrappers. Hubs never call
//!   adapter lifecycle methods directly after this module.
//! - `dismiss`, `purge` — pure state mutations (V5LIFECYCLE-5).

use std::collections::{BTreeMap, BTreeSet};

use crate::v5::adapters::{
    self, extract_host, for_provider, DriftFieldKind, ForgeAuth, ForgeErrorClass, ForgeMetadata,
    ForgePortError, MetadataFields, ProviderVisibility,
};
use crate::v5::config::{
    CredentialType, DomainName, OrgConfig, OrgRepo, ProviderKind, Remote, RepoMetadataLocal,
    RepoName, RepoRef,
};
use crate::v5::secrets::SecretResolver;

// ---------------------------------------------------------------------
// Provider derivation (relocated from repos.rs; still pub(crate)-ish —
// we expose it as `pub` so the hubs can call it through the ops
// namespace and no caller re-introduces a duplicate).
// ---------------------------------------------------------------------

/// Map a `Remote` to its `ProviderKind` via either an explicit
/// `provider:` override on the remote or the global `provider_map`
/// domain lookup.
pub fn derive_provider(
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

// ---------------------------------------------------------------------
// Drift computation (relocated from repos.rs).
// ---------------------------------------------------------------------

/// A single metadata field that disagrees between local declared state
/// and the forge's current state.
#[derive(Debug, Clone)]
pub struct DriftField {
    pub field: String,
    pub local: serde_json::Value,
    pub remote: serde_json::Value,
}

/// Compute drift from local `RepoMetadataLocal` (a declared subset)
/// against the forge's `ForgeMetadata` (always all four fields).
/// Only fields the local side declares participate in drift.
#[must_use]
pub fn compute_drift(local: &Option<RepoMetadataLocal>, remote: &ForgeMetadata) -> Vec<DriftField> {
    let Some(local) = local else {
        return vec![];
    };
    let mut out = Vec::new();
    if let Some(ref lv) = local.default_branch {
        if *lv != remote.default_branch {
            out.push(DriftField {
                field: "default_branch".into(),
                local: serde_json::Value::String(lv.clone()),
                remote: serde_json::Value::String(remote.default_branch.clone()),
            });
        }
    }
    if let Some(ref lv) = local.description {
        if *lv != remote.description {
            out.push(DriftField {
                field: "description".into(),
                local: serde_json::Value::String(lv.clone()),
                remote: serde_json::Value::String(remote.description.clone()),
            });
        }
    }
    if let Some(lv) = local.archived {
        if lv != remote.archived {
            out.push(DriftField {
                field: "archived".into(),
                local: serde_json::Value::Bool(lv),
                remote: serde_json::Value::Bool(remote.archived),
            });
        }
    }
    if let Some(ref lv) = local.visibility {
        if *lv != remote.visibility {
            out.push(DriftField {
                field: "visibility".into(),
                local: serde_json::Value::String(lv.clone()),
                remote: serde_json::Value::String(remote.visibility.clone()),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------
// Forge-call wrappers (V5LIFECYCLE-4).
// ---------------------------------------------------------------------

/// Token credential-ref lookup for an org (first cred of type Token,
/// if any).
pub fn token_ref_for(org: &OrgConfig) -> Option<&str> {
    org.forge
        .credentials
        .iter()
        .find(|c| matches!(c.cred_type, CredentialType::Token))
        .map(|c| c.key.as_str())
}

/// Does this remote exist on its forge?
pub async fn exists_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<bool, ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref: token_ref,
        resolver,
    };
    adapter.repo_exists(remote, repo_ref, &auth).await
}

/// Create a repo on its forge.
pub async fn create_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    visibility: ProviderVisibility,
    description: &str,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref: token_ref,
        resolver,
    };
    adapter.create_repo(remote, repo_ref, visibility, description, &auth).await
}

/// Delete a repo on its forge.
pub async fn delete_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref: token_ref,
        resolver,
    };
    adapter.delete_repo(remote, repo_ref, &auth).await
}

/// V5PARITY-6: rename a repo on the forge.
pub async fn rename_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    new_name: &str,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, resolver };
    adapter.rename_repo(remote, repo_ref, new_name, &auth).await
}

/// V5PARITY-2: list repos on a forge for an org. `provider` is
/// supplied explicitly because there's no per-repo `Remote` yet at
/// import time.
pub async fn list_on_forge(
    provider: ProviderKind,
    org: &crate::v5::config::OrgName,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<Vec<crate::v5::adapters::RemoteRepo>, ForgePortError> {
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref,
        resolver,
    };
    adapter.list_repos(org, &auth).await
}

/// Generic metadata write on the forge. Used by `repos.push`.
pub async fn write_metadata_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    fields: &MetadataFields,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<MetadataFields, ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref,
        resolver,
    };
    adapter.write_metadata(remote, repo_ref, fields, &auth).await
}

/// Privatize a repo on its forge via `write_metadata` with only the
/// `visibility` field set to `private`. Used by soft-delete
/// (V5LIFECYCLE-6). Does not touch any other metadata field.
pub async fn privatize_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth {
        token_ref: token_ref,
        resolver,
    };
    let mut fields: MetadataFields = std::collections::BTreeMap::new();
    fields.insert(
        DriftFieldKind::Visibility,
        serde_json::Value::String("private".into()),
    );
    adapter
        .write_metadata(remote, repo_ref, &fields, &auth)
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------------
// Sync — single source of truth, called by both hubs (V5LIFECYCLE-3).
// ---------------------------------------------------------------------

/// One per-remote sync outcome.
#[derive(Debug, Clone)]
pub struct SyncOutcomeEntry {
    pub remote: Remote,
    pub provider: Option<ProviderKind>,
    pub status: SyncStatus,
    pub drift: Vec<DriftField>,
    pub metadata: Option<ForgeMetadata>,
    pub error_class: Option<ForgeErrorClass>,
    pub error_message: Option<String>,
}

/// `SyncStatus` reuses the CONTRACTS §types enum variants as lowercase
/// strings on the wire; here we use a bounded enum internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    InSync,
    Drifted,
    Errored,
}

impl SyncStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "in_sync",
            Self::Drifted => "drifted",
            Self::Errored => "errored",
        }
    }
}

/// Run the sync inner loop on a single repo. Per-remote result vec;
/// callers aggregate at the level they care about (per-remote events
/// for `repos.sync`, per-repo collapsed events for `workspaces.sync`).
pub async fn sync_one(
    repo: &OrgRepo,
    org: &OrgConfig,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    remote_filter: Option<&str>,
) -> Vec<SyncOutcomeEntry> {
    let tref = token_ref_for(org);
    let repo_ref = RepoRef {
        org: org.name.clone(),
        name: repo.name.clone(),
    };
    let filtered: Vec<&Remote> = if let Some(f) = remote_filter.filter(|s| !s.is_empty()) {
        repo.remotes.iter().filter(|r| r.url.as_str() == f).collect()
    } else {
        repo.remotes.iter().collect()
    };
    let mut out = Vec::new();
    for r in filtered {
        let provider = match derive_provider(r, provider_map) {
            Ok(p) => Some(p),
            Err(e) => {
                out.push(SyncOutcomeEntry {
                    remote: r.clone(),
                    provider: None,
                    status: SyncStatus::Errored,
                    drift: vec![],
                    metadata: None,
                    error_class: Some(ForgeErrorClass::Network),
                    error_message: Some(e),
                });
                continue;
            }
        };
        let adapter = for_provider(provider.unwrap());
        let auth = ForgeAuth {
            token_ref: tref,
            resolver,
        };
        match adapter.read_metadata(r, &repo_ref, &auth).await {
            Ok(meta) => {
                let drift = compute_drift(&repo.metadata, &meta);
                let status = if drift.is_empty() {
                    SyncStatus::InSync
                } else {
                    SyncStatus::Drifted
                };
                out.push(SyncOutcomeEntry {
                    remote: r.clone(),
                    provider,
                    status,
                    drift,
                    metadata: Some(meta),
                    error_class: None,
                    error_message: None,
                });
            }
            Err(e) => {
                out.push(SyncOutcomeEntry {
                    remote: r.clone(),
                    provider,
                    status: SyncStatus::Errored,
                    drift: vec![],
                    metadata: None,
                    error_class: Some(e.class),
                    error_message: Some(e.message),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// Lifecycle state mutations (V5LIFECYCLE-5).
// ---------------------------------------------------------------------

/// Set the repo's lifecycle to `dismissed` and union `privatized` into
/// its `privatized_on` set. Doesn't write disk — caller follows up
/// with `ops::state::save_org`.
pub fn dismiss(repo: &mut OrgRepo, privatized: BTreeSet<ProviderKind>) {
    let md = repo.metadata.get_or_insert_with(RepoMetadataLocal::default);
    md.lifecycle = Some(crate::v5::config::RepoLifecycle::Dismissed);
    md.privatized_on.extend(privatized);
}

/// Remove the repo entry from the org (after purge guards). Errors:
/// `NotDismissed` when lifecycle != dismissed; `Protected` when
/// protected == true.
pub fn purge(org: &mut OrgConfig, name: &RepoName) -> Result<(), PurgeError> {
    let idx = org
        .repos
        .iter()
        .position(|r| r.name == *name)
        .ok_or(PurgeError::NotFound)?;
    let repo = &org.repos[idx];
    let md = repo.metadata.as_ref();
    let protected = md.and_then(|m| m.protected).unwrap_or(false);
    if protected {
        return Err(PurgeError::Protected);
    }
    let lifecycle = md.and_then(|m| m.lifecycle);
    if lifecycle != Some(crate::v5::config::RepoLifecycle::Dismissed) {
        return Err(PurgeError::NotDismissed);
    }
    org.repos.remove(idx);
    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PurgeError {
    #[error("repo not found in org")]
    NotFound,
    #[error("repo is protected")]
    Protected,
    #[error("repo lifecycle is not dismissed (run repos.delete first)")]
    NotDismissed,
}

// Suppress unused imports we expose for hub consumers.
#[allow(unused_imports)]
use adapters as _adapters_anchor;
