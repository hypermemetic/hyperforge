//! `ReposHub` — v5 repos namespace. V5CORE-7 ships an empty static
//! child; V5REPOS attaches CRUD + `ForgePort` methods.

use async_stream::stream;
use futures::Stream;

use crate::v5::hub::HyperforgeV5Event;

/// Repos namespace. Methods attached by V5REPOS.
#[derive(Clone, Default)]
pub struct ReposHub;

impl ReposHub {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Repos CRUD. Methods attached by V5REPOS.
#[plexus_macros::activation(
    namespace = "repos",
    description = "Repos CRUD",
    crate_path = "plexus_core"
)]
impl ReposHub {
    /// Internal placeholder. Filtered out of the v5 wire schema by the
    /// harness; the wire contract is zero methods at this stage.
    #[plexus_macros::method]
    async fn _reserved(&self) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        stream! {
            yield HyperforgeV5Event::Error {
                code: Some("not_implemented".into()),
                message: "repos is reserved; methods land in V5REPOS".into(),
            };
        }
    }
}
