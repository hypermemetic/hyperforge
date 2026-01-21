//! Event types for repository activations.
//!
//! Each repository operation has its own event type to provide type-safe,
//! operation-specific event streams.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::{Forge, PackageConfig, RepoSummary, RepoDetails};

// ============================================================================
// Shared Types
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
// List Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoListEvent {
    /// List of repositories
    Listed {
        org_name: String,
        repos: Vec<RepoSummary>,
        staged: bool,
    },

    /// Details of a single repository
    Details {
        org_name: String,
        repo: RepoDetails,
    },

    /// Error during list operation
    Error {
        org_name: String,
        message: String,
    },
}

// ============================================================================
// Create Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoCreateEvent {
    /// Repository staged for creation
    Staged {
        org_name: String,
        repo_name: String,
    },

    /// Local git repository initialized
    LocalInitialized {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// .gitignore file created
    GitignoreCreated {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// Git remote added to local repository
    RemoteAdded {
        org_name: String,
        repo_name: String,
        remote: String,
        url: String,
    },

    /// Local repository setup complete
    LocalSetupComplete {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// Error during create operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}

// ============================================================================
// Adopt Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoAdoptEvent {
    /// Adopt operation started
    Started {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// Git repository initialized during adopt
    GitInitialized {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// Git remotes detected during adopt
    RemotesDetected {
        org_name: String,
        repo_name: String,
        forges: Vec<Forge>,
    },

    /// Packages detected during adopt
    PackagesDetected {
        org_name: String,
        repo_name: String,
        packages: Vec<PackageConfig>,
    },

    /// Adopt operation completed
    Complete {
        org_name: String,
        repo_name: String,
        path: PathBuf,
    },

    /// Error during adopt operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}

// ============================================================================
// Sync Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoSyncEvent {
    /// Sync operation started
    Started {
        org_name: String,
        repo_count: usize,
    },

    /// Sync progress update
    Progress {
        org_name: String,
        repo_name: String,
        stage: String,
    },

    /// Repository synced to forge
    Synced {
        org_name: String,
        repo_name: String,
        forge: Forge,
        url: String,
    },

    /// Forge skipped due to sync flag being false
    ForgeSkipped {
        org_name: String,
        forge: Forge,
        reason: String,
    },

    /// Outputs captured from Pulumi after apply
    OutputsCaptured {
        org_name: String,
        repo_name: String,
        forge: Forge,
        url: String,
        id: Option<String>,
    },

    /// Sync operation completed
    Complete {
        org_name: String,
        success: bool,
        synced_count: usize,
    },

    /// Error during sync operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}

// ============================================================================
// Remove Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoRemoveEvent {
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

    /// Error during remove operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}

// ============================================================================
// Refresh Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoRefreshEvent {
    /// Refresh started - querying forges for remote state
    Started {
        org_name: String,
        forges: Vec<Forge>,
    },

    /// Forge query progress during refresh
    Progress {
        org_name: String,
        forge: Forge,
        repos_found: usize,
    },

    /// Refresh completed with discovery statistics
    Complete {
        org_name: String,
        discovered: usize,
        matched: usize,
        untracked: usize,
    },

    /// Error during refresh operation
    Error {
        org_name: String,
        message: String,
    },
}

// ============================================================================
// Diff Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoDiffEvent {
    /// Diff result for a single repository
    RepoDiff {
        org_name: String,
        repo_name: String,
        status: DiffStatus,
        details: Vec<String>,
    },

    /// Overall diff summary for an organization
    Summary {
        org_name: String,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
        untracked: usize,
    },

    /// Error during diff operation
    Error {
        org_name: String,
        message: String,
    },
}

// ============================================================================
// Converge Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoConvergeEvent {
    /// Converge operation started
    Started {
        org_name: String,
        phases: Vec<String>,
    },

    /// Converge phase status update
    Phase {
        org_name: String,
        phase: String,
        status: String,
    },

    /// Converge operation completed
    Complete {
        org_name: String,
        success: bool,
        changes_applied: usize,
        final_state: ConvergeResult,
    },

    /// Error during converge operation
    Error {
        org_name: String,
        message: String,
    },
}

// ============================================================================
// Clone Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RepoCloneEvent {
    /// Clone operation started
    Started {
        org_name: String,
        repo_name: String,
        target_path: PathBuf,
    },

    /// Clone progress - cloning from forge
    Progress {
        org_name: String,
        repo_name: String,
        forge: Forge,
        stage: String,
    },

    /// Remote added during clone
    RemoteAdded {
        org_name: String,
        repo_name: String,
        remote_name: String,
        url: String,
    },

    /// Git remotes validated for a repository
    RemotesValidated {
        org_name: String,
        repo_name: String,
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

    /// Clone completed successfully
    Complete {
        org_name: String,
        repo_name: String,
        target_path: PathBuf,
        remotes: Vec<String>,
    },

    /// Error during clone operation
    Error {
        org_name: String,
        repo_name: Option<String>,
        message: String,
    },
}
