//! Event types for workspace activations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::{ResolutionSource, WorkspaceBinding};

// ============================================================================
// RepoSyncStatus - Status of a repo during sync/preview
// ============================================================================

/// Status of a repo during sync or preview operations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepoSyncStatus {
    /// Repo will be created on target forge
    ToCreate,
    /// Repo will be updated on target forge
    ToUpdate,
    /// Repo will be deleted from target forge
    ToDelete,
    /// Repo is in sync, no action needed
    InSync,
    /// Repo sync failed
    Failed,
    /// Repo sync succeeded (for actual sync, not preview)
    Synced,
}

/// Summary of a single repo's status within a forge
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoStatusEntry {
    pub name: String,
    pub status: RepoSyncStatus,
    /// Human-readable details (e.g., "visibility: public → private")
    pub details: Vec<String>,
}

/// Info about a repo in the workspace context
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceRepoInfo {
    pub name: String,
    /// Whether the repo is cloned locally in the workspace
    pub cloned: bool,
    /// Whether the repo is migrated to use hyperforge-ssh
    pub migrated: bool,
}

// ============================================================================
// WorkspaceEvent
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    /// List of workspace bindings
    Listed { bindings: Vec<WorkspaceBinding> },

    /// Current workspace resolution
    Resolved {
        org_name: String,
        source: ResolutionSource,
        path: PathBuf,
    },

    /// Workspace bound to org
    Bound { path: PathBuf, org_name: String },

    /// Workspace unbound
    Unbound { path: PathBuf },

    /// No workspace binding found
    NotBound { path: PathBuf },

    /// Repos discovered during auto-create scan
    ReposDiscovered { path: PathBuf, repos: Vec<String> },

    /// Repo staged during auto-create
    RepoStaged { repo_name: String },

    /// List of repos in workspace
    ReposListed {
        org_name: String,
        workspace_path: PathBuf,
        repos: Vec<WorkspaceRepoInfo>,
    },

    /// Error during workspace operation
    Error { message: String },

    // ============================================================================
    // Diff Events
    // ============================================================================

    /// Diff operation started for workspace
    DiffStarted {
        workspace_path: PathBuf,
        org_count: usize,
    },

    /// Diff result for a single org in the workspace
    OrgDiffResult {
        org_name: String,
        in_sync: usize,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
    },

    /// Org diff failed with error
    OrgDiffError {
        org_name: String,
        message: String,
    },

    /// Diff operation completed with summary
    DiffComplete {
        total_orgs: usize,
        total_in_sync: usize,
        total_to_create: usize,
        total_to_update: usize,
        total_to_delete: usize,
    },

    // ============================================================================
    // Import Events
    // ============================================================================

    /// Workspace import operation started
    ImportStarted {
        workspace_path: PathBuf,
        org_count: usize,
    },

    /// Started importing repos for a specific org
    OrgImportStarted { org_name: String },

    /// Completed importing repos for a specific org
    OrgImportComplete {
        org_name: String,
        imported: usize,
        skipped: usize,
        errors: usize,
    },

    /// Workspace import operation completed
    ImportComplete {
        total_imported: usize,
        total_skipped: usize,
        total_errors: usize,
    },

    // ============================================================================
    // Clone All Events
    // ============================================================================

    /// Clone all operation started
    CloneAllStarted { org_count: usize },

    /// Starting clone_all for a specific org
    OrgCloneAllStarted { org_name: String },

    /// Clone all complete for a specific org
    OrgCloneAllComplete {
        org_name: String,
        cloned: usize,
        skipped: usize,
        failed: usize,
    },

    /// Clone all operation complete for all orgs
    CloneAllComplete {
        total_cloned: usize,
        total_skipped: usize,
        total_failed: usize,
    },

    // ============================================================================
    // Preview Events (sync without --yes)
    // ============================================================================

    /// Preview operation started for workspace
    PreviewStarted {
        workspace_path: PathBuf,
        org_count: usize,
    },

    /// Preview started for a specific org
    OrgPreviewStarted { org_name: String },

    /// Pulumi preview output line
    PreviewOutput { line: String },

    /// Importing existing resource into Pulumi state
    PulumiImporting {
        org_name: String,
        repo_name: String,
        forge: String,
    },

    /// Pulumi import result for a resource
    PulumiImported {
        org_name: String,
        repo_name: String,
        forge: String,
        imported: bool,  // true if newly imported, false if already in state
    },

    /// Grouped status for all repos on a single forge (preview mode)
    ForgePreview {
        org_name: String,
        forge: String,
        repos: Vec<RepoStatusEntry>,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
    },

    /// Grouped status for all repos on a single forge (sync mode)
    ForgeSynced {
        org_name: String,
        forge: String,
        repos: Vec<RepoStatusEntry>,
        created: usize,
        updated: usize,
        deleted: usize,
        unchanged: usize,
        failed: usize,
    },

    /// Preview completed for a specific org - shows what would change
    OrgPreviewComplete {
        org_name: String,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        unchanged: usize,
    },

    /// Preview operation completed for entire workspace
    PreviewComplete {
        workspace_path: PathBuf,
        total_to_create: usize,
        total_to_update: usize,
        total_to_delete: usize,
        total_unchanged: usize,
    },

    // ============================================================================
    // Sync Events (sync with --yes)
    // ============================================================================

    /// Sync operation started for workspace
    SyncStarted {
        workspace_path: PathBuf,
        org_count: usize,
    },

    /// Sync started for a specific org within workspace sync
    OrgSyncStarted { org_name: String },

    /// Pulumi sync output line
    SyncOutput { line: String },

    /// Sync completed for a specific org within workspace sync
    OrgSyncComplete {
        org_name: String,
        synced: usize,
        unchanged: usize,
        failed: usize,
    },

    /// Sync operation completed for entire workspace
    SyncComplete {
        workspace_path: PathBuf,
        total_synced: usize,
        total_unchanged: usize,
        total_failed: usize,
    },

    // ============================================================================
    // SSH Config Enforcement Events
    // ============================================================================

    /// SSH config enforcement started for workspace
    SshEnforceStarted {
        workspace_path: PathBuf,
        repo_count: usize,
    },

    /// SSH config updated for a single repo
    SshConfigUpdated {
        repo_name: String,
        org_name: String,
    },

    /// SSH config already correct for a repo
    SshConfigUnchanged { repo_name: String },

    /// SSH config enforcement failed for a repo
    SshConfigError {
        repo_name: String,
        message: String,
    },

    /// SSH config enforcement completed
    SshEnforceComplete {
        workspace_path: PathBuf,
        updated: usize,
        unchanged: usize,
        failed: usize,
    },

    // ============================================================================
    // Uninitialized Repos Discovery Events
    // ============================================================================

    /// Uninitialized repos discovered (git repos not in config)
    UninitializedRepos {
        workspace_path: PathBuf,
        repos: Vec<String>,
    },

    // ============================================================================
    // Forge Query Events
    // ============================================================================

    /// Repos found on a specific forge
    ForgeRepos {
        org_name: String,
        forge: String,
        repos: Vec<ForgeRepoInfo>,
    },
}

/// Info about a repo on a forge
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeRepoInfo {
    pub name: String,
    pub visibility: crate::types::Visibility,
    pub description: Option<String>,
}
