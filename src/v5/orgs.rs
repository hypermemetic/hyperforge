//! `OrgsHub` — v5 orgs namespace. V5CORE-6 ships this as an empty
//! static child. The V5ORGS epic attaches CRUD methods.
//!
//! plexus-macros 0.5 requires ≥ 1 `#[plexus_macros::method]` per
//! activation. The `_reserved` method below is a placeholder; the v5
//! harness's schema introspection filters method names beginning with
//! `_`, so the ticket's "zero methods" contract holds on the wire.

use async_stream::stream;
use futures::Stream;

use crate::v5::hub::HyperforgeV5Event;

/// Orgs namespace. CRUD methods added by V5ORGS.
#[derive(Clone, Default)]
pub struct OrgsHub;

impl OrgsHub {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Orgs CRUD. Methods attached by V5ORGS.
#[plexus_macros::activation(
    namespace = "orgs",
    description = "Orgs CRUD",
    crate_path = "plexus_core"
)]
impl OrgsHub {
    /// Internal placeholder so the activation macro has a method to
    /// anchor on. Not part of the v5 wire contract.
    #[plexus_macros::method]
    async fn _reserved(&self) -> impl Stream<Item = HyperforgeV5Event> + Send + 'static {
        stream! {
            yield HyperforgeV5Event::Error {
                code: Some("not_implemented".into()),
                message: "orgs is reserved; methods land in V5ORGS".into(),
            };
        }
    }
}
