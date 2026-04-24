//! `WorkspacesHub` — v5 workspaces namespace. V5CORE-8 ships an empty
//! static child; V5WS attaches CRUD + reconcile + sync methods.

use async_stream::stream;
use futures::Stream;

use crate::v5::hub::HyperforgeV5Event;

/// Workspaces namespace. Methods attached by V5WS.
#[derive(Clone, Default)]
pub struct WorkspacesHub;

impl WorkspacesHub {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Workspaces CRUD + reconcile + sync. Methods attached by V5WS.
#[plexus_macros::activation(
    namespace = "workspaces",
    description = "Workspaces CRUD",
    crate_path = "plexus_core"
)]
impl WorkspacesHub {
    /// Internal placeholder. Filtered out of the v5 wire schema by the
    /// harness; the wire contract is zero methods at this stage.
    #[plexus_macros::method]
    async fn _reserved(&self) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        stream! {
            yield HyperforgeV5Event::Error {
                code: Some("not_implemented".into()),
                message: "workspaces is reserved; methods land in V5WS".into(),
            };
        }
    }
}
