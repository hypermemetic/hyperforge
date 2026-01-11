//! Event types for workspace activations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::{ResolutionSource, WorkspaceBinding};

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
    // Sync Events
    // ============================================================================

    /// Sync operation started for workspace
    SyncStarted {
        workspace_path: PathBuf,
        org_count: usize,
    },

    /// Sync started for a specific org within workspace sync
    OrgSyncStarted { org_name: String },

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
}
