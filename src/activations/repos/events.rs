//! Event types for repository activations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::Forge;

// ============================================================================
// ConvergeResult - Result of a convergence operation
// ============================================================================

/// Result of a convergence operation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConvergeResult {
    /// Whether the system converged (no remaining drift)
    pub converged: bool,
    /// Number of repos synced (created + updated)
    pub repos_synced: usize,
    /// Number of repos created on forges
    pub repos_created: usize,
    /// Number of repos deleted from forges
    pub repos_deleted: usize,
    /// Whether drift was detected after apply
    pub drift_detected: bool,
}

// ============================================================================
// DiffStatus - Shared type for diff operations
// ============================================================================

/// Status of a repository in a diff operation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiffStatus {
    /// Repo in local config, not on forges
    ToCreate,
    /// Repo exists but config differs
    ToUpdate,
    /// Repo marked for deletion
    ToDelete,
    /// Repo in sync
    InSync,
    /// Repo on forges but not in local config
    Untracked,
}

// ============================================================================
// RepoEvent
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoEvent {
    /// List of repositories
    Listed {
        org_name: String,
        repos: Vec<crate::types::RepoSummary>,
        staged: bool,
    },

    /// Details of a single repository
    Details {
        org_name: String,
        repo: crate::types::RepoDetails,
    },

    /// Repository staged for creation
    Staged {
        org_name: String,
        repo_name: String,
    },

    /// Repository created/synced on forge
    Synced {
        org_name: String,
        repo_name: String,
        forge: Forge,
        url: String,
    },

    /// Sync operation started
    SyncStarted { org_name: String, repo_count: usize },

    /// Sync progress update
    SyncProgress {
        org_name: String,
        repo_name: String,
        stage: String,
    },

    /// Sync operation completed
    SyncComplete {
        org_name: String,
        success: bool,
        synced_count: usize,
    },

    /// Repository marked for deletion
    MarkedForDeletion {
        org_name: String,
        repo_name: String,
    },

    /// Repository protection error (protected repos require --force)
    ProtectionError {
        org_name: String,
        repo_name: String,
        message: String,
    },

    /// Repository removed
    Removed {
        org_name: String,
        repo_name: String,
    },

    /// Git remote added to local repository
    RemoteAdded {
        org_name: String,
        repo_name: String,
        remote: String,
        url: String,
    },

    /// Git remotes validated for a repository
    RemotesValidated {
        org_name: String,
        repo_name: String,
        remotes: Vec<String>,
    },

    /// Outputs captured from Pulumi after apply
    OutputsCaptured {
        org_name: String,
        repo_name: String,
        forge: Forge,
        url: String,
        id: Option<String>,
    },

    /// Refresh started - querying forges for remote state
    RefreshStarted {
        org_name: String,
        forges: Vec<Forge>,
    },

    /// Forge query progress during refresh
    RefreshProgress {
        org_name: String,
        forge: Forge,
        repos_found: usize,
    },

    /// Refresh completed with discovery statistics
    RefreshComplete {
        org_name: String,
        discovered: usize,
        matched: usize,
        untracked: usize,
    },

    /// Diff result for a single repository
    RepoDiff {
        org_name: String,
        repo_name: String,
        status: DiffStatus,
        details: Vec<String>,
    },

    /// Overall diff summary for an organization
    DiffSummary {
        org_name: String,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
        untracked: usize,
    },

    /// Converge operation started
    ConvergeStarted {
        org_name: String,
        phases: Vec<String>,
    },

    /// Converge phase status update
    ConvergePhase {
        org_name: String,
        phase: String,
        status: String,
    },

    /// Converge operation completed
    ConvergeComplete {
        org_name: String,
        success: bool,
        changes_applied: usize,
        final_state: ConvergeResult,
    },

    /// Clone operation started
    CloneStarted {
        org_name: String,
        repo_name: String,
        target_path: PathBuf,
    },

    /// Clone progress - cloning from forge
    CloneProgress {
        org_name: String,
        repo_name: String,
        forge: Forge,
        stage: String,
    },

    /// Remote added during clone
    CloneRemoteAdded {
        org_name: String,
        repo_name: String,
        remote_name: String,
        url: String,
    },

    /// Clone completed successfully
    CloneComplete {
        org_name: String,
        repo_name: String,
        target_path: PathBuf,
        remotes: Vec<String>,
    },

    /// Remote sync status (comparing refs across remotes)
    RemoteSyncStatus {
        org_name: String,
        repo_name: String,
        branch: String,
        in_sync: bool,
        details: Vec<String>,
    },

    /// Forge skipped due to sync flag being false
    ForgeSkipped {
        org_name: String,
        forge: Forge,
        reason: String,
    },

    /// Error during repo operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}
