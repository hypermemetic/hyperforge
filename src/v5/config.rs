//! YAML config loaders for the three v5 schemas (V5CORE-3).
//!
//! * Global `config.yaml`
//! * Per-org `orgs/<OrgName>.yaml`
//! * Per-workspace `workspaces/<WorkspaceName>.yaml`
//!
//! All types come from CONTRACTS §types. Unknown top-level fields,
//! unknown enum variants, and basename/name mismatches are hard errors.
//! Writes are atomic (temp + rename) per D8.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------
// §types — newtypes and enums referenced by the composite schemas.
// ---------------------------------------------------------------------

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            Hash,
            Serialize,
            Deserialize,
            PartialOrd,
            Ord,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_newtype!(OrgName);
string_newtype!(RepoName);
string_newtype!(WorkspaceName);
string_newtype!(RemoteUrl);
string_newtype!(DomainName);
string_newtype!(FsPath);

/// Forge providers known to v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Github,
    Codeberg,
    Gitlab,
}

/// Credential kinds known to v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    Token,
    SshKey,
}

/// `CredentialEntry { key, type }`. `key` is either a `secrets://…` ref
/// or an `FsPath`; both serialize as a bare string, so the wire form is
/// `{key: "...", type: "token"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialEntry {
    pub key: String,
    #[serde(rename = "type")]
    pub cred_type: CredentialType,
}

/// Remote URL, optionally overriding the domain → provider map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub url: RemoteUrl,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider: Option<ProviderKind>,
}

/// `RepoRef { org, name }`.
///
/// Serialized canonically as a JSON object `{org, name}`. For YAML
/// convenience, `Deserialize` also accepts the human-friendly string
/// form `<org>/<name>` — used inside workspace yaml `ref:` fields and
/// on-the-wire wherever callers find the object form redundant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepoRef {
    pub org: OrgName,
    pub name: RepoName,
}

impl<'de> Deserialize<'de> for RepoRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Accept either the canonical object `{org, name}` or the
        // shorthand string `<org>/<name>`.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Object { org: OrgName, name: RepoName },
            String(String),
        }
        let v = Either::deserialize(deserializer)?;
        match v {
            Either::Object { org, name } => Ok(Self { org, name }),
            Either::String(s) => {
                let (org, name) = s.split_once('/').ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "invalid RepoRef shorthand (expected '<org>/<name>'): {s}"
                    ))
                })?;
                if org.is_empty() || name.is_empty() {
                    return Err(serde::de::Error::custom(format!(
                        "invalid RepoRef shorthand (empty segment): {s}"
                    )));
                }
                Ok(Self {
                    org: org.into(),
                    name: name.into(),
                })
            }
        }
    }
}

/// A repo declared on an org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgRepo {
    pub name: RepoName,
    #[serde(default)]
    pub remotes: Vec<Remote>,
    /// V5PARITY-34: when present, scopes which forges this repo lives
    /// on. Routing ops (sync, push, set_archived, …) intersect this
    /// list with the providers derived from `remotes`. `None` =
    /// unscoped; every remote's provider participates (legacy
    /// behavior, byte-identical for fixtures that don't carry the
    /// field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forges: Option<Vec<ProviderKind>>,
    /// Optional declared-local values for the D3 portable metadata set.
    /// When absent, `repos.sync` treats the local side as "unknown" for
    /// that field and reports drift only against the declared keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<RepoMetadataLocal>,
}

/// Local declaration of portable metadata. Fields are optional; only
/// declared fields participate in drift comparisons / pushes.
///
/// V5LIFECYCLE-5 added `lifecycle`, `privatized_on`, `protected`.
/// These default such that an absent `metadata:` block OR a metadata
/// block without the new fields round-trips byte-identically after a
/// load → save cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepoMetadataLocal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// `active` (normal) | `dismissed` (soft-deleted). `purged` is a
    /// transient — a purged repo is removed from the org yaml. The
    /// default is `Active`; serialized as absent so repos that never
    /// went through soft-delete stay byte-identical on round-trip.
    #[serde(default, skip_serializing_if = "RepoLifecycle::is_default")]
    pub lifecycle: RepoLifecycle,
    /// Providers where privatization succeeded during soft-delete.
    /// Empty set → serialized as absent (skip_serializing_if).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub privatized_on: std::collections::BTreeSet<ProviderKind>,
    /// When true, `repos.delete` and `repos.purge` refuse this repo.
    /// Default false is serialized as absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protected: bool,
}

/// Lifecycle phases a repo can occupy. See D12. `Active` is the
/// default — V5PARITY-12 upgraded this from `Option<RepoLifecycle>` to
/// a required field so "unset vs Active" is no longer representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepoLifecycle {
    #[default]
    Active,
    Dismissed,
}

impl RepoLifecycle {
    /// Used by `RepoMetadataLocal` via `skip_serializing_if` to keep
    /// `Active` (the default) absent on the wire.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Active)
    }
}

impl OrgRepo {
    /// The canonical / primary remote — by position, `remotes[0]`.
    /// v5 doesn't model primary/mirror as a type-level split; the
    /// position convention is enforced via this helper so the intent
    /// is visible at call sites.
    #[must_use]
    pub fn canonical_remote(&self) -> Option<&Remote> {
        self.remotes.first()
    }
}

/// A workspace entry, either a string shorthand `<org>/<name>` or a
/// `{ref, dir}` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WorkspaceRepo {
    Shorthand(String),
    Object {
        #[serde(rename = "ref")]
        reference: RepoRef,
        dir: String,
    },
}

// ---------------------------------------------------------------------
// Schema roots.
// ---------------------------------------------------------------------

/// Global `config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace: Option<WorkspaceName>,
    #[serde(default)]
    pub provider_map: BTreeMap<DomainName, ProviderKind>,
}

/// Legacy org forge block — used only for backward-compatible
/// deserialization of the old `forge:` YAML key. Not stored on
/// `OrgConfig`; converted to a single-entry `BTreeMap` during deser.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeBlock {
    pub provider: ProviderKind,
    #[serde(default)]
    pub credentials: Vec<CredentialEntry>,
}

/// Per-provider credential block. The provider identity is the map key
/// in `OrgConfig.forges`, so it is not duplicated inside the block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeProviderBlock {
    #[serde(default)]
    pub credentials: Vec<CredentialEntry>,
}

/// Per-org `orgs/<name>.yaml` schema root.
///
/// `forges` is the canonical field (serialized as `forges:`). For
/// backward compatibility the deserializer also accepts the legacy
/// `forge:` single-provider form and promotes it to a single-entry map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgConfig {
    pub name: OrgName,
    pub forges: BTreeMap<ProviderKind, ForgeProviderBlock>,
    pub repos: Vec<OrgRepo>,
}

// -- Convenience accessors on OrgConfig -----------------------------------

impl OrgConfig {
    /// All configured provider kinds (map-key order, i.e. `Ord` order).
    pub fn providers(&self) -> impl Iterator<Item = ProviderKind> + '_ {
        self.forges.keys().copied()
    }

    /// The "primary" provider — the first key in the `BTreeMap`.
    /// Returns `None` only when the map is empty (which valid configs
    /// should never have, but callers can handle gracefully).
    #[must_use]
    pub fn primary_provider(&self) -> Option<ProviderKind> {
        self.forges.keys().next().copied()
    }

    /// Credentials for a specific provider. Empty slice when the
    /// provider is absent.
    #[must_use]
    pub fn credentials_for(&self, provider: ProviderKind) -> &[CredentialEntry] {
        self.forges
            .get(&provider)
            .map(|b| b.credentials.as_slice())
            .unwrap_or(&[])
    }

    /// Iterate `(provider, &CredentialEntry)` across all providers.
    pub fn all_credentials(&self) -> impl Iterator<Item = (ProviderKind, &CredentialEntry)> {
        self.forges
            .iter()
            .flat_map(|(&p, b)| b.credentials.iter().map(move |c| (p, c)))
    }

    /// Mutable access to the primary provider's credential list.
    /// Panics if no providers are configured.
    pub fn primary_credentials_mut(&mut self) -> &mut Vec<CredentialEntry> {
        let pk = self.primary_provider().expect("OrgConfig has no providers");
        &mut self.forges.get_mut(&pk).expect("primary exists").credentials
    }
}

// -- Custom Serialize / Deserialize for OrgConfig -------------------------

impl Serialize for OrgConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            name: &'a OrgName,
            forges: &'a BTreeMap<ProviderKind, ForgeProviderBlock>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            repos: &'a Vec<OrgRepo>,
        }
        Wire {
            name: &self.name,
            forges: &self.forges,
            repos: &self.repos,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OrgConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Raw helper accepting either `forge` or `forges`.
        #[derive(Deserialize)]
        struct Raw {
            name: OrgName,
            #[serde(default)]
            forge: Option<ForgeBlock>,
            #[serde(default)]
            forges: Option<BTreeMap<ProviderKind, ForgeProviderBlock>>,
            #[serde(default)]
            repos: Vec<OrgRepo>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let forges = match (raw.forge, raw.forges) {
            (Some(_), Some(_)) => {
                return Err(serde::de::Error::custom(
                    "ambiguous: both `forge` and `forges` present; use only `forges`",
                ));
            }
            (None, None) => {
                return Err(serde::de::Error::custom(
                    "missing field: expected `forge` (legacy) or `forges`",
                ));
            }
            (Some(legacy), None) => {
                let mut map = BTreeMap::new();
                map.insert(
                    legacy.provider,
                    ForgeProviderBlock {
                        credentials: legacy.credentials,
                    },
                );
                map
            }
            (None, Some(map)) => map,
        };
        Ok(OrgConfig {
            name: raw.name,
            forges,
            repos: raw.repos,
        })
    }
}

/// Per-workspace `workspaces/<name>.yaml` schema root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub name: WorkspaceName,
    pub path: FsPath,
    #[serde(default)]
    pub repos: Vec<WorkspaceRepo>,
}

// ---------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid yaml in {file}: {source}")]
    InvalidYaml {
        file: String,
        #[source]
        source: serde_yaml::Error,
    },
    #[error(
        "basename/name mismatch in orgs/{file}: expected 'name: {file}' but got 'name: {found}'"
    )]
    OrgNameMismatch { file: String, found: String },
    #[error(
        "basename/name mismatch in workspaces/{file}: expected 'name: {file}' but got 'name: {found}'"
    )]
    WorkspaceNameMismatch { file: String, found: String },
    #[error("I/O error for {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------
// Loaders.
// ---------------------------------------------------------------------

/// Everything a v5 daemon knows about on-disk state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedConfig {
    pub global: GlobalConfig,
    /// Orgs keyed by name for O(1) lookup.
    pub orgs: BTreeMap<OrgName, OrgConfig>,
    pub workspaces: BTreeMap<WorkspaceName, WorkspaceConfig>,
}

/// Load every YAML file under `config_dir` into a typed bundle.
/// Missing `config.yaml` → default empty config. Missing `orgs/` or
/// `workspaces/` → empty maps. All other errors propagate.
pub fn load_all(config_dir: &Path) -> Result<LoadedConfig, ConfigError> {
    let global = load_global(&config_dir.join("config.yaml"))?;

    let orgs_dir = config_dir.join("orgs");
    let orgs = if orgs_dir.is_dir() {
        load_orgs(&orgs_dir)?
    } else {
        BTreeMap::new()
    };

    let ws_dir = config_dir.join("workspaces");
    let workspaces = if ws_dir.is_dir() {
        load_workspaces(&ws_dir)?
    } else {
        BTreeMap::new()
    };

    Ok(LoadedConfig {
        global,
        orgs,
        workspaces,
    })
}

/// Load `config.yaml`. Missing → default empty config.
pub fn load_global(path: &Path) -> Result<GlobalConfig, ConfigError> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(GlobalConfig::default()),
        Err(e) => {
            return Err(ConfigError::Io {
                file: path.display().to_string(),
                source: e,
            });
        }
    };
    // serde_yaml silently returns `Null` for an empty document; normalise.
    if raw.trim().is_empty() {
        return Ok(GlobalConfig::default());
    }
    serde_yaml::from_str(&raw).map_err(|e| ConfigError::InvalidYaml {
        file: path.display().to_string(),
        source: e,
    })
}

/// Load every `orgs/*.yaml` in the directory.
pub fn load_orgs(dir: &Path) -> Result<BTreeMap<OrgName, OrgConfig>, ConfigError> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|e| ConfigError::Io {
        file: dir.display().to_string(),
        source: e,
    })? {
        let entry = entry.map_err(|e| ConfigError::Io {
            file: dir.display().to_string(),
            source: e,
        })?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let basename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let raw = fs::read_to_string(&path).map_err(|e| ConfigError::Io {
            file: path.display().to_string(),
            source: e,
        })?;
        let cfg: OrgConfig = serde_yaml::from_str(&raw).map_err(|e| ConfigError::InvalidYaml {
            file: path.display().to_string(),
            source: e,
        })?;
        if cfg.name.as_str() != basename {
            return Err(ConfigError::OrgNameMismatch {
                file: basename,
                found: cfg.name.0,
            });
        }
        out.insert(cfg.name.clone(), cfg);
    }
    Ok(out)
}

/// Load every `workspaces/*.yaml` in the directory.
pub fn load_workspaces(dir: &Path) -> Result<BTreeMap<WorkspaceName, WorkspaceConfig>, ConfigError> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|e| ConfigError::Io {
        file: dir.display().to_string(),
        source: e,
    })? {
        let entry = entry.map_err(|e| ConfigError::Io {
            file: dir.display().to_string(),
            source: e,
        })?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let basename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let raw = fs::read_to_string(&path).map_err(|e| ConfigError::Io {
            file: path.display().to_string(),
            source: e,
        })?;
        let cfg: WorkspaceConfig =
            serde_yaml::from_str(&raw).map_err(|e| ConfigError::InvalidYaml {
                file: path.display().to_string(),
                source: e,
            })?;
        if cfg.name.as_str() != basename {
            return Err(ConfigError::WorkspaceNameMismatch {
                file: basename,
                found: cfg.name.0,
            });
        }
        out.insert(cfg.name.clone(), cfg);
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Writers — atomic (temp + rename) per D8.
// ---------------------------------------------------------------------

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
            file: parent.display().to_string(),
            source: e,
        })?;
    }
    let tmp = tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    fs::write(&tmp, bytes).map_err(|e| ConfigError::Io {
        file: tmp.display().to_string(),
        source: e,
    })?;
    fs::rename(&tmp, path).map_err(|e| ConfigError::Io {
        file: path.display().to_string(),
        source: e,
    })
}

fn tempfile_in(dir: &Path) -> Result<PathBuf, ConfigError> {
    let mut rng = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for _ in 0..32 {
        let name = format!(".v5-atomic-{rng:x}");
        let p = dir.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                rng = rng.wrapping_add(1);
            }
            Err(e) => {
                return Err(ConfigError::Io {
                    file: p.display().to_string(),
                    source: e,
                });
            }
        }
    }
    Err(ConfigError::Io {
        file: dir.display().to_string(),
        source: std::io::Error::other("failed to allocate temp file after 32 tries"),
    })
}

/// Write `config.yaml` atomically.
pub fn save_global(path: &Path, cfg: &GlobalConfig) -> Result<(), ConfigError> {
    let bytes = serde_yaml::to_string(cfg)
        .map_err(|e| ConfigError::InvalidYaml {
            file: path.display().to_string(),
            source: e,
        })?
        .into_bytes();
    write_atomic(path, &bytes)
}

/// Write `orgs/<name>.yaml` atomically.
pub fn save_org(dir: &Path, cfg: &OrgConfig) -> Result<(), ConfigError> {
    let path = dir.join(format!("{}.yaml", cfg.name));
    let bytes = serde_yaml::to_string(cfg)
        .map_err(|e| ConfigError::InvalidYaml {
            file: path.display().to_string(),
            source: e,
        })?
        .into_bytes();
    write_atomic(&path, &bytes)
}

/// Write `workspaces/<name>.yaml` atomically.
pub fn save_workspace(dir: &Path, cfg: &WorkspaceConfig) -> Result<(), ConfigError> {
    let path = dir.join(format!("{}.yaml", cfg.name));
    let bytes = serde_yaml::to_string(cfg)
        .map_err(|e| ConfigError::InvalidYaml {
            file: path.display().to_string(),
            source: e,
        })?
        .into_bytes();
    write_atomic(&path, &bytes)
}

// ---------------------------------------------------------------------
// Tests — exercise round-trip, error cases, and fixtures.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("v5")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn empty_fixture_loads() {
        let cfg = load_all(&fixture_dir("empty")).expect("empty fixture loads");
        assert!(cfg.global.default_workspace.is_none());
        assert!(cfg.global.provider_map.is_empty());
        assert!(cfg.orgs.is_empty());
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn minimal_org_fixture_loads() {
        let cfg = load_all(&fixture_dir("minimal_org")).expect("minimal_org loads");
        assert_eq!(cfg.orgs.len(), 1);
        let demo = cfg.orgs.get(&OrgName("demo".into())).expect("demo org");
        assert_eq!(demo.name.as_str(), "demo");
        assert_eq!(demo.primary_provider(), Some(ProviderKind::Github));
        assert!(demo.credentials_for(ProviderKind::Github).is_empty());
        assert!(demo.repos.is_empty());
    }

    #[test]
    fn round_trip_minimal_org() {
        let first = load_all(&fixture_dir("minimal_org")).expect("load first");
        // Serialise each org, reparse, compare.
        for (_name, cfg) in &first.orgs {
            let yaml = serde_yaml::to_string(cfg).expect("serialise");
            let back: OrgConfig = serde_yaml::from_str(&yaml).expect("reparse");
            assert_eq!(&back, cfg);
        }
    }

    #[test]
    fn unknown_provider_rejected() {
        let bad = "name: demo\nforge: {provider: bitbucket, credentials: []}\nrepos: []\n";
        let err = serde_yaml::from_str::<OrgConfig>(bad).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("bitbucket") || msg.contains("unknown variant"),
            "error must name the unknown variant: {msg}"
        );
    }

    #[test]
    fn invalid_yaml_is_named() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.yaml"), "this: is: not: yaml:\n").unwrap();
        let err = load_global(&tmp.path().join("config.yaml")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("config.yaml"), "msg names config.yaml: {msg}");
    }

    #[test]
    fn basename_mismatch_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let orgs = tmp.path().join("orgs");
        std::fs::create_dir_all(&orgs).unwrap();
        std::fs::write(
            orgs.join("foo.yaml"),
            "name: bar\nforges:\n  github:\n    credentials: []\nrepos: []\n",
        )
        .unwrap();
        let err = load_orgs(&orgs).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("foo"), "msg names basename: {msg}");
        assert!(msg.contains("bar"), "msg names in-file name: {msg}");
    }

    #[test]
    fn round_trip_global() {
        let cfg = GlobalConfig {
            default_workspace: Some(WorkspaceName("main".into())),
            provider_map: [(DomainName("github.com".into()), ProviderKind::Github)]
                .into_iter()
                .collect(),
        };
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let back: GlobalConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, cfg);
    }

    // ----------------------------------------------------------------
    // MFORGE-2: multi-forge schema + backward-compatible deser tests.
    // ----------------------------------------------------------------

    #[test]
    fn test_round_trip_legacy_org() {
        let legacy = "name: demo\nforge:\n  provider: github\n  credentials:\n    - key: secrets://gh\n      type: token\nrepos: []\n";
        let org: OrgConfig = serde_yaml::from_str(legacy).expect("legacy deser");
        assert_eq!(org.primary_provider(), Some(ProviderKind::Github));
        assert_eq!(org.credentials_for(ProviderKind::Github).len(), 1);
        // Re-serialize → must use `forges:` key.
        let yaml = serde_yaml::to_string(&org).expect("ser");
        assert!(yaml.contains("forges:"), "serialized form must use 'forges:': {yaml}");
        assert!(!yaml.contains("forge:") || yaml.contains("forges:"), "must not contain bare 'forge:'");
        // Re-parse → identical.
        let back: OrgConfig = serde_yaml::from_str(&yaml).expect("reparse");
        assert_eq!(back, org);
    }

    #[test]
    fn test_round_trip_multi_forge_org() {
        let yaml = "name: multi\nforges:\n  github:\n    credentials:\n      - key: secrets://gh\n        type: token\n  codeberg:\n    credentials:\n      - key: secrets://cb\n        type: token\nrepos: []\n";
        let org: OrgConfig = serde_yaml::from_str(yaml).expect("deser");
        assert_eq!(org.forges.len(), 2);
        assert_eq!(org.credentials_for(ProviderKind::Github).len(), 1);
        assert_eq!(org.credentials_for(ProviderKind::Codeberg).len(), 1);
        // Round-trip.
        let ser = serde_yaml::to_string(&org).expect("ser");
        let back: OrgConfig = serde_yaml::from_str(&ser).expect("reparse");
        assert_eq!(back, org);
    }

    #[test]
    fn test_legacy_single_forge_to_new() {
        let legacy = "name: x\nforge:\n  provider: codeberg\n  credentials:\n    - key: secrets://cb\n      type: token\n";
        let org: OrgConfig = serde_yaml::from_str(legacy).expect("deser");
        assert_eq!(org.forges.len(), 1);
        assert_eq!(org.primary_provider(), Some(ProviderKind::Codeberg));
        assert_eq!(org.credentials_for(ProviderKind::Codeberg)[0].key, "secrets://cb");
    }

    #[test]
    fn test_both_forge_and_forges_errors() {
        let bad = "name: x\nforge:\n  provider: github\n  credentials: []\nforges:\n  github:\n    credentials: []\n";
        let err = serde_yaml::from_str::<OrgConfig>(bad).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("ambiguous"), "error must mention ambiguity: {msg}");
    }

    #[test]
    fn test_neither_forge_nor_forges_errors() {
        let bad = "name: x\nrepos: []\n";
        let err = serde_yaml::from_str::<OrgConfig>(bad).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("missing"), "error must mention missing field: {msg}");
    }

    #[test]
    fn test_existing_fixtures_still_load() {
        // All fixture dirs that contain orgs/*.yaml must load.
        for dir_name in &[
            "minimal_org",
            "org_with_credentials",
            "org_with_custom_domain_repo",
            "org_with_mirror_repo",
            "org_with_repo",
            "org_with_workspace_ref",
            "tier2-template",
            "two_orgs",
            "ws_cross_org",
            "ws_empty",
            "ws_with_one_repo",
        ] {
            let dir = fixture_dir(dir_name);
            load_all(&dir).unwrap_or_else(|e| panic!("fixture {dir_name} failed: {e}"));
        }
    }

    #[test]
    fn test_convenience_methods() {
        let yaml = "name: multi\nforges:\n  codeberg:\n    credentials:\n      - key: secrets://cb\n        type: token\n  github:\n    credentials:\n      - key: secrets://gh\n        type: token\n      - key: ~/.ssh/id_ed25519\n        type: ssh_key\n";
        let org: OrgConfig = serde_yaml::from_str(yaml).expect("deser");
        // providers() — BTreeMap order uses derived Ord: Github(0) < Codeberg(1).
        let provs: Vec<_> = org.providers().collect();
        assert_eq!(provs, vec![ProviderKind::Github, ProviderKind::Codeberg]);
        // primary_provider() — first in BTreeMap order (Github).
        assert_eq!(org.primary_provider(), Some(ProviderKind::Github));
        // credentials_for.
        assert_eq!(org.credentials_for(ProviderKind::Github).len(), 2);
        assert_eq!(org.credentials_for(ProviderKind::Codeberg).len(), 1);
        assert_eq!(org.credentials_for(ProviderKind::Gitlab).len(), 0);
        // all_credentials.
        let all: Vec<_> = org.all_credentials().collect();
        assert_eq!(all.len(), 3);
    }
}
