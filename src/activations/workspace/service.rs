//! Workspace service - business logic for workspace operations.
//!
//! This module contains the core workspace logic extracted from the activation.
//! The activation layer handles parameter validation and event emission,
//! while this service handles the actual work.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;

use crate::activations::org::OrgActivation;
use crate::bridge::{GitRemoteBridge, PulumiBridge};
use crate::events::{PulumiEvent, WorkspaceEvent};
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

    /// Get PulumiBridge for sync operations
    pub fn pulumi_bridge(&self) -> PulumiBridge {
        PulumiBridge::new(&self.paths)
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
                if auto_yes {
                    events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: 0, unchanged: 0, failed: 1 });
                } else {
                    events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: 0, to_update: 0, to_delete: 0, unchanged: 0 });
                }
                return events;
            }
        };

        let storage = self.org_storage(org_name);
        let bridge = self.pulumi_bridge();

        // Select Pulumi stack
        if let Err(e) = bridge.select_stack(org_name).await {
            events.push(WorkspaceEvent::Error { message: format!("Failed to select Pulumi stack: {}", e) });
            if auto_yes {
                events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: 0, unchanged: 0, failed: 1 });
            } else {
                events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: 0, to_update: 0, to_delete: 0, unchanged: 0 });
            }
            return events;
        }

        let repos_file = self.repos_file(org_name);
        let staged_file = self.staged_repos_file(org_name);

        // Load and merge repos
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

        // Import existing repos into Pulumi state
        for (repo_name, repo_cfg) in &repos_config.repos {
            if repo_cfg.delete { continue; }
            for forge in [Forge::GitHub, Forge::Codeberg] {
                let (resource_type, resource_id) = match forge {
                    Forge::GitHub => ("github:index/repository:Repository", repo_name.to_string()),
                    Forge::Codeberg => ("gitea:index/repository:Repository", format!("{}/{}", org_config.owner, repo_name)),
                    Forge::GitLab => continue,
                };

                events.push(WorkspaceEvent::PulumiImporting { org_name: org_name.to_string(), repo_name: repo_name.clone(), forge: forge.to_string() });

                match bridge.import_resource(org_name, resource_type, repo_name, &resource_id).await {
                    Ok(imported) => events.push(WorkspaceEvent::PulumiImported { org_name: org_name.to_string(), repo_name: repo_name.clone(), forge: forge.to_string(), imported }),
                    Err(e) if !e.contains("does not exist") && !e.contains("not found") => {
                        events.push(WorkspaceEvent::Error { message: format!("Failed to import {}/{} on {}: {}", org_name, repo_name, forge, e) });
                    }
                    _ => {}
                }
            }
        }

        // Run Pulumi preview or up
        if auto_yes {
            let mut pulumi_stream = Box::pin(bridge.up(org_name, &repos_file, &staged_file, true));
            let mut org_synced = 0;
            let mut org_failed = 0;
            let mut up_success = false;

            while let Some(event) = pulumi_stream.next().await {
                match event {
                    PulumiEvent::Output { line } => events.push(WorkspaceEvent::SyncOutput { line }),
                    PulumiEvent::UpComplete { success, creates, updates, .. } => {
                        up_success = success;
                        if success { org_synced = creates + updates; } else { org_failed = repos_config.repos.len(); }
                    }
                    PulumiEvent::Error { message } => {
                        events.push(WorkspaceEvent::Error { message: format!("{}: {}", org_name, message) });
                        org_failed = repos_config.repos.len();
                    }
                    _ => {}
                }
            }

            // Update synced state and clone new repos
            if up_success {
                if let Ok(outputs) = bridge.get_outputs(org_name).await {
                    for (repo_name, repo_output) in outputs.repos {
                        if let Some(url) = repo_output.github_url {
                            let _ = storage.update_synced(&repo_name, Forge::GitHub, url, repo_output.github_id).await;
                        }
                        if let Some(url) = repo_output.codeberg_url {
                            let _ = storage.update_synced(&repo_name, Forge::Codeberg, url, repo_output.codeberg_id).await;
                        }
                    }
                }

                // Clone repos that don't exist locally
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

            events.push(WorkspaceEvent::OrgSyncComplete { org_name: org_name.to_string(), synced: org_synced, unchanged: repos_config.repos.len().saturating_sub(org_synced), failed: org_failed });
        } else {
            let mut pulumi_stream = Box::pin(bridge.preview(org_name, &repos_file, &staged_file));
            let (mut org_creates, mut org_updates, mut org_deletes, mut org_unchanged) = (0, 0, 0, 0);

            while let Some(event) = pulumi_stream.next().await {
                match event {
                    PulumiEvent::Output { line } => events.push(WorkspaceEvent::PreviewOutput { line }),
                    PulumiEvent::PreviewComplete { creates, updates, deletes, unchanged } => {
                        org_creates = creates;
                        org_updates = updates;
                        org_deletes = deletes;
                        org_unchanged = unchanged;
                    }
                    PulumiEvent::Error { message } => events.push(WorkspaceEvent::Error { message: format!("{}: {}", org_name, message) }),
                    _ => {}
                }
            }

            events.push(WorkspaceEvent::OrgPreviewComplete { org_name: org_name.to_string(), to_create: org_creates, to_update: org_updates, to_delete: org_deletes, unchanged: org_unchanged });
        }

        events
    }
}
