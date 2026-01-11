//! Event types for forge activations.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::storage::TokenStatus;
use crate::types::Forge;

// ============================================================================
// ForgeRepoSummary
// ============================================================================

/// Summary of a repository on a forge
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeRepoSummary {
    pub name: String,
    pub description: Option<String>,
    pub url: String,
    pub private: bool,
}

// ============================================================================
// ForgeEvent
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForgeEvent {
    /// List of repositories on forge
    ReposListed {
        forge: Forge,
        owner: String,
        repos: Vec<ForgeRepoSummary>,
    },

    /// Repository created on forge
    RepoCreated {
        forge: Forge,
        owner: String,
        repo_name: String,
        url: String,
    },

    /// Authentication status
    AuthStatus {
        forge: Forge,
        authenticated: bool,
        user: Option<String>,
        scopes: Vec<String>,
    },

    /// API call progress
    ApiProgress {
        forge: Forge,
        operation: String,
        message: String,
    },

    /// API error
    Error {
        forge: Forge,
        operation: String,
        message: String,
        status_code: Option<u16>,
    },

    // ========================================================================
    // Auth command events
    // ========================================================================

    /// Auth check started
    AuthStarted {
        forge: Forge,
        org_name: String,
    },

    /// Auth check completed with result
    AuthResult {
        forge: Forge,
        org_name: String,
        status: TokenStatus,
        username: Option<String>,
        scopes: Vec<String>,
        last_validated: Option<DateTime<Utc>>,
    },

    /// Auth check failed (network/internal error, not auth failure)
    AuthFailed {
        forge: Forge,
        org_name: String,
        error: String,
    },

    // ========================================================================
    // Refresh command events
    // ========================================================================

    /// Token refresh started
    RefreshStarted {
        forge: Forge,
        org_name: String,
    },

    /// Token refresh completed successfully
    RefreshComplete {
        forge: Forge,
        org_name: String,
        status: TokenStatus,
    },

    /// Token refresh failed
    RefreshFailed {
        forge: Forge,
        org_name: String,
        error: String,
    },
}
