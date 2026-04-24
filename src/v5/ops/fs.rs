//! `ops::fs` — filesystem interactions outside the daemon's config dir
//! (V5LIFECYCLE-9). Currently just `.hyperforge/config.toml` read/write.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::config::{OrgName, ProviderKind};

/// On-disk shape of `<repo>/.hyperforge/config.toml` — CONTRACTS §types
/// `HyperforgeRepoConfig`. Narrower than v4's HyperforgeConfig; CI /
/// dist / large-file-threshold are deliberately v5-out-of-scope.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HyperforgeRepoConfig {
    pub repo_name: String,
    pub org: OrgName,
    #[serde(default)]
    pub forges: Vec<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Path layout: `<dir>/.hyperforge/config.toml`.
fn config_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".hyperforge").join("config.toml")
}

/// Read the `.hyperforge/config.toml` in `dir`. `Ok(None)` when absent;
/// `Err` on malformed TOML.
pub fn read_hyperforge_config(dir: &Path) -> Result<Option<HyperforgeRepoConfig>, InitError> {
    let path = config_path(dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(InitError::Io(e.to_string())),
    };
    toml::from_str(&raw).map(Some).map_err(|e| InitError::InvalidToml(e.to_string()))
}

/// Write a `.hyperforge/config.toml` atomically. Fails if a file
/// exists at the target and `force` is false.
pub fn write_hyperforge_config(
    dir: &Path,
    cfg: &HyperforgeRepoConfig,
    force: bool,
) -> Result<std::path::PathBuf, InitError> {
    if !dir.is_dir() {
        return Err(InitError::TargetNotDir(dir.display().to_string()));
    }
    let path = config_path(dir);
    if path.exists() && !force {
        return Err(InitError::AlreadyExists(path.display().to_string()));
    }
    let parent = path.parent().ok_or_else(|| InitError::Io("no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| InitError::Io(e.to_string()))?;
    let body = toml::to_string_pretty(cfg).map_err(|e| InitError::InvalidToml(e.to_string()))?;
    let tmp = parent.join(".hyperforge-config.toml.tmp");
    std::fs::write(&tmp, body).map_err(|e| InitError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| InitError::Io(e.to_string()))?;
    Ok(path)
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("target path is not a directory: {0}")]
    TargetNotDir(String),
    #[error("already exists at {0} — pass force=true to overwrite")]
    AlreadyExists(String),
    #[error("invalid TOML: {0}")]
    InvalidToml(String),
    #[error("io: {0}")]
    Io(String),
}

impl InitError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TargetNotDir(_) => "not_a_directory",
            Self::AlreadyExists(_) => "already_exists",
            Self::InvalidToml(_) => "invalid_toml",
            Self::Io(_) => "io_error",
        }
    }
}
