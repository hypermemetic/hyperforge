//! Event types for organization activations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{Forge, Org, OrgSummary, Visibility};

// ============================================================================
// OrgEvent
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrgEvent {
    /// List of all organizations
    Listed { orgs: Vec<OrgSummary> },

    /// Details of a single organization
    Details { org: Org },

    /// Organization created
    Created { org_name: String },

    /// Organization removed
    Removed { org_name: String },

    /// Organization updated
    Updated {
        org_name: String,
        field: String,
        value: String,
    },

    /// SSH config updated with Host entries for the org's forges
    SshConfigUpdated {
        org_name: String,
        hosts: Vec<String>,
    },

    /// Informational message about organization
    Info { name: String, message: String },

    /// Import started
    ImportStarted {
        org_name: String,
        forges: Vec<Forge>,
    },

    /// Repository discovered during import
    RepoImported {
        org_name: String,
        repo_name: String,
        forges: Vec<Forge>,
        description: Option<String>,
        visibility: Visibility,
    },

    /// Import completed
    ImportComplete {
        org_name: String,
        imported_count: usize,
        skipped_count: usize,
    },

    /// Error occurred
    Error { message: String },
}
