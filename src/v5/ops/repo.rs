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

// -- Per-provider credential resolution (MFORGE-3) --------------------

/// Token credential-ref for a specific provider within an org.
/// Returns the `key` of the first `CredentialType::Token` entry
/// in that provider's block, or `None` when the provider is absent
/// or has no token credential.
pub fn token_ref_for_provider(org: &OrgConfig, provider: ProviderKind) -> Option<&str> {
    org.credentials_for(provider)
        .iter()
        .find(|c| matches!(c.cred_type, CredentialType::Token))
        .map(|c| c.key.as_str())
}

/// Provider-default secret path. Convention:
/// `secrets://<provider>/_default/token`. Always returns a path;
/// the secret may or may not exist — that is a runtime resolution
/// concern.
#[must_use]
pub fn default_token_ref_for_provider(provider: ProviderKind) -> String {
    let label = match provider {
        ProviderKind::Github => "github",
        ProviderKind::Codeberg => "codeberg",
        ProviderKind::Gitlab => "gitlab",
    };
    format!("secrets://{label}/_default/token")
}

/// SSH-key credential-ref for a specific provider within an org.
/// Returns the `key` of the first `CredentialType::SshKey` entry
/// in that provider's block, or `None`.
pub fn ssh_key_for_provider(org: &OrgConfig, provider: ProviderKind) -> Option<&str> {
    org.credentials_for(provider)
        .iter()
        .find(|c| matches!(c.cred_type, CredentialType::SshKey))
        .map(|c| c.key.as_str())
}

// -- Backward-compatible wrappers (delegate to primary_provider) ------

/// Token credential-ref lookup for an org's *primary* provider.
/// Thin wrapper over `token_ref_for_provider`; prefer the explicit
/// variant in new code.
pub fn token_ref_for(org: &OrgConfig) -> Option<&str> {
    let pk = org.primary_provider()?;
    token_ref_for_provider(org, pk)
}

/// Provider-default secret path for the org's *primary* provider.
/// Thin wrapper over `default_token_ref_for_provider`; prefer the
/// explicit variant in new code.
#[must_use]
pub fn default_token_ref_for(org: &OrgConfig) -> String {
    let pk = org.primary_provider().unwrap_or(ProviderKind::Github);
    default_token_ref_for_provider(pk)
}

/// V5PARITY-34: filter remotes by the per-repo `forges` scope.
///
/// When `repo.forges` is `None`, every remote participates (legacy
/// behavior). When it's `Some(list)`, only remotes whose derived
/// provider is in `list` are returned. Empty `Some([])` means
/// "the repo is currently scoped to no forges" — no remotes
/// participate; callers emit a `forge_excluded` signal.
#[must_use]
pub fn filter_remotes_by_forges<'a>(
    repo: &'a OrgRepo,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
) -> Vec<&'a Remote> {
    let Some(scope) = repo.forges.as_ref() else {
        return repo.remotes.iter().collect();
    };
    repo.remotes
        .iter()
        .filter(|r| match derive_provider(r, provider_map) {
            Ok(p) => scope.contains(&p),
            Err(_) => false,
        })
        .collect()
}

/// V5PARITY-34: canonical remote that's in scope. Mirrors
/// `OrgRepo::canonical_remote()` (== `remotes.first()`) but applies
/// the `forges` filter. Returns `None` when the repo has no remotes
/// OR when every remote was excluded by the per-repo scope.
#[must_use]
pub fn canonical_remote_in_scope<'a>(
    repo: &'a OrgRepo,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
) -> Option<&'a Remote> {
    filter_remotes_by_forges(repo, provider_map).into_iter().next()
}

/// V5PARITY-34: did the per-repo `forges` scope exclude every remote?
/// Used by routing methods to emit `forge_excluded` rather than a
/// generic `no_remotes` error.
#[must_use]
pub fn all_remotes_excluded(repo: &OrgRepo, provider_map: &BTreeMap<DomainName, ProviderKind>) -> bool {
    !repo.remotes.is_empty() && filter_remotes_by_forges(repo, provider_map).is_empty()
}

/// Does this remote exist on its forge?
pub async fn exists_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
    fallback_token_ref: Option<String>,
) -> Result<bool, ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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
    fallback_token_ref: Option<String>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
    adapter.create_repo(remote, repo_ref, visibility, description, &auth).await
}

/// Delete a repo on its forge.
pub async fn delete_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
    fallback_token_ref: Option<String>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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
    fallback_token_ref: Option<String>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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
    fallback_token_ref: Option<String>,
) -> Result<Vec<crate::v5::adapters::RemoteRepo>, ForgePortError> {
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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
    fallback_token_ref: Option<String>,
) -> Result<MetadataFields, ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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
    fallback_token_ref: Option<String>,
) -> Result<(), ForgePortError> {
    let provider = derive_provider(remote, provider_map).map_err(|e| {
        ForgePortError::new(ForgeErrorClass::Network, e)
    })?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
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

/// Resolve a repo's CANONICAL owner on its forge, following any
/// rename/transfer redirect (org-diff). Thin wrapper over the adapter's
/// `canonical_owner` that mirrors `exists_on_forge`'s shape so hubs never
/// touch the adapter directly.
pub async fn canonical_owner_on_forge(
    remote: &Remote,
    repo_ref: &RepoRef,
    provider_map: &BTreeMap<DomainName, ProviderKind>,
    resolver: &dyn SecretResolver,
    token_ref: Option<&str>,
    fallback_token_ref: Option<String>,
) -> Result<String, ForgePortError> {
    let provider = derive_provider(remote, provider_map)
        .map_err(|e| ForgePortError::new(ForgeErrorClass::Network, e))?;
    let adapter = for_provider(provider);
    let auth = ForgeAuth { token_ref, fallback_token_ref, resolver };
    adapter.canonical_owner(remote, repo_ref, &auth).await
}

// ---------------------------------------------------------------------
// Org-diff — discover which repos moved between two orgs on the forge.
// Read-only; the bucketing is pure so it unit-tests against an injected
// resolver. The `moved` bucket is the exact `members` argument an
// operator then feeds to `workspaces.migrate_org`.
// ---------------------------------------------------------------------

/// Bucket a single repo lands in after resolving its canonical owner
/// against the forge, relative to a `(from_org, to_org)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgDiffVerdict {
    /// Canonical owner == `to_org`: the repo moved as intended.
    Moved,
    /// Canonical owner == `from_org`: the repo did NOT move.
    Stayed,
    /// Canonical owner is some third org/user.
    Elsewhere,
    /// Repo not found on the forge at all.
    Missing,
}

impl OrgDiffVerdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::Stayed => "stayed",
            Self::Elsewhere => "elsewhere",
            Self::Missing => "missing",
        }
    }

    /// Pure classifier: given the forge's canonical-owner answer for a
    /// repo (`Ok(owner)` or `Err(class)`), bucket it. A `not_found`
    /// forge error → `Missing`; any other forge error → `Elsewhere`
    /// is NOT assumed — errors other than not_found are surfaced by the
    /// caller as a per-repo error event, so this only handles the
    /// resolved-owner and not-found cases.
    #[must_use]
    pub fn classify(canonical_owner: &str, from_org: &str, to_org: &str) -> Self {
        if canonical_owner == to_org {
            Self::Moved
        } else if canonical_owner == from_org {
            Self::Stayed
        } else {
            Self::Elsewhere
        }
    }

    /// Two-sided classifier (DEFECT 56f3fc38). When the `from_org`
    /// canonical-owner lookup returns `not_found`, we CANNOT conclude the
    /// repo is gone: a private repo that was transferred to `to_org`
    /// 301-redirects, but if the source token lacks read access at the
    /// destination GitHub returns 404 (hides existence). So we probe
    /// `to_org` directly with `to_org`'s OWN credentials. If it exists
    /// there, the repo MOVED; only when it is absent at BOTH ends is it
    /// genuinely MISSING.
    #[must_use]
    pub const fn classify_not_found_at_source(exists_at_to_org: bool) -> Self {
        if exists_at_to_org {
            Self::Moved
        } else {
            Self::Missing
        }
    }
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
    let repo_ref = RepoRef {
        org: org.name.clone(),
        name: repo.name.clone(),
    };
    // V5PARITY-34: scope by per-repo `forges` first, then narrow by
    // explicit URL filter if the caller asked for one.
    let scoped: Vec<&Remote> = filter_remotes_by_forges(repo, provider_map);
    let filtered: Vec<&Remote> = if let Some(f) = remote_filter.filter(|s| !s.is_empty()) {
        scoped.into_iter().filter(|r| r.url.as_str() == f).collect()
    } else {
        scoped
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
        // MFORGE-3: resolve credentials per-remote, not once for the repo.
        let pk = provider.unwrap();
        let tref = token_ref_for_provider(org, pk);
        let fallback = Some(default_token_ref_for_provider(pk));
        let adapter = for_provider(pk);
        let auth = ForgeAuth {
            token_ref: tref,
            fallback_token_ref: fallback,
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
    md.lifecycle = crate::v5::config::RepoLifecycle::Dismissed;
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
    let protected = md.is_some_and(|m| m.protected);
    if protected {
        return Err(PurgeError::Protected);
    }
    let lifecycle = md.map_or(crate::v5::config::RepoLifecycle::Active, |m| m.lifecycle);
    if lifecycle != crate::v5::config::RepoLifecycle::Dismissed {
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

// ---------------------------------------------------------------------
// Tests (MFORGE-3).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v5::config::{
        CredentialEntry, CredentialType, ForgeProviderBlock, OrgConfig, OrgName, ProviderKind,
    };
    use std::collections::BTreeMap;

    /// Helper: build an `OrgConfig` from a map of provider → credentials.
    fn org_with(entries: Vec<(ProviderKind, Vec<CredentialEntry>)>) -> OrgConfig {
        let mut forges = BTreeMap::new();
        for (pk, creds) in entries {
            forges.insert(pk, ForgeProviderBlock { credentials: creds });
        }
        OrgConfig {
            name: OrgName("test".into()),
            forges,
            repos: vec![],
        }
    }

    fn token_cred(key: &str) -> CredentialEntry {
        CredentialEntry {
            key: key.to_string(),
            cred_type: CredentialType::Token,
        }
    }

    fn ssh_cred(key: &str) -> CredentialEntry {
        CredentialEntry {
            key: key.to_string(),
            cred_type: CredentialType::SshKey,
        }
    }

    #[test]
    fn test_token_ref_for_provider_github() {
        let org = org_with(vec![
            (ProviderKind::Github, vec![token_cred("secrets://gh")]),
        ]);
        assert_eq!(
            token_ref_for_provider(&org, ProviderKind::Github),
            Some("secrets://gh"),
        );
        assert_eq!(
            token_ref_for_provider(&org, ProviderKind::Codeberg),
            None,
        );
    }

    #[test]
    fn test_token_ref_for_provider_multi() {
        let org = org_with(vec![
            (ProviderKind::Github, vec![token_cred("secrets://gh")]),
            (ProviderKind::Codeberg, vec![token_cred("secrets://cb")]),
        ]);
        assert_eq!(
            token_ref_for_provider(&org, ProviderKind::Github),
            Some("secrets://gh"),
        );
        assert_eq!(
            token_ref_for_provider(&org, ProviderKind::Codeberg),
            Some("secrets://cb"),
        );
        assert_eq!(
            token_ref_for_provider(&org, ProviderKind::Gitlab),
            None,
        );
    }

    #[test]
    fn test_default_token_ref_for_provider() {
        assert_eq!(
            default_token_ref_for_provider(ProviderKind::Github),
            "secrets://github/_default/token",
        );
        assert_eq!(
            default_token_ref_for_provider(ProviderKind::Codeberg),
            "secrets://codeberg/_default/token",
        );
        assert_eq!(
            default_token_ref_for_provider(ProviderKind::Gitlab),
            "secrets://gitlab/_default/token",
        );
    }

    #[test]
    fn test_ssh_key_for_provider() {
        let org = org_with(vec![
            (
                ProviderKind::Github,
                vec![
                    token_cred("secrets://gh"),
                    ssh_cred("~/.ssh/id_ed25519"),
                ],
            ),
            (ProviderKind::Codeberg, vec![token_cred("secrets://cb")]),
        ]);
        assert_eq!(
            ssh_key_for_provider(&org, ProviderKind::Github),
            Some("~/.ssh/id_ed25519"),
        );
        assert_eq!(
            ssh_key_for_provider(&org, ProviderKind::Codeberg),
            None,
        );
    }

    #[test]
    fn test_backward_compat_wrappers() {
        // primary_provider is Github (first in BTreeMap order).
        let org = org_with(vec![
            (ProviderKind::Github, vec![token_cred("secrets://gh")]),
            (ProviderKind::Codeberg, vec![token_cred("secrets://cb")]),
        ]);
        // token_ref_for delegates to primary (Github).
        assert_eq!(token_ref_for(&org), Some("secrets://gh"));
        // default_token_ref_for delegates to primary (Github).
        assert_eq!(
            default_token_ref_for(&org),
            "secrets://github/_default/token",
        );
    }

    #[test]
    fn test_org_diff_verdict_classify() {
        use super::OrgDiffVerdict as V;
        // moved: canonical owner is the target org.
        assert_eq!(
            V::classify("hypermemetic-ai", "hypermemetic", "hypermemetic-ai"),
            V::Moved,
        );
        // stayed: canonical owner is still the source org.
        assert_eq!(
            V::classify("hypermemetic", "hypermemetic", "hypermemetic-ai"),
            V::Stayed,
        );
        // elsewhere: some third owner.
        assert_eq!(
            V::classify("someone-else", "hypermemetic", "hypermemetic-ai"),
            V::Elsewhere,
        );
    }

    #[test]
    fn test_org_diff_verdict_classify_not_found_at_source() {
        use super::OrgDiffVerdict as V;
        // DEFECT 56f3fc38: a private repo transferred to `to_org` whose
        // canonical-owner lookup 404s at `from_org` (source token lacks
        // destination access) is recovered as MOVED once the two-sided
        // probe confirms it exists at `to_org`.
        assert_eq!(V::classify_not_found_at_source(true), V::Moved);
        // Absent at both ends => genuinely MISSING.
        assert_eq!(V::classify_not_found_at_source(false), V::Missing);
    }

    #[test]
    fn test_org_diff_verdict_as_str() {
        use super::OrgDiffVerdict as V;
        assert_eq!(V::Moved.as_str(), "moved");
        assert_eq!(V::Stayed.as_str(), "stayed");
        assert_eq!(V::Elsewhere.as_str(), "elsewhere");
        assert_eq!(V::Missing.as_str(), "missing");
    }

    /// The discovery→migration composition: bucketing 53 declared repos
    /// where only some moved yields exactly the `moved` set that
    /// `workspaces.migrate_org` should receive as `members` — never
    /// over-migrating the ones that stayed.
    #[test]
    fn test_org_diff_buckets_partial_migration() {
        use super::OrgDiffVerdict as V;
        let from = "hypermemetic";
        let to = "hypermemetic-ai";
        // Forge truth per repo. `Ok(owner)` is a resolved canonical owner;
        // `Err(exists_at_to)` models a `not_found` at `from_org` plus the
        // two-sided probe's answer at `to_org` (DEFECT 56fc3f38): a private
        // repo whose source token lacks destination access 404s at the
        // source but is recovered via the `to_org` probe.
        let forge_truth: [(&str, Result<&str, bool>); 6] = [
            ("alpha", Ok("hypermemetic-ai")), // moved (resolved)
            ("beta", Ok("hypermemetic")),     // stayed
            ("gamma", Ok("hypermemetic-ai")), // moved (resolved)
            ("delta", Ok("third-party")),     // elsewhere
            ("epsilon", Err(true)),  // transferred private: 404 at source, exists at to => moved
            ("zeta", Err(false)),    // absent at both ends => missing
        ];
        let mut moved = Vec::new();
        let mut stayed = Vec::new();
        let mut elsewhere = Vec::new();
        let mut missing = Vec::new();
        for (repo, truth) in forge_truth {
            let verdict = match truth {
                Ok(owner) => V::classify(owner, from, to),
                Err(exists_at_to) => V::classify_not_found_at_source(exists_at_to),
            };
            match verdict {
                V::Moved => moved.push(repo),
                V::Stayed => stayed.push(repo),
                V::Elsewhere => elsewhere.push(repo),
                V::Missing => missing.push(repo),
            }
        }
        // The `moved` set is exactly what migrate_org's members must be —
        // including the transferred private repo recovered by the probe.
        assert_eq!(moved, vec!["alpha", "gamma", "epsilon"]);
        assert_eq!(stayed, vec!["beta"]);
        assert_eq!(elsewhere, vec!["delta"]);
        assert_eq!(missing, vec!["zeta"]);
    }

    #[test]
    fn test_backward_compat_empty_org() {
        let org = OrgConfig {
            name: OrgName("empty".into()),
            forges: BTreeMap::new(),
            repos: vec![],
        };
        assert_eq!(token_ref_for(&org), None);
        // Falls back to Github when no providers.
        assert_eq!(
            default_token_ref_for(&org),
            "secrets://github/_default/token",
        );
    }
}
