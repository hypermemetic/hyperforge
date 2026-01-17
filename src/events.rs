//! Event types for hyperforge activations.
//!
//! This module re-exports all domain-specific events from their respective
//! activation modules, providing a unified import point.
//!
//! Events are typed domain events that stream from activation methods.
//! Each activation has its own event enum tagged with `#[serde(tag = "type")]`.
//! Events are self-describing and support streaming (progress, partial results).

// Re-export all domain-specific events from activation modules
pub use crate::activations::forge::events::{ForgeEvent, ForgeRepoSummary};
pub use crate::activations::org::events::OrgEvent;
pub use crate::activations::repos::events::{ConvergeResult, DiffStatus, RepoEvent};
pub use crate::activations::secrets::events::SecretEvent;
pub use crate::activations::workspace::events::{RepoStatusEntry, RepoSyncStatus, WorkspaceEvent};
