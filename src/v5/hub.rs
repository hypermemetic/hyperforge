//! `HyperforgeHub` (v5) — root activation for the v5 rewrite.
//!
//! V5CORE-2 scaffolded the hub with a placeholder `status` returning
//! only `version`. V5CORE-5 pins the full `StatusEvent` shape
//! (`version` + `config_dir`). V5CORE-6/7/8 attach child stubs,
//! V5CORE-4 adds `resolve_secret`.
//!
//! plexus-macros 0.5 rejects activations with zero `#[method]`
//! functions, so `status` ships from V5CORE-2 onwards.

use std::path::PathBuf;
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::orgs::OrgsHub;
use crate::v5::repos::ReposHub;
use crate::v5::secrets::{SecretRef, SecretResolver, YamlSecretStore};
use crate::v5::workspaces::WorkspacesHub;

/// Events emitted by the v5 root hub.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeV5Event {
    /// Daemon self-report. `version` is the crate version; `config_dir`
    /// is the absolute, expanded config directory in use (V5CORE-5).
    Status {
        version: String,
        config_dir: String,
    },
    /// Secret-resolve success (V5CORE-4). Carries the plaintext value
    /// under `.value`. Only emitted by `resolve_secret`; the redaction
    /// rule from CONTRACTS §types prohibits every other method from
    /// including resolved values.
    SecretResolved { value: String },
    /// Generic error event.
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

/// Root activation for hyperforge v5.
#[derive(Clone)]
pub struct HyperforgeHub {
    state: Arc<HubState>,
}

/// Shared read-only state the root hub threads into methods.
#[derive(Debug)]
pub struct HubState {
    /// Absolute, expanded config directory.
    pub config_dir: PathBuf,
}

impl HyperforgeHub {
    /// Construct a hub rooted at the given config directory.
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(HubState { config_dir }),
        }
    }
}

/// Hyperforge v5 root — minimal scaffold.
#[plexus_macros::activation(
    namespace = "hyperforge",
    description = "Hyperforge v5 root",
    crate_path = "plexus_core"
)]
impl HyperforgeHub {
    /// Orgs namespace — CRUD + credentials. Methods attached by V5ORGS.
    #[plexus_macros::child]
    fn orgs(&self) -> OrgsHub {
        OrgsHub::new(self.state.config_dir.clone())
    }

    /// Repos namespace — CRUD + `ForgePort`. Methods attached by V5REPOS.
    #[plexus_macros::child]
    fn repos(&self) -> ReposHub {
        ReposHub::with_config_dir(self.state.config_dir.clone())
    }

    /// Workspaces namespace — CRUD + reconcile + sync. Methods attached by V5WS.
    #[plexus_macros::child]
    fn workspaces(&self) -> WorkspacesHub {
        WorkspacesHub::new(self.state.config_dir.clone())
    }

    /// Return daemon version and config directory.
    #[plexus_macros::method]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let config_dir = self.state.config_dir.display().to_string();
        stream! {
            yield HyperforgeV5Event::Status { version, config_dir };
        }
    }

    /// Resolve a `secrets://<path>` reference through the embedded
    /// secret store and emit the plaintext value.
    ///
    /// This method exists to give tests a wire surface for the
    /// `SecretResolver` capability (V5CORE-4 acceptance #1). Production
    /// callers use the trait directly; no other wire method emits
    /// resolved secrets (redaction rule from CONTRACTS §types).
    #[plexus_macros::method(params(
        secret_ref = "secrets:// reference to resolve"
    ))]
    pub async fn resolve_secret(
        &self,
        secret_ref: String,
    ) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        let store = YamlSecretStore::new(&self.state.config_dir);
        stream! {
            let parsed = match SecretRef::parse(&secret_ref) {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeV5Event::Error {
                        code: Some(e.code().to_string()),
                        message: format!("{secret_ref}: {e}"),
                    };
                    return;
                }
            };
            match store.resolve(&parsed) {
                Ok(value) => yield HyperforgeV5Event::SecretResolved { value },
                Err(e) => yield HyperforgeV5Event::Error {
                    code: Some(e.code().to_string()),
                    message: e.to_string(),
                },
            }
        }
    }
}
