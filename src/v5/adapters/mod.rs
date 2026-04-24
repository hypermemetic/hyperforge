//! `ForgePort` trait and provider adapters for the v5 repos surface.
//!
//! The trait (V5REPOS-2) is the portable intersection pinned in CONTRACTS
//! §decisions D3: `{default_branch, description, archived, visibility}`.
//! Adapters implement this trait against concrete provider APIs; they
//! MAY read more internally but MUST NOT leak provider-specific fields
//! through the trait's wire surface.

pub mod codeberg;
pub mod github;
pub mod gitlab;

use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::config::{ProviderKind, Remote, RepoRef};
use crate::v5::secrets::SecretResolver;

// ---------------------------------------------------------------------
// Types at the trait boundary.
// ---------------------------------------------------------------------

/// Portable metadata fields intersected across GitHub/Codeberg/GitLab.
/// Pinned per D3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriftFieldKind {
    DefaultBranch,
    Description,
    Archived,
    Visibility,
}

impl DriftFieldKind {
    /// Wire-surface name for this field (`snake_case`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefaultBranch => "default_branch",
            Self::Description => "description",
            Self::Archived => "archived",
            Self::Visibility => "visibility",
        }
    }

    /// Enumerate every variant in a stable order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Archived,
            Self::DefaultBranch,
            Self::Description,
            Self::Visibility,
        ]
    }
}

/// Read-side metadata: all four fields present (per D3 intersection).
///
/// Values are deliberately typed as `serde_json::Value` at this
/// boundary so tri-state vs boolean visibility variants flow through
/// uniformly while still yielding the typed wire shape consumers check
/// for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
pub struct ForgeMetadata {
    pub default_branch: String,
    pub description: String,
    pub archived: bool,
    /// Provider-dependent string: `public`/`private` (GitHub/Codeberg)
    /// or `public`/`internal`/`private` (GitLab).
    pub visibility: String,
}

/// Write-side field map: only declared fields are applied; absent fields
/// untouched on the remote.
pub type MetadataFields = BTreeMap<DriftFieldKind, serde_json::Value>;

/// Closed error class set for v1.
///
/// Original five are per V5REPOS-2. `Conflict` and
/// `UnsupportedVisibility` were added by V5PROV-2 (D10) for the three
/// lifecycle methods on `ForgePort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeErrorClass {
    NotFound,
    Auth,
    Network,
    UnsupportedField,
    RateLimited,
    /// create_repo only — the repo already exists on the remote.
    Conflict,
    /// create_repo only — the provider does not support the requested
    /// visibility variant (e.g., `internal` on github.com/codeberg.org).
    UnsupportedVisibility,
}

impl ForgeErrorClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Auth => "auth",
            Self::Network => "network",
            Self::UnsupportedField => "unsupported_field",
            Self::RateLimited => "rate_limited",
            Self::Conflict => "conflict",
            Self::UnsupportedVisibility => "unsupported_visibility",
        }
    }
}

/// Typed error carried at the wire boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgePortError {
    pub class: ForgeErrorClass,
    pub message: String,
}

impl ForgePortError {
    #[must_use]
    pub fn new(class: ForgeErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::NotFound, msg)
    }

    #[must_use]
    pub fn auth(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::Auth, msg)
    }

    #[must_use]
    pub fn network(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::Network, msg)
    }

    #[must_use]
    pub fn unsupported_field(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::UnsupportedField, msg)
    }

    #[must_use]
    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::RateLimited, msg)
    }

    #[must_use]
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::Conflict, msg)
    }

    #[must_use]
    pub fn unsupported_visibility(msg: impl Into<String>) -> Self {
        Self::new(ForgeErrorClass::UnsupportedVisibility, msg)
    }
}

impl std::fmt::Display for ForgePortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class.as_str(), self.message)
    }
}

impl std::error::Error for ForgePortError {}

// ---------------------------------------------------------------------
// Trait.
// ---------------------------------------------------------------------

/// Authentication hint passed to adapters. Adapters resolve the secret
/// through the `SecretResolver` at call-time; the resolved plaintext
/// never leaves the adapter.
#[derive(Clone)]
pub struct ForgeAuth<'a> {
    /// `secrets://…` reference. Adapter calls `resolver.resolve(...)`.
    pub token_ref: Option<&'a str>,
    pub resolver: &'a dyn SecretResolver,
}

/// `ProviderVisibility` (per CONTRACTS §types).
///
/// Adapters reject the variants their provider lacks by returning
/// `ForgePortError { class: unsupported_visibility }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVisibility {
    Public,
    Private,
    Internal,
}

impl ProviderVisibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
        }
    }

    /// Case-insensitive parse of `public` / `private` / `internal`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            "internal" => Ok(Self::Internal),
            other => Err(format!(
                "invalid visibility '{other}'; allowed: public, private, internal"
            )),
        }
    }
}

/// Portable capability trait over the three forges.
///
/// Read/write the D3 metadata intersection, plus the three lifecycle
/// methods pinned by V5PROV-2 (D10): `create_repo`, `delete_repo`,
/// `repo_exists`. Adapters MAY read additional fields internally; they
/// MUST NOT leak provider-specific shapes through this trait.
#[async_trait]
pub trait ForgePort: Send + Sync {
    /// Provider variant this adapter handles.
    fn provider(&self) -> ProviderKind;

    /// Read portable metadata for `(remote, repo_ref)`.
    async fn read_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<ForgeMetadata, ForgePortError>;

    /// Write portable metadata. Only `fields` keys are applied.
    async fn write_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        fields: &MetadataFields,
        auth: &ForgeAuth<'_>,
    ) -> Result<MetadataFields, ForgePortError>;

    /// Create the repo on the remote.
    ///
    /// On an already-existing repo, returns
    /// `ForgePortError { class: conflict }`. On an
    /// adapter-unsupported `visibility`, returns
    /// `ForgePortError { class: unsupported_visibility }` without
    /// issuing the API call.
    async fn create_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        visibility: ProviderVisibility,
        description: &str,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError>;

    /// Delete the repo on the remote.
    ///
    /// On a missing repo, returns
    /// `ForgePortError { class: not_found }` (not a silent success —
    /// callers distinguish "already gone" from "auth fails" via the
    /// error class).
    async fn delete_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError>;

    /// Probe whether the remote repo exists and is reachable with the
    /// given credentials. `Ok(true)` = exists and readable;
    /// `Ok(false)` = doesn't exist; `Err { class: auth }` = we can't
    /// even check.
    async fn repo_exists(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<bool, ForgePortError>;
}

// ---------------------------------------------------------------------
// Dispatch.
// ---------------------------------------------------------------------

/// Select the adapter for a given `ProviderKind`.
#[must_use]
pub fn for_provider(kind: ProviderKind) -> Box<dyn ForgePort> {
    match kind {
        ProviderKind::Github => Box::new(github::GithubAdapter::new()),
        ProviderKind::Codeberg => Box::new(codeberg::CodebergAdapter::new()),
        ProviderKind::Gitlab => Box::new(gitlab::GitlabAdapter::new()),
    }
}

// ---------------------------------------------------------------------
// Shared helpers used by concrete adapters.
// ---------------------------------------------------------------------

/// Extract host portion of a `Remote`'s URL. Accepts:
/// * `https://host/path…`
/// * `http://host/path…`
/// * `ssh://user@host/path…`
/// * `git@host:owner/name…` (SCP-like form)
#[must_use]
pub fn extract_host(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if let Some(rest) = trimmed.strip_prefix("https://") {
        return Some(host_segment(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        return Some(host_segment(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://") {
        return Some(host_segment(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("git://") {
        return Some(host_segment(rest));
    }
    // SCP form: user@host:path (first `:` before first `/` in host part)
    if let Some(at) = trimmed.find('@') {
        let after_at = &trimmed[at + 1..];
        if let Some(colon) = after_at.find(':') {
            let maybe_host = &after_at[..colon];
            if !maybe_host.is_empty() && !maybe_host.contains('/') {
                return Some(maybe_host.to_lowercase());
            }
        }
    }
    None
}

fn host_segment(rest: &str) -> String {
    // Strip any `user@` prefix, then cut at first `/` or `:`.
    let after_userinfo = rest.rsplit_once('@').map_or(rest, |(_u, h)| h);
    let end = after_userinfo
        .find([':', '/'])
        .unwrap_or(after_userinfo.len());
    after_userinfo[..end].to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_https() {
        assert_eq!(
            extract_host("https://github.com/demo/widget.git").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn host_scp() {
        assert_eq!(
            extract_host("git@github.com:demo/widget.git").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn host_ssh() {
        assert_eq!(
            extract_host("ssh://git@codeberg.org/demo/widget.git").as_deref(),
            Some("codeberg.org")
        );
    }

    #[test]
    fn host_lowercase() {
        assert_eq!(
            extract_host("https://GitHub.com/demo/widget.git").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn host_unparseable_relative() {
        assert!(extract_host("../foo.git").is_none());
    }

    #[test]
    fn drift_field_all_is_sorted() {
        // `all()` returns in alphabetical order; the capability schema
        // event surfaces them sorted.
        let names: Vec<&'static str> =
            DriftFieldKind::all().iter().map(|k| k.as_str()).collect();
        assert_eq!(
            names,
            vec!["archived", "default_branch", "description", "visibility"]
        );
    }
}
