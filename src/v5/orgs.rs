//! `OrgsHub` — v5 orgs namespace. V5ORGS attaches CRUD + credential
//! management methods on top of the V5CORE-6 stub.
//!
//! Event envelope follows CONTRACTS D9: every event serializes with a
//! top-level `type` discriminator (`snake_case`). Errors use
//! `{type: "error", code, message}`. Secret redaction rule is enforced:
//! returned events carry `CredentialEntry` refs (key + type) only —
//! never resolved plaintext values.

use std::path::PathBuf;
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::config::{load_orgs, CredentialEntry, OrgName, ProviderKind, RepoName};

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
}
