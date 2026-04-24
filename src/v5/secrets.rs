//! Embedded secret store — resolves `secrets://<path>` references.
//!
//! V5CORE-4 pins the `SecretResolver` capability: one fallible lookup
//! keyed on a `SecretRef`. v1 backend is a YAML file at
//! `$HF_CONFIG/secrets.yaml`; future backends (OS keyring, remote KMS)
//! slot in without touching callers.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Typed `secrets://<path>` reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(String);

impl SecretRef {
    /// Validate and wrap a raw string. Rejects anything that isn't
    /// `secrets://` followed by a non-empty path portion.
    pub fn parse(s: &str) -> Result<Self, SecretError> {
        let path = s
            .strip_prefix("secrets://")
            .ok_or_else(|| SecretError::InvalidRef(s.to_string()))?;
        if path.is_empty() {
            return Err(SecretError::InvalidRef(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }

    /// The `<path>` portion after the `secrets://` prefix.
    #[must_use]
    pub fn path(&self) -> &str {
        self.0.strip_prefix("secrets://").unwrap_or(&self.0)
    }

    /// The full `secrets://<path>` string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors emitted by the secret store.
#[derive(Debug, Error)]
pub enum SecretError {
    /// Reference doesn't match `secrets://<path>` (rejected before any I/O).
    #[error("invalid secret reference: {0}")]
    InvalidRef(String),
    /// Reference is valid but nothing is stored under that path.
    #[error("secret not found: {0}")]
    NotFound(String),
    /// The backing YAML file exists but can't be parsed.
    #[error("corrupted secret store {file}: {reason}")]
    Corrupt { file: String, reason: String },
    /// A key is present but its value isn't a string.
    #[error("non-string value for key '{0}' in {1}")]
    BadValue(String, String),
    /// Low-level I/O error reading the file.
    #[error("I/O error reading {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
}

impl SecretError {
    /// Wire-side error discriminator (snake_case).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRef(_) => "invalid_ref",
            Self::NotFound(_) => "not_found",
            Self::Corrupt { .. } => "corrupt_store",
            Self::BadValue(..) => "bad_value",
            Self::Io { .. } => "io_error",
        }
    }
}

/// Capability: turn a `SecretRef` into a plaintext secret.
///
/// Implementers SHOULD be safe to share across threads and SHOULD NOT
/// log the returned plaintext. Callers MUST NOT surface resolved values
/// through any response type listed in `CONTRACTS §types` (see the
/// secret-redaction rule).
pub trait SecretResolver: Send + Sync {
    /// Resolve the given reference or return a typed error.
    fn resolve(&self, reference: &SecretRef) -> Result<String, SecretError>;
}

/// YAML-backed implementation (v1). Reads `<config_dir>/secrets.yaml`
/// on each call — secrets are not cached, so edits are picked up
/// without restarting the daemon.
#[derive(Debug, Clone)]
pub struct YamlSecretStore {
    file: PathBuf,
}

impl YamlSecretStore {
    /// Construct a YAML-backed store rooted at the given config dir.
    #[must_use]
    pub fn new(config_dir: &Path) -> Self {
        Self {
            file: config_dir.join("secrets.yaml"),
        }
    }

    fn file_display(&self) -> String {
        // Always surface the basename + full path so error messages
        // satisfy the V5CORE-4 "error message names secrets.yaml"
        // invariant.
        self.file.display().to_string()
    }
}

impl SecretResolver for YamlSecretStore {
    fn resolve(&self, reference: &SecretRef) -> Result<String, SecretError> {
        // Load the file. Missing → empty store (all lookups not-found).
        let contents = match fs::read_to_string(&self.file) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SecretError::NotFound(reference.as_str().to_string()));
            }
            Err(e) => {
                return Err(SecretError::Io {
                    file: self.file_display(),
                    source: e,
                });
            }
        };

        // Parse YAML. Use serde_yaml::Value first so we can diagnose
        // unexpected value shapes precisely.
        let parsed: serde_yaml::Value = serde_yaml::from_str(&contents).map_err(|e| {
            SecretError::Corrupt {
                file: format!("secrets.yaml ({})", self.file_display()),
                reason: e.to_string(),
            }
        })?;

        let map = match parsed {
            serde_yaml::Value::Mapping(m) => m,
            serde_yaml::Value::Null => {
                // Empty document → not-found.
                return Err(SecretError::NotFound(reference.as_str().to_string()));
            }
            _ => {
                return Err(SecretError::Corrupt {
                    file: format!("secrets.yaml ({})", self.file_display()),
                    reason: "top-level value is not a mapping".to_string(),
                });
            }
        };

        let key = reference.path();
        let key_yaml = serde_yaml::Value::String(key.to_string());
        match map.get(&key_yaml) {
            None => Err(SecretError::NotFound(reference.as_str().to_string())),
            Some(serde_yaml::Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(SecretError::BadValue(
                key.to_string(),
                format!("secrets.yaml ({})", self.file_display()),
            )),
        }
    }
}
