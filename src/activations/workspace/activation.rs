use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;
use std::path::PathBuf;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError,
};
use hub_macro::hub_methods;

use std::collections::HashSet;

use crate::storage::{HyperforgePaths, GlobalConfig, OrgStorage};
use crate::events::WorkspaceEvent;
use crate::types::{WorkspaceBinding, ResolutionSource, RepoConfig};

pub struct WorkspaceActivation {
    paths: Arc<HyperforgePaths>,
}

impl WorkspaceActivation {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }
}

#[hub_methods(
    namespace = "workspace",
    version = "1.0.0",
    description = "Workspace binding management",
    crate_path = "hub_core"
)]
impl WorkspaceActivation {
    /// List all workspace bindings
    #[hub_method(description = "List all workspace bindings")]
    pub async fn list(&self) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            match GlobalConfig::load(&paths).await {
                Ok(config) => {
                    let bindings: Vec<WorkspaceBinding> = config.workspaces
                        .iter()
                        .map(|(path, org)| WorkspaceBinding {
                            path: path.clone(),
                            org_name: org.clone(),
                        })
                        .collect();

                    yield WorkspaceEvent::Listed { bindings };
                }
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Show current workspace resolution
    #[hub_method(
        description = "Show current workspace resolution",
        params(path = "Path to check (defaults to current directory)")
    )]
    pub async fn show(&self, path: Option<String>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            let check_path = match path {
                Some(p) => PathBuf::from(p),
                None => std::env::current_dir().unwrap_or_default(),
            };

            match GlobalConfig::load(&paths).await {
                Ok(config) => {
                    if let Some(org_name) = config.resolve_workspace(&check_path) {
                        // Find the matching workspace path
                        let source_path = config.workspaces
                            .iter()
                            .filter(|(ws_path, _)| {
                                ws_path.canonicalize()
                                    .map(|p| check_path.canonicalize()
                                        .map(|cp| cp.starts_with(&p))
                                        .unwrap_or(false))
                                    .unwrap_or(false)
                            })
                            .max_by_key(|(ws_path, _)| ws_path.components().count())
                            .map(|(p, _)| p.clone());

                        yield WorkspaceEvent::Resolved {
                            org_name,
                            source: ResolutionSource::WorkspaceBinding {
                                path: source_path.unwrap_or_default(),
                            },
                            path: check_path,
                        };
                    } else if let Some(ref default_org) = config.default_org {
                        yield WorkspaceEvent::Resolved {
                            org_name: default_org.clone(),
                            source: ResolutionSource::Default,
                            path: check_path,
                        };
                    } else {
                        yield WorkspaceEvent::NotBound { path: check_path };
                    }
                }
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Bind a directory to an organization
    #[hub_method(
        description = "Bind a directory to an organization",
        params(
            path = "Directory path to bind",
            org_name = "Organization to bind to",
            auto_create = "Scan for git repos and stage them automatically"
        )
    )]
    pub async fn bind(
        &self,
        path: String,
        org_name: String,
        auto_create: Option<bool>,
    ) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            let bind_path = PathBuf::from(&path);

            // Validate org exists
            let config = match GlobalConfig::load(&paths).await {
                Ok(mut config) => {
                    if !config.organizations.contains_key(&org_name) {
                        yield WorkspaceEvent::Error {
                            message: format!("Organization not found: {}", org_name),
                        };
                        return;
                    }

                    // Add binding
                    config.workspaces.insert(bind_path.clone(), org_name.clone());

                    // Save
                    if let Err(e) = config.save(&paths).await {
                        yield WorkspaceEvent::Error { message: e.to_string() };
                        return;
                    }

                    yield WorkspaceEvent::Bound {
                        path: bind_path.clone(),
                        org_name: org_name.clone(),
                    };

                    config
                }
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                    return;
                }
            };

            // Handle auto-create: scan for git repos and stage them
            if auto_create.unwrap_or(false) {
                let discovered_repos = match discover_git_repos(&bind_path).await {
                    Ok(repos) => repos,
                    Err(e) => {
                        yield WorkspaceEvent::Error {
                            message: format!("Failed to scan for git repos: {}", e),
                        };
                        return;
                    }
                };

                if !discovered_repos.is_empty() {
                    yield WorkspaceEvent::ReposDiscovered {
                        path: bind_path.clone(),
                        repos: discovered_repos.clone(),
                    };

                    // Stage each discovered repo
                    let storage = OrgStorage::new((*paths).clone(), org_name.clone());

                    // Get org defaults for forges (convert ForgesConfig to Vec<Forge>)
                    let default_forges = config.get_org(&org_name).map(|o| o.forges.all_forges());

                    for repo_name in discovered_repos {
                        let repo_config = RepoConfig {
                            description: None,
                            visibility: None, // Will use org default
                            forges: default_forges.clone(),
                            protected: false,
                            delete: false,
                            synced: None,
                            discovered: None,
                        };

                        match storage.stage_repo(repo_name.clone(), repo_config).await {
                            Ok(()) => {
                                yield WorkspaceEvent::RepoStaged { repo_name };
                            }
                            Err(e) => {
                                yield WorkspaceEvent::Error {
                                    message: format!("Failed to stage repo: {}", e),
                                };
                            }
                        }
                    }
                }
            }
        }
    }

    /// Remove a workspace binding
    #[hub_method(
        description = "Remove a workspace binding",
        params(path = "Directory path to unbind")
    )]
    pub async fn unbind(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            let unbind_path = PathBuf::from(&path);

            match GlobalConfig::load(&paths).await {
                Ok(mut config) => {
                    if config.workspaces.remove(&unbind_path).is_some() {
                        if let Err(e) = config.save(&paths).await {
                            yield WorkspaceEvent::Error { message: e.to_string() };
                            return;
                        }

                        yield WorkspaceEvent::Unbound { path: unbind_path };
                    } else {
                        yield WorkspaceEvent::NotBound { path: unbind_path };
                    }
                }
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Show diff for all orgs bound to current workspace
    #[hub_method(
        description = "Show diff for all orgs bound to current workspace",
        params(path = "Path to check (defaults to current directory)")
    )]
    pub async fn diff(&self, path: Option<String>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            let check_path = match path {
                Some(p) => PathBuf::from(p),
                None => std::env::current_dir().unwrap_or_default(),
            };

            // Load global config
            let config = match GlobalConfig::load(&paths).await {
                Ok(cfg) => cfg,
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                    return;
                }
            };

            // Resolve workspace to find the workspace path binding
            let workspace_path = match config.resolve_workspace(&check_path) {
                Some(_) => {
                    // Find the actual workspace path (longest prefix match)
                    let canonical = check_path.canonicalize().unwrap_or(check_path.clone());
                    config.workspaces
                        .iter()
                        .filter(|(ws_path, _)| {
                            ws_path.canonicalize()
                                .map(|p| canonical.starts_with(&p))
                                .unwrap_or(false)
                        })
                        .max_by_key(|(ws_path, _)| ws_path.components().count())
                        .map(|(p, _)| p.clone())
                        .unwrap_or(check_path.clone())
                }
                None => {
                    yield WorkspaceEvent::NotBound { path: check_path };
                    return;
                }
            };

            // Find all orgs bound to this workspace path (in practice usually one, but could be multiple)
            // Actually the mapping is path -> org, so one path maps to one org
            // But we should check if the user wants all orgs or just the resolved one
            // For workspace diff, we diff all bound orgs that contain this path
            let bound_orgs: Vec<String> = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    let canonical = check_path.canonicalize().ok();
                    ws_path.canonicalize()
                        .map(|p| canonical.as_ref().map(|c| c.starts_with(&p)).unwrap_or(false))
                        .unwrap_or(false)
                })
                .map(|(_, org)| org.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            if bound_orgs.is_empty() {
                yield WorkspaceEvent::NotBound { path: check_path };
                return;
            }

            yield WorkspaceEvent::DiffStarted {
                workspace_path: workspace_path.clone(),
                org_count: bound_orgs.len(),
            };

            // Aggregate totals
            let mut total_in_sync = 0usize;
            let mut total_to_create = 0usize;
            let mut total_to_update = 0usize;
            let mut total_to_delete = 0usize;

            // Calculate diff for each org
            for org_name in &bound_orgs {
                // Get org config
                let org_config = match config.get_org(org_name) {
                    Some(cfg) => cfg,
                    None => {
                        yield WorkspaceEvent::OrgDiffError {
                            org_name: org_name.clone(),
                            message: format!("Organization config not found: {}", org_name),
                        };
                        continue;
                    }
                };

                let storage = OrgStorage::new((*paths).clone(), org_name.clone());

                // Load repos config (committed)
                let mut repos = match storage.load_repos().await {
                    Ok(r) => r,
                    Err(e) => {
                        yield WorkspaceEvent::OrgDiffError {
                            org_name: org_name.clone(),
                            message: e.to_string(),
                        };
                        continue;
                    }
                };

                // Also include staged repos (pending changes)
                if let Ok(staged) = storage.load_staged().await {
                    for (name, staged_config) in staged.repos {
                        if staged_config.delete {
                            // Mark for deletion in existing entry
                            if let Some(existing) = repos.repos.get_mut(&name) {
                                existing.delete = true;
                            }
                        } else {
                            // Add or replace with staged version
                            repos.repos.insert(name, staged_config);
                        }
                    }
                }

                // Calculate diff statistics for this org
                let mut to_create = 0usize;
                let mut to_update = 0usize;
                let mut to_delete = 0usize;
                let mut in_sync = 0usize;

                for (_, repo_config) in &repos.repos {
                    let desired_forges: HashSet<_> = repo_config.forges
                        .as_ref()
                        .map(|f| f.iter().cloned().collect())
                        .unwrap_or_else(|| org_config.forges.all_forges().into_iter().collect());

                    let synced_forges: HashSet<_> = repo_config.synced
                        .as_ref()
                        .map(|s| s.forges.keys().cloned().collect())
                        .unwrap_or_default();

                    if repo_config.delete {
                        to_delete += 1;
                    } else if synced_forges.is_empty() {
                        to_create += 1;
                    } else if desired_forges != synced_forges {
                        to_update += 1;
                    } else {
                        in_sync += 1;
                    }
                }

                // Emit org result
                yield WorkspaceEvent::OrgDiffResult {
                    org_name: org_name.clone(),
                    in_sync,
                    to_create,
                    to_update,
                    to_delete,
                };

                // Accumulate totals
                total_in_sync += in_sync;
                total_to_create += to_create;
                total_to_update += to_update;
                total_to_delete += to_delete;
            }

            // Emit final summary
            yield WorkspaceEvent::DiffComplete {
                total_orgs: bound_orgs.len(),
                total_in_sync,
                total_to_create,
                total_to_update,
                total_to_delete,
            };
        }
    }

    /// Import repos for all orgs bound to current workspace
    #[hub_method(
        description = "Import repos for all orgs bound to current workspace",
        params(include_private = "Include private repositories")
    )]
    pub async fn import(
        &self,
        include_private: Option<bool>,
    ) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();
        let include_priv = include_private.unwrap_or(false);

        stream! {
            // Get current working directory
            let cwd = match std::env::current_dir() {
                Ok(p) => p,
                Err(e) => {
                    yield WorkspaceEvent::Error {
                        message: format!("Failed to get current directory: {}", e),
                    };
                    return;
                }
            };

            // Load config
            let config = match GlobalConfig::load(&paths).await {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                    return;
                }
            };

            // Resolve workspace to find binding
            let canonical_cwd = match cwd.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    yield WorkspaceEvent::Error {
                        message: format!("Failed to canonicalize path: {}", e),
                    };
                    return;
                }
            };

            // Find the workspace binding that matches this path (longest prefix match)
            let workspace_binding = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    ws_path.canonicalize()
                        .map(|p| canonical_cwd.starts_with(&p))
                        .unwrap_or(false)
                })
                .max_by_key(|(ws_path, _)| ws_path.components().count())
                .map(|(path, org)| (path.clone(), org.clone()));

            let (workspace_path, resolved_org) = match workspace_binding {
                Some((path, org)) => (path, org),
                None => {
                    // Check for default org
                    if let Some(ref default_org) = config.default_org {
                        (cwd.clone(), default_org.clone())
                    } else {
                        yield WorkspaceEvent::NotBound { path: cwd };
                        return;
                    }
                }
            };

            // Collect all unique orgs for workspaces that contain or are contained by this path
            // This allows importing for multiple orgs if workspace has sub-workspace bindings
            let canonical_workspace = workspace_path.canonicalize().unwrap_or(workspace_path.clone());
            let mut orgs_to_import: Vec<String> = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    ws_path.canonicalize()
                        .map(|p| p.starts_with(&canonical_workspace)
                             || canonical_workspace.starts_with(&p))
                        .unwrap_or(false)
                })
                .map(|(_, org)| org.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            // If no sub-workspaces found, use the resolved org
            if orgs_to_import.is_empty() {
                orgs_to_import.push(resolved_org);
            }

            // Sort for consistent output
            orgs_to_import.sort();

            yield WorkspaceEvent::ImportStarted {
                workspace_path: workspace_path.clone(),
                org_count: orgs_to_import.len(),
            };

            let mut total_imported = 0usize;
            let mut total_skipped = 0usize;
            let mut total_errors = 0usize;

            // Import for each org using OrgActivation
            use crate::activations::org::OrgActivation;
            use crate::events::OrgEvent;
            use futures::StreamExt;

            let org_activation = OrgActivation::new(paths.clone());

            for org_name in orgs_to_import {
                yield WorkspaceEvent::OrgImportStarted {
                    org_name: org_name.clone(),
                };

                let mut imported = 0usize;
                let mut skipped = 0usize;
                let mut errors = 0usize;

                // Call org import and collect events
                let mut import_stream = std::pin::pin!(
                    org_activation.import(org_name.clone(), Some(include_priv), None)
                        .await
                );

                while let Some(event) = import_stream.next().await {
                    match event {
                        OrgEvent::ImportComplete {
                            imported_count,
                            skipped_count,
                            ..
                        } => {
                            imported = imported_count;
                            skipped = skipped_count;
                        }
                        OrgEvent::Error { message } => {
                            errors += 1;
                            yield WorkspaceEvent::Error {
                                message: format!("{}: {}", org_name, message),
                            };
                        }
                        _ => {
                            // Other events (ImportStarted, RepoImported) are handled internally
                        }
                    }
                }

                yield WorkspaceEvent::OrgImportComplete {
                    org_name,
                    imported,
                    skipped,
                    errors,
                };

                total_imported += imported;
                total_skipped += skipped;
                total_errors += errors;
            }

            yield WorkspaceEvent::ImportComplete {
                total_imported,
                total_skipped,
                total_errors,
            };
        }
    }

    /// Clone all repos for all orgs bound to current workspace
    #[hub_method(description = "Clone all repos for all orgs bound to current workspace")]
    pub async fn clone_all(&self) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            // Get current working directory
            let cwd = match std::env::current_dir() {
                Ok(p) => p,
                Err(e) => {
                    yield WorkspaceEvent::Error {
                        message: format!("Failed to get current directory: {}", e),
                    };
                    return;
                }
            };

            // Load global config
            let config = match GlobalConfig::load(&paths).await {
                Ok(c) => c,
                Err(e) => {
                    yield WorkspaceEvent::Error {
                        message: format!("Failed to load config: {}", e),
                    };
                    return;
                }
            };

            // Find the workspace binding that contains the cwd
            let workspace_binding = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    ws_path.canonicalize()
                        .map(|p| cwd.canonicalize()
                            .map(|cp| cp.starts_with(&p))
                            .unwrap_or(false))
                        .unwrap_or(false)
                })
                .max_by_key(|(ws_path, _)| ws_path.components().count());

            let (workspace_path, _primary_org) = match workspace_binding {
                Some((path, org)) => (path.clone(), org.clone()),
                None => {
                    yield WorkspaceEvent::NotBound { path: cwd };
                    return;
                }
            };

            // Get all unique orgs bound to this workspace path
            // (In case multiple orgs are bound to the same workspace or subdirectories)
            let bound_orgs: Vec<String> = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    // Include orgs bound to this workspace or any parent/child paths
                    ws_path.canonicalize()
                        .map(|p| {
                            let ws_canonical = workspace_path.canonicalize().unwrap_or_default();
                            p.starts_with(&ws_canonical) || ws_canonical.starts_with(&p)
                        })
                        .unwrap_or(false)
                })
                .map(|(_, org)| org.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            if bound_orgs.is_empty() {
                yield WorkspaceEvent::Error {
                    message: "No organizations bound to this workspace".to_string(),
                };
                return;
            }

            yield WorkspaceEvent::CloneAllStarted { org_count: bound_orgs.len() };

            let mut total_cloned: usize = 0;
            let mut total_skipped: usize = 0;
            let mut total_failed: usize = 0;

            // Clone repos for each org
            for org_name in &bound_orgs {
                yield WorkspaceEvent::OrgCloneAllStarted { org_name: org_name.clone() };

                // Get org config
                let org_config = match config.get_org(org_name) {
                    Some(c) => c.clone(),
                    None => {
                        yield WorkspaceEvent::Error {
                            message: format!("Organization config not found: {}", org_name),
                        };
                        total_failed += 1;
                        continue;
                    }
                };

                // Load repos for this org
                let storage = OrgStorage::new((*paths).clone(), org_name.clone());
                let repos = match storage.load_repos().await {
                    Ok(r) => r,
                    Err(e) => {
                        yield WorkspaceEvent::Error {
                            message: format!("Failed to load repos for {}: {}", org_name, e),
                        };
                        total_failed += 1;
                        continue;
                    }
                };

                let origin_forge = &org_config.origin;
                let mut org_cloned: usize = 0;
                let mut org_skipped: usize = 0;
                let mut org_failed: usize = 0;

                // Clone each repo
                for (repo_name, repo_config) in &repos.repos {
                    if repo_config.delete {
                        continue; // Skip repos marked for deletion
                    }

                    let repo_path = workspace_path.join(repo_name);

                    // Skip if already exists
                    if repo_path.exists() {
                        org_skipped += 1;
                        continue;
                    }

                    // Get forges for this repo
                    let org_forges = org_config.forges.all_forges();
                    let forges = repo_config.forges.as_ref()
                        .unwrap_or(&org_forges);

                    let origin_url = org_config.origin_url(org_name, repo_name);

                    // Run git clone
                    let clone_output = tokio::process::Command::new("git")
                        .args(["clone", &origin_url, repo_path.to_str().unwrap_or(".")])
                        .output()
                        .await;

                    match clone_output {
                        Ok(output) if output.status.success() => {
                            // Add remotes for other forges
                            for forge in forges {
                                if forge == origin_forge {
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

                            org_cloned += 1;
                        }
                        Ok(_output) => {
                            org_failed += 1;
                        }
                        Err(_) => {
                            org_failed += 1;
                        }
                    }
                }

                yield WorkspaceEvent::OrgCloneAllComplete {
                    org_name: org_name.clone(),
                    cloned: org_cloned,
                    skipped: org_skipped,
                    failed: org_failed,
                };

                total_cloned += org_cloned;
                total_skipped += org_skipped;
                total_failed += org_failed;
            }

            yield WorkspaceEvent::CloneAllComplete {
                total_cloned,
                total_skipped,
                total_failed,
            };
        }
    }

    /// Sync repos for all orgs bound to current workspace
    #[hub_method(
        description = "Sync repos for all orgs bound to current workspace",
        params(yes = "Skip confirmation prompts")
    )]
    pub async fn sync(&self, yes: Option<bool>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();
        let auto_yes = yes.unwrap_or(false);

        stream! {
            let check_path = std::env::current_dir().unwrap_or_default();

            // Load global config
            let config = match GlobalConfig::load(&paths).await {
                Ok(cfg) => cfg,
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e.to_string() };
                    return;
                }
            };

            // Resolve workspace to find the workspace path binding
            let workspace_path = match config.resolve_workspace(&check_path) {
                Some(_) => {
                    // Find the actual workspace path (longest prefix match)
                    let canonical = check_path.canonicalize().unwrap_or(check_path.clone());
                    config.workspaces
                        .iter()
                        .filter(|(ws_path, _)| {
                            ws_path.canonicalize()
                                .map(|p| canonical.starts_with(&p))
                                .unwrap_or(false)
                        })
                        .max_by_key(|(ws_path, _)| ws_path.components().count())
                        .map(|(p, _)| p.clone())
                        .unwrap_or(check_path.clone())
                }
                None => {
                    yield WorkspaceEvent::NotBound { path: check_path };
                    return;
                }
            };

            // Find all orgs bound to this workspace path
            let bound_orgs: Vec<String> = config.workspaces
                .iter()
                .filter(|(ws_path, _)| {
                    let canonical = check_path.canonicalize().ok();
                    ws_path.canonicalize()
                        .map(|p| canonical.as_ref().map(|c| c.starts_with(&p)).unwrap_or(false))
                        .unwrap_or(false)
                })
                .map(|(_, org)| org.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            if bound_orgs.is_empty() {
                yield WorkspaceEvent::NotBound { path: check_path };
                return;
            }

            yield WorkspaceEvent::SyncStarted {
                workspace_path: workspace_path.clone(),
                org_count: bound_orgs.len(),
            };

            // Aggregate totals across all orgs
            let mut total_synced = 0usize;
            let mut total_unchanged = 0usize;
            let mut total_failed = 0usize;

            // Sync each org
            for org_name in &bound_orgs {
                yield WorkspaceEvent::OrgSyncStarted {
                    org_name: org_name.clone(),
                };

                // Get org config
                let org_config = match config.get_org(org_name) {
                    Some(cfg) => cfg.clone(),
                    None => {
                        yield WorkspaceEvent::OrgSyncComplete {
                            org_name: org_name.clone(),
                            synced: 0,
                            unchanged: 0,
                            failed: 1,
                        };
                        total_failed += 1;
                        continue;
                    }
                };

                let storage = OrgStorage::new((*paths).clone(), org_name.clone());

                // Count repos before sync to calculate unchanged
                let repos_before = match storage.load_repos().await {
                    Ok(r) => r.repos.len(),
                    Err(_) => 0,
                };

                // Merge staged into committed
                let repos_config = match storage.merge_staged().await {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        yield WorkspaceEvent::Error {
                            message: format!("Failed to merge staged for {}: {}", org_name, e),
                        };
                        yield WorkspaceEvent::OrgSyncComplete {
                            org_name: org_name.clone(),
                            synced: 0,
                            unchanged: 0,
                            failed: 1,
                        };
                        total_failed += 1;
                        continue;
                    }
                };

                let repos_to_sync: Vec<_> = repos_config.repos
                    .iter()
                    .filter(|(_, cfg)| !cfg.delete)
                    .collect();

                if repos_to_sync.is_empty() {
                    yield WorkspaceEvent::OrgSyncComplete {
                        org_name: org_name.clone(),
                        synced: 0,
                        unchanged: repos_before,
                        failed: 0,
                    };
                    total_unchanged += repos_before;
                    continue;
                }

                // Validate/setup git remotes for each repo that exists locally
                let workspace_paths: Vec<PathBuf> = config.workspaces
                    .iter()
                    .filter(|(_, org)| *org == org_name)
                    .map(|(path, _)| path.clone())
                    .collect();

                for (repo_name, repo_cfg) in &repos_to_sync {
                    if repo_cfg.delete {
                        continue;
                    }

                    // Find local repo path in any workspace
                    let local_repo_path = workspace_paths
                        .iter()
                        .map(|ws| ws.join(repo_name))
                        .find(|path| path.join(".git").exists());

                    if let Some(repo_path) = local_repo_path {
                        // Get forges for this repo
                        let org_forges = org_config.forges.all_forges();
                        let forges = repo_cfg.forges.as_ref().unwrap_or(&org_forges);

                        let git_bridge = crate::bridge::GitRemoteBridge::new(
                            repo_path,
                            org_name.clone(),
                            org_config.owner.clone(),
                        );

                        // Setup remotes (ignore errors, just log)
                        let _ = git_bridge.setup_forge_remotes(forges, repo_name).await;
                    }
                }

                // Run Pulumi sync
                let bridge = crate::bridge::PulumiBridge::new(&paths);

                // Select/create stack for this org
                if let Err(e) = bridge.select_stack(org_name).await {
                    yield WorkspaceEvent::Error {
                        message: format!("Failed to select Pulumi stack for {}: {}", org_name, e),
                    };
                    yield WorkspaceEvent::OrgSyncComplete {
                        org_name: org_name.clone(),
                        synced: 0,
                        unchanged: 0,
                        failed: repos_to_sync.len(),
                    };
                    total_failed += repos_to_sync.len();
                    continue;
                }

                let repos_file = paths.repos_file(org_name);
                let staged_file = paths.staged_repos_file(org_name);

                // Run pulumi up
                use futures::StreamExt;
                let pulumi_stream = bridge.up(org_name, &repos_file, &staged_file, auto_yes);
                let mut pulumi_stream = Box::pin(pulumi_stream);

                let mut org_synced = 0usize;
                let mut org_failed = 0usize;
                let mut up_success = false;

                while let Some(event) = pulumi_stream.next().await {
                    match event {
                        crate::events::PulumiEvent::UpComplete { success, creates, updates, .. } => {
                            up_success = success;
                            if success {
                                org_synced = creates + updates;
                            } else {
                                org_failed = repos_to_sync.len();
                                if !auto_yes {
                                    yield WorkspaceEvent::Error {
                                        message: format!(
                                            "{}: Pulumi sync failed. Use --yes true to skip confirmation prompts.",
                                            org_name
                                        ),
                                    };
                                } else {
                                    yield WorkspaceEvent::Error {
                                        message: format!("{}: Pulumi sync failed", org_name),
                                    };
                                }
                            }
                        }
                        crate::events::PulumiEvent::Error { message } => {
                            yield WorkspaceEvent::Error {
                                message: format!("{}: {}", org_name, message),
                            };
                            org_failed = repos_to_sync.len();
                        }
                        _ => {}
                    }
                }

                // Capture outputs after successful apply
                if up_success {
                    if let Ok(outputs) = bridge.get_outputs(org_name).await {
                        for (repo_name, repo_output) in outputs.repos {
                            use crate::types::Forge;
                            if let Some(url) = repo_output.github_url {
                                let _ = storage.update_synced(
                                    &repo_name,
                                    Forge::GitHub,
                                    url,
                                    repo_output.github_id,
                                ).await;
                            }
                            if let Some(url) = repo_output.codeberg_url {
                                let _ = storage.update_synced(
                                    &repo_name,
                                    Forge::Codeberg,
                                    url,
                                    repo_output.codeberg_id,
                                ).await;
                            }
                        }
                    }
                }

                let org_unchanged = repos_before.saturating_sub(org_synced);

                yield WorkspaceEvent::OrgSyncComplete {
                    org_name: org_name.clone(),
                    synced: org_synced,
                    unchanged: org_unchanged,
                    failed: org_failed,
                };

                total_synced += org_synced;
                total_unchanged += org_unchanged;
                total_failed += org_failed;
            }

            // Emit final summary
            yield WorkspaceEvent::SyncComplete {
                workspace_path,
                total_synced,
                total_unchanged,
                total_failed,
            };
        }
    }
}

#[async_trait]
impl ChildRouter for WorkspaceActivation {
    fn router_namespace(&self) -> &str {
        "workspace"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None  // Workspace has no children
    }
}

/// Scan a directory for subdirectories containing .git folders
/// Returns a list of repository names (directory names that contain .git)
async fn discover_git_repos(base_path: &PathBuf) -> std::io::Result<Vec<String>> {
    let mut repos = Vec::new();

    let mut entries = tokio::fs::read_dir(base_path).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        // Only check directories
        if !path.is_dir() {
            continue;
        }

        // Check if this directory contains a .git folder
        let git_path = path.join(".git");
        if git_path.exists() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                repos.push(name.to_string());
            }
        }
    }

    // Sort for consistent output
    repos.sort();

    Ok(repos)
}
