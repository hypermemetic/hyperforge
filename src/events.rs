//! Event types for hyperforge activations.
//!
//! This module re-exports all domain-specific events from their respective
//! activation modules, providing a unified import point.
//!
//! Events are typed domain events that stream from activation methods.
//! Each activation has its own event enum tagged with `#[serde(tag = "type")]`.
//! Events are self-describing and support streaming (progress, partial results).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Re-export all domain-specific events from activation modules
pub use crate::activations::forge::events::{ForgeEvent, ForgeRepoSummary};
pub use crate::activations::org::events::OrgEvent;
pub use crate::activations::repos::events::{ConvergeResult, DiffStatus, RepoEvent};
pub use crate::activations::secrets::events::SecretEvent;
pub use crate::activations::workspace::events::WorkspaceEvent;

// ============================================================================
// PulumiEvent - Bridge-level events (not activation-specific)
// ============================================================================

/// Pulumi operation type
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PulumiOperation {
    Create,
    Update,
    Delete,
    Replace,
    Same,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PulumiEvent {
    /// Preview started
    PreviewStarted { org_name: String, stack: String },

    /// Resource change planned
    ResourcePlanned {
        operation: PulumiOperation,
        resource_type: String,
        resource_name: String,
    },

    /// Preview completed
    PreviewComplete {
        creates: usize,
        updates: usize,
        deletes: usize,
        unchanged: usize,
    },

    /// Up started
    UpStarted { org_name: String, stack: String },

    /// Resource change applied
    ResourceApplied {
        operation: PulumiOperation,
        resource_type: String,
        resource_name: String,
        success: bool,
    },

    /// Up completed
    UpComplete {
        success: bool,
        creates: usize,
        updates: usize,
        deletes: usize,
    },

    /// Pulumi output line (raw)
    Output { line: String },

    /// Error
    Error { message: String },
}
