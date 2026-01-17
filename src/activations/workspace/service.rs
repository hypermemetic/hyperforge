//! Workspace service - business logic for workspace operations.
//!
//! This module contains the core workspace logic extracted from the activation.
//! The activation layer handles parameter validation and event emission,
//! while this service handles the actual work.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::activations::org::OrgActivation;
use crate::activations::workspace::events::{RepoStatusEntry, RepoSyncStatus};
use crate::adapters::{GitHubAdapter, CodebergAdapter, LocalForge};
use crate::bridge::{GitRemoteBridge, KeychainBridge};
use crate::domain::PropertyDiff;
use crate::events::WorkspaceEvent;
use crate::ports::ForgePort;
use crate::services::symmetric_sync::{SymmetricSyncService, SyncOptions, SyncOutcome};
use crate::storage::{GlobalConfig, HyperforgePaths, OrgConfig, OrgStorage};
use crate::types::{Forge, RepoConfig, WorkspaceBinding};

/// Result of workspace resolution
pub struct WorkspaceResolution {
    pub workspace_path: PathBuf,
    pub org_name: String,
    pub bound_orgs: Vec<String>,
}

/// Diff statistics for an organization
#[derive(Default)]
pub struct OrgDiffStats {
    pub in_sync: usize,
    pub to_create: usize,
    pub to_update: usize,
    pub to_delete: usize,
}

/// Service for workspace operations.
pub struct WorkspaceService {
    paths: Arc<HyperforgePaths>,
}

impl WorkspaceService {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }

    /// List all workspace bindings
    pub async fn list_bindings(&self) -> Result<Vec<WorkspaceBinding>, String> {
        let config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        Ok(config
            .workspaces
            .iter()
            .map(|(path, org)| WorkspaceBinding {
                path: path.clone(),
                org_name: org.clone(),
            })
            .collect())
    }

    /// Resolve workspace for a given path
    pub async fn resolve_workspace(
        &self,
        check_path: &PathBuf,
    ) -> Result<Option<WorkspaceResolution>, String> {
        let config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(org_name) = config.resolve_workspace(check_path) {
            let workspace_path = self.find_workspace_path(&config, check_path);
            let bound_orgs = self.find_bound_orgs(&config, check_path);

            Ok(Some(WorkspaceResolution {
                workspace_path: workspace_path.unwrap_or_else(|| check_path.clone()),
                org_name,
                bound_orgs,
            }))
        } else if let Some(ref default_org) = config.default_org {
            Ok(Some(WorkspaceResolution {
                workspace_path: check_path.clone(),
                org_name: default_org.clone(),
                bound_orgs: vec![default_org.clone()],
            }))
        } else {
            Ok(None)
        }
    }

    /// Bind a directory to an organization
    pub async fn bind(
        &self,
        bind_path: PathBuf,
        org_name: String,
    ) -> Result<(), String> {
        let mut config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        if !config.organizations.contains_key(&org_name) {
            return Err(format!("Organization not found: {}", org_name));
        }

        config.workspaces.insert(bind_path, org_name);
        config.save(&self.paths).await.map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Unbind a directory from its organization
    pub async fn unbind(&self, unbind_path: &PathBuf) -> Result<bool, String> {
        let mut config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        let removed = config.workspaces.remove(unbind_path).is_some();
        if removed {
            config.save(&self.paths).await.map_err(|e| e.to_string())?;
        }

        Ok(removed)
    }

    /// Discover git repositories in a directory
    pub async fn discover_git_repos(&self, base_path: &PathBuf) -> Result<Vec<String>, String> {
        let mut repos = Vec::new();
        let mut entries = tokio::fs::read_dir(base_path)
            .await
            .map_err(|e| e.to_string())?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();
            if path.is_dir() && path.join(".git").exists() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    repos.push(name.to_string());
                }
            }
        }

        repos.sort();
        Ok(repos)
    }

    /// Stage a repository for an organization
    pub async fn stage_repo(
        &self,
        org_name: &str,
        repo_name: String,
        forges: Option<Vec<Forge>>,
    ) -> Result<(), String> {
        let config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        let default_forges = config
            .get_org(org_name)
            .map(|o| o.forges.all_forges());

        let storage = OrgStorage::new((*self.paths).clone(), org_name.to_string());
        let repo_config = RepoConfig {
            description: None,
            visibility: None,
            forges: forges.or(default_forges),
            protected: false,
            delete: false,
            synced: None,
            discovered: None,
        };

        storage
            .stage_repo(repo_name, repo_config)
            .await
            .map_err(|e| e.to_string())
    }

    /// Compute diff statistics for an organization
    pub async fn compute_org_diff(&self, org_name: &str) -> Result<OrgDiffStats, String> {
        let config = GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())?;

        let org_config = config
            .get_org(org_name)
            .ok_or_else(|| format!("Organization config not found: {}", org_name))?;

        let storage = OrgStorage::new((*self.paths).clone(), org_name.to_string());
        let mut repos = storage.load_repos().await.map_err(|e| e.to_string())?;

        // Include staged repos
        if let Ok(staged) = storage.load_staged().await {
            for (name, staged_config) in staged.repos {
                if staged_config.delete {
                    if let Some(existing) = repos.repos.get_mut(&name) {
                        existing.delete = true;
                    }
                } else {
                    repos.repos.insert(name, staged_config);
                }
            }
        }

        let mut stats = OrgDiffStats::default();

        for (_, repo_config) in &repos.repos {
            let desired_forges: HashSet<_> = repo_config
                .forges
                .as_ref()
                .map(|f| f.iter().cloned().collect())
                .unwrap_or_else(|| org_config.forges.all_forges().into_iter().collect());

            let synced_forges: HashSet<_> = repo_config
                .synced
                .as_ref()
                .map(|s| s.forges.keys().cloned().collect())
                .unwrap_or_default();

            if repo_config.delete {
                stats.to_delete += 1;
            } else if synced_forges.is_empty() {
                stats.to_create += 1;
            } else if desired_forges != synced_forges {
                stats.to_update += 1;
            } else {
                stats.in_sync += 1;
            }
        }

        Ok(stats)
    }

    /// Clone a repository to the workspace
    pub async fn clone_repo(
        &self,
        workspace_path: &PathBuf,
        repo_name: &str,
        org_config: &OrgConfig,
        org_name: &str,
        forges: &[Forge],
    ) -> Result<bool, String> {
        let repo_path = workspace_path.join(repo_name);

        if repo_path.exists() {
            return Ok(false); // Skipped
        }

        let origin_url = org_config.origin_url(org_name, repo_name);
        let clone_output = tokio::process::Command::new("git")
            .args(["clone", &origin_url, repo_path.to_str().unwrap_or(".")])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if !clone_output.status.success() {
            return Err("Clone failed".to_string());
        }

        // Configure hyperforge org for SSH wrapper
        let _ = tokio::process::Command::new("git")
            .current_dir(&repo_path)
            .args(["config", "hyperforge.org", org_name])
            .output()
            .await;

        // Set core.sshCommand to use hyperforge-ssh wrapper
        // The wrapper reads hyperforge.org and resolves the correct SSH key
        if let Some(home) = std::env::var_os("HOME") {
            let ssh_wrapper = std::path::Path::new(&home)
                .join(".hypermemetic-infra/scripts/hyperforge-ssh");
            let _ = tokio::process::Command::new("git")
                .current_dir(&repo_path)
                .args(["config", "core.sshCommand", ssh_wrapper.to_str().unwrap_or("")])
                .output()
                .await;
        }

        // Add remotes for other forges
        for forge in forges {
            if forge == &org_config.origin {
                continue;
            }

            let forge_name = forge.to_string().to_lowercase();
            let remote_url = org_config.ssh_url(forge, org_name, repo_name);

            let _ = tokio::process::Command::new("git")
                .current_dir(&repo_path)
                .args(["remote", "add", &forge_name, &remote_url])
                .output()
                .await;
        }

        // Fetch all remotes
        let _ = tokio::process::Command::new("git")
            .current_dir(&repo_path)
            .args(["fetch", "--all"])
            .output()
            .await;

        Ok(true) // Cloned
    }

    /// Setup git remotes for repositories in the workspace
    pub async fn setup_repo_remotes(
        &self,
        repo_path: PathBuf,
        org_name: &str,
        owner: &str,
        repo_name: &str,
        forges: &[Forge],
    ) -> Result<Vec<String>, String> {
        let git_bridge = GitRemoteBridge::new(repo_path, org_name.to_string(), owner.to_string());
        git_bridge
            .setup_forge_remotes(forges, repo_name)
            .await
            .map_err(|e| e.to_string())
    }

    /// Get OrgActivation for delegating import
    pub fn org_activation(&self) -> OrgActivation {
        OrgActivation::new(self.paths.clone())
    }

    /// Build forge adapters for an organization's configured forges
    /// Uses `credential_org` for API token lookup (supports sub-orgs with templates)
    pub async fn build_forge_adapters(
        &self,
        credential_org: &str,
        forges: &[Forge],
    ) -> (Vec<(Forge, Arc<dyn ForgePort>)>, Vec<String>) {
        let keychain = KeychainBridge::new(credential_org);
        let mut adapters: Vec<(Forge, Arc<dyn ForgePort>)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for forge in forges {
            match forge {
                Forge::Local => continue,
                Forge::GitLab => {
                    errors.push("GitLab not yet supported".to_string());
                    continue;
                }
                _ => {}
            }

            let token_key = match forge {
                Forge::GitHub => "github-token",
                Forge::Codeberg => "codeberg-token",
                _ => continue,
            };

            match keychain.get(token_key).await {
                Ok(Some(token)) => {
                    let adapter: Arc<dyn ForgePort> = match forge {
                        Forge::GitHub => Arc::new(GitHubAdapter::new(token)),
                        Forge::Codeberg => Arc::new(CodebergAdapter::new(token)),
                        _ => continue,
                    };
                    adapters.push((forge.clone(), adapter));
                }
                Ok(None) => {
                    errors.push(format!("No token configured for {}", forge));
                }
                Err(e) => {
                    errors.push(format!("Failed to get {} token: {}", forge, e));
                }
            }
        }

        (adapters, errors)
    }

    /// Get OrgStorage for an organization
    pub fn org_storage(&self, org_name: &str) -> OrgStorage {
        OrgStorage::new((*self.paths).clone(), org_name.to_string())
    }

    /// Load global config
    pub async fn load_config(&self) -> Result<GlobalConfig, String> {
        GlobalConfig::load(&self.paths)
            .await
            .map_err(|e| e.to_string())
    }

    /// Get repos file path for an organization
    pub fn repos_file(&self, org_name: &str) -> PathBuf {
        self.paths.repos_file(org_name)
    }

    /// Get staged repos file path for an organization
    pub fn staged_repos_file(&self, org_name: &str) -> PathBuf {
        self.paths.staged_repos_file(org_name)
    }

    // ============================================================================
    // Private Helpers
    // ============================================================================

    fn find_workspace_path(&self, config: &GlobalConfig, check_path: &PathBuf) -> Option<PathBuf> {
        let canonical = check_path.canonicalize().ok()?;
        config
            .workspaces
            .iter()
            .filter(|(ws_path, _)| {
                ws_path
                    .canonicalize()
                    .map(|p| canonical.starts_with(&p))
                    .unwrap_or(false)
            })
            .max_by_key(|(ws_path, _)| ws_path.components().count())
            .map(|(p, _)| p.clone())
    }

    fn find_bound_orgs(&self, config: &GlobalConfig, check_path: &PathBuf) -> Vec<String> {
        let canonical = check_path.canonicalize().ok();
        config
            .workspaces
            .iter()
            .filter(|(ws_path, _)| {
                ws_path
                    .canonicalize()
                    .map(|p| {
                        canonical
                            .as_ref()
                            .map(|c| c.starts_with(&p))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .map(|(_, org)| org.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Build human-readable detail strings from a sync action
    fn build_action_details(action: &crate::domain::SyncAction) -> Vec<String> {
        match action {
            crate::domain::SyncAction::Update { diffs, .. } => {
                diffs.iter().map(|diff| {
                    match diff {
                        PropertyDiff::Visibility { source, target } => {
                            format!("visibility: {:?} → {:?}", target, source)
                        }
                        PropertyDiff::Description { source, target } => {
                            let from = target.as_deref().unwrap_or("(none)");
                            let to = source.as_deref().unwrap_or("(none)");
                            format!("description: \"{}\" → \"{}\"", from, to)
                        }
                    }
                }).collect()
            }
            _ => vec![],
        }
    }

    /// Get paths reference for cloning service
    pub fn paths(&self) -> &Arc<HyperforgePaths> {
        &self.paths
    }

    /// Process sync/preview for a single org - returns collected events
    pub async fn process_org_sync(
        &self,
        config: &GlobalConfig,
        org_name: &str,
        workspace_path: &PathBuf,
        auto_yes: bool,
    ) -> Vec<WorkspaceEvent> {
        let mut events = Vec::new();

        if auto_yes {
            events.push(WorkspaceEvent::OrgSyncStarted { org_name: org_name.to_string() });
        } else {
            events.push(WorkspaceEvent::OrgPreviewStarted { org_name: org_name.to_string() });
        }

        let org_config = match config.get_org(org_name) {
            Some(c) => c.clone(),
            None => {
                events.push(WorkspaceEvent::Error { message: format!("Organization not found: {}", org_name) });
                if auto_yes {
                    events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: 0, unchanged: 0, failed: 1 });
                } else {
                    events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: 0, to_update: 0, to_delete: 0, unchanged: 0 });
                }
                return events;
            }
        };

        let storage = self.org_storage(org_name);

        // Load and optionally merge repos
        let repos_config = if auto_yes {
            match storage.merge_staged().await {
                Ok(cfg) => cfg,
                Err(e) => {
                    events.push(WorkspaceEvent::Error { message: format!("Failed to merge staged: {}", e) });
                    events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: 0, unchanged: 0, failed: 1 });
                    return events;
                }
            }
        } else {
            match storage.load_repos().await {
                Ok(mut repos) => {
                    if let Ok(staged) = storage.load_staged().await {
                        for (name, staged_config) in staged.repos {
                            if staged_config.delete {
                                if let Some(existing) = repos.repos.get_mut(&name) {
                                    existing.delete = true;
                                }
                            } else {
                                repos.repos.insert(name, staged_config);
                            }
                        }
                    }
                    repos
                }
                Err(e) => {
                    events.push(WorkspaceEvent::Error { message: format!("Failed to load repos: {}", e) });
                    events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: 0, to_update: 0, to_delete: 0, unchanged: 0 });
                    return events;
                }
            }
        };

        // Build LocalForge from merged repos config (includes staged changes)
        // Also collect repos marked for deletion (they're excluded from LocalForge)
        let local_forge = LocalForge::from_repos_config(&repos_config);
        let repos_to_delete: Vec<String> = repos_config.repos
            .iter()
            .filter(|(_, cfg)| cfg.delete)
            .map(|(name, _)| name.clone())
            .collect();

        // Build forge adapters (use credential_org for API token lookup)
        let forges = org_config.forges.all_forges();
        let credential_org = org_config.credential_org(org_name);
        let (target_forges, adapter_errors) = self.build_forge_adapters(credential_org, &forges).await;

        // Report adapter errors
        for err in adapter_errors {
            events.push(WorkspaceEvent::Error { message: err });
        }

        if target_forges.is_empty() {
            events.push(WorkspaceEvent::Error { message: "No forge adapters available for sync".to_string() });
            if auto_yes {
                events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: 0, unchanged: 0, failed: 1 });
            } else {
                events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: 0, to_update: 0, to_delete: 0, unchanged: 0 });
            }
            return events;
        }

        // Configure sync options
        let sync_options = if auto_yes {
            SyncOptions::new()
        } else {
            SyncOptions::new().dry_run()
        };

        let mut total_creates = 0;
        let mut total_updates = 0;
        let mut total_deletes = 0;
        let mut total_unchanged = 0;
        let mut total_failed = 0;

        // Sync to each target forge
        for (forge, adapter) in target_forges {
            events.push(WorkspaceEvent::SyncOutput {
                line: format!("Syncing to {}...", forge),
            });

            match SymmetricSyncService::sync(
                &local_forge,
                adapter.as_ref(),
                org_name,
                sync_options.clone(),
            ).await {
                Ok(report) => {
                    // Collect all repo statuses for this forge
                    let mut repo_entries: Vec<RepoStatusEntry> = Vec::new();
                    let mut forge_creates = 0;
                    let mut forge_updates = 0;
                    let mut forge_deletes = 0;
                    let mut forge_unchanged = 0;
                    let mut forge_failed = 0;

                    for result in &report.results {
                        let details = Self::build_action_details(&result.action);

                        match &result.outcome {
                            SyncOutcome::Applied => {
                                if result.action.is_create() {
                                    forge_creates += 1;
                                    total_creates += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::Synced,
                                        details: vec!["created".to_string()],
                                    });
                                    // Update synced state
                                    let url = format!("{}://{}/{}", forge.to_string().to_lowercase(), org_config.owner, result.identity.name);
                                    let _ = storage.update_synced(&result.identity.name, forge.clone(), url, None).await;
                                } else if result.action.is_update() {
                                    forge_updates += 1;
                                    total_updates += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::Synced,
                                        details,
                                    });
                                    // Update synced state for updates too
                                    let url = format!("{}://{}/{}", forge.to_string().to_lowercase(), org_config.owner, result.identity.name);
                                    let _ = storage.update_synced(&result.identity.name, forge.clone(), url, None).await;
                                } else if result.action.is_delete() {
                                    forge_deletes += 1;
                                    total_deletes += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::Synced,
                                        details: vec!["deleted".to_string()],
                                    });
                                }
                            }
                            SyncOutcome::Skipped => {
                                // Dry run - count what would happen
                                if result.action.is_create() {
                                    forge_creates += 1;
                                    total_creates += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::ToCreate,
                                        details: vec![],
                                    });
                                } else if result.action.is_update() {
                                    forge_updates += 1;
                                    total_updates += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::ToUpdate,
                                        details,
                                    });
                                } else if result.action.is_delete() {
                                    forge_deletes += 1;
                                    total_deletes += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: result.identity.name.clone(),
                                        status: RepoSyncStatus::ToDelete,
                                        details: vec![],
                                    });
                                }
                            }
                            SyncOutcome::Failed { error } => {
                                forge_failed += 1;
                                total_failed += 1;
                                repo_entries.push(RepoStatusEntry {
                                    name: result.identity.name.clone(),
                                    status: RepoSyncStatus::Failed,
                                    details: vec![error.clone()],
                                });
                            }
                            SyncOutcome::NoOp => {
                                forge_unchanged += 1;
                                total_unchanged += 1;
                                repo_entries.push(RepoStatusEntry {
                                    name: result.identity.name.clone(),
                                    status: RepoSyncStatus::InSync,
                                    details: vec![],
                                });
                            }
                        }
                    }

                    // Handle repos marked for deletion
                    for repo_name in &repos_to_delete {
                        if auto_yes {
                            // Actually delete the repo from this forge
                            let identity = crate::domain::RepoIdentity::new(org_name, repo_name);
                            match adapter.delete_repo(&identity).await {
                                Ok(_) => {
                                    forge_deletes += 1;
                                    total_deletes += 1;
                                    repo_entries.push(RepoStatusEntry {
                                        name: repo_name.clone(),
                                        status: RepoSyncStatus::Synced,
                                        details: vec!["deleted".to_string()],
                                    });
                                }
                                Err(e) => {
                                    // Not found is OK - means it's already gone
                                    if e.to_string().contains("not found") || e.to_string().contains("404") {
                                        repo_entries.push(RepoStatusEntry {
                                            name: repo_name.clone(),
                                            status: RepoSyncStatus::InSync,
                                            details: vec!["already deleted".to_string()],
                                        });
                                    } else {
                                        forge_failed += 1;
                                        total_failed += 1;
                                        repo_entries.push(RepoStatusEntry {
                                            name: repo_name.clone(),
                                            status: RepoSyncStatus::Failed,
                                            details: vec![format!("delete failed: {}", e)],
                                        });
                                    }
                                }
                            }
                        } else {
                            // Preview mode - just show what would be deleted
                            forge_deletes += 1;
                            total_deletes += 1;
                            repo_entries.push(RepoStatusEntry {
                                name: repo_name.clone(),
                                status: RepoSyncStatus::ToDelete,
                                details: vec![],
                            });
                        }
                    }

                    // Sort repos by status (changes first), then by name
                    repo_entries.sort_by(|a, b| {
                        let status_order = |s: &RepoSyncStatus| match s {
                            RepoSyncStatus::ToCreate => 0,
                            RepoSyncStatus::ToUpdate => 1,
                            RepoSyncStatus::ToDelete => 2,
                            RepoSyncStatus::Failed => 3,
                            RepoSyncStatus::Synced => 4,
                            RepoSyncStatus::InSync => 5,
                        };
                        let order_cmp = status_order(&a.status).cmp(&status_order(&b.status));
                        if order_cmp == std::cmp::Ordering::Equal {
                            a.name.cmp(&b.name)
                        } else {
                            order_cmp
                        }
                    });

                    // Emit grouped event for this forge
                    if auto_yes {
                        events.push(WorkspaceEvent::ForgeSynced {
                            org_name: org_name.to_string(),
                            forge: forge.to_string(),
                            repos: repo_entries,
                            created: forge_creates,
                            updated: forge_updates,
                            deleted: forge_deletes,
                            unchanged: forge_unchanged,
                            failed: forge_failed,
                        });
                    } else {
                        events.push(WorkspaceEvent::ForgePreview {
                            org_name: org_name.to_string(),
                            forge: forge.to_string(),
                            repos: repo_entries,
                            to_create: forge_creates,
                            to_update: forge_updates,
                            to_delete: forge_deletes,
                            in_sync: forge_unchanged,
                        });
                    }
                }
                Err(e) => {
                    events.push(WorkspaceEvent::Error {
                        message: format!("Sync to {} failed: {}", forge, e),
                    });
                    total_failed += repos_config.repos.len();
                }
            }
        }

        // Clean up deleted repos from config after successful sync
        if auto_yes && total_deletes > 0 && total_failed == 0 {
            let _ = storage.remove_deleted_repos().await;
        }

        // Clone repos that don't exist locally (only on actual sync)
        if auto_yes && total_failed == 0 {
            if let Ok(updated_repos) = storage.load_repos().await {
                let default_forges = org_config.forges.all_forges();
                for (repo_name, repo_cfg) in &updated_repos.repos {
                    if repo_cfg.delete { continue; }
                    let repo_path = workspace_path.join(repo_name);
                    if !repo_path.exists() {
                        let forges = repo_cfg.forges.as_ref().unwrap_or(&default_forges);
                        let _ = self.clone_repo(workspace_path, repo_name, &org_config, org_name, forges).await;
                    }
                }
            }
        }

        // Emit completion event
        if auto_yes {
            events.push(WorkspaceEvent::OrgSyncComplete {
                org_name: org_name.to_string(),
                synced: total_creates + total_updates,
                unchanged: total_unchanged,
                failed: total_failed,
            });
        } else {
            events.push(WorkspaceEvent::OrgPreviewComplete {
                org_name: org_name.to_string(),
                to_create: total_creates,
                to_update: total_updates,
                to_delete: total_deletes,
                unchanged: total_unchanged,
            });
        }

        events
    }
}
