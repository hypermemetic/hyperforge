//! Workspace repos router - delegates to ReposActivation after resolving org from path.

use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use hub_core::plexus::{Activation, ChildRouter, PlexusStream, PlexusError};
use hub_macro::hub_methods;

use hub_core::plexus::ChildSummary;
use crate::storage::{HyperforgePaths, GlobalConfig};
use crate::activations::repos::ReposActivation;
use super::service::WorkspaceService;
use super::events::{WorkspaceEvent, WorkspaceRepoInfo};
use crate::events::RepoEvent;

/// Router for workspace repos commands.
/// Resolves the org from --path and delegates to ReposActivation.
pub struct WorkspaceReposRouter {
    paths: Arc<HyperforgePaths>,
}

impl WorkspaceReposRouter {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }

    /// Resolve org from path and get ReposActivation
    async fn resolve_repos_activation(&self, path: &str) -> Result<(ReposActivation, String, PathBuf), String> {
        let cwd = PathBuf::from(path);
        let svc = WorkspaceService::new(self.paths.clone());

        let resolution = svc.resolve_workspace(&cwd).await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No workspace bound at {}", path))?;

        let org_name = resolution.bound_orgs.first()
            .ok_or_else(|| "No org bound to workspace".to_string())?
            .clone();

        let config = GlobalConfig::load(&self.paths).await
            .map_err(|e| e.to_string())?;

        let org_config = config.get_org(&org_name)
            .ok_or_else(|| format!("Org '{}' not found in config", org_name))?
            .clone();

        let activation = ReposActivation::new(
            self.paths.clone(),
            org_name.clone(),
            org_config,
        );

        Ok((activation, org_name, resolution.workspace_path))
    }
}

#[hub_methods(
    namespace = "repos",
    version = "1.0.0",
    description = "Workspace repository management",
    crate_path = "hub_core",
    hub
)]
impl WorkspaceReposRouter {
    /// List repositories in this workspace
    #[hub_method(
        description = "List repositories",
        params(path = "Workspace path", staged = "Show staged repos instead of committed")
    )]
    pub async fn list(&self, path: String, staged: Option<bool>) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((activation, _, _)) => {
                    let mut inner = std::pin::pin!(activation.list(staged).await);
                    while let Some(ev) = futures::StreamExt::next(&mut inner).await {
                        yield ev;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: "unknown".into(),
                        repo_name: None,
                        message: e,
                    };
                }
            }
        }
    }

    /// Create/update a repository configuration
    #[hub_method(
        description = "Create or update a repository",
        params(
            path = "Workspace path",
            repo_name = "Repository name",
            description = "Repository description",
            visibility = "public or private",
            forges = "Comma-separated forge list"
        )
    )]
    pub async fn create(
        &self,
        path: String,
        repo_name: String,
        description: Option<String>,
        visibility: Option<String>,
        forges: Option<String>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((activation, _, _)) => {
                    let mut inner = std::pin::pin!(activation.create(repo_name, description, visibility, forges).await);
                    while let Some(ev) = futures::StreamExt::next(&mut inner).await {
                        yield ev;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: "unknown".into(),
                        repo_name: None,
                        message: e,
                    };
                }
            }
        }
    }

    /// Compare local desired state vs synced state
    #[hub_method(
        description = "Show differences between desired and synced state",
        params(path = "Workspace path", refresh = "Query forges for fresh state")
    )]
    pub async fn diff(&self, path: String, refresh: Option<bool>) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((activation, _, _)) => {
                    let mut inner = std::pin::pin!(activation.diff(refresh).await);
                    while let Some(ev) = futures::StreamExt::next(&mut inner).await {
                        yield ev;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: "unknown".into(),
                        repo_name: None,
                        message: e,
                    };
                }
            }
        }
    }

    /// Update a repository's configuration
    #[hub_method(
        description = "Update repository settings",
        params(
            path = "Workspace path",
            repo_name = "Repository to update",
            visibility = "public or private",
            description = "Repository description",
            protected = "Protect repo from deletion"
        )
    )]
    pub async fn update(
        &self,
        path: String,
        repo_name: String,
        visibility: Option<String>,
        description: Option<String>,
        protected: Option<bool>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((activation, _, _)) => {
                    let mut inner = std::pin::pin!(activation.update(repo_name, visibility, description, protected).await);
                    while let Some(ev) = futures::StreamExt::next(&mut inner).await {
                        yield ev;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: "unknown".into(),
                        repo_name: None,
                        message: e,
                    };
                }
            }
        }
    }

    /// Mark a repository for deletion
    #[hub_method(
        description = "Remove a repository",
        params(
            path = "Workspace path",
            repo_name = "Repository to remove",
            force = "Force removal of protected repos"
        )
    )]
    pub async fn remove(
        &self,
        path: String,
        repo_name: String,
        force: Option<bool>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((activation, _, _)) => {
                    let mut inner = std::pin::pin!(activation.remove(repo_name, force).await);
                    while let Some(ev) = futures::StreamExt::next(&mut inner).await {
                        yield ev;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: "unknown".into(),
                        repo_name: None,
                        message: e,
                    };
                }
            }
        }
    }

    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![]
    }

    /// Show migration status for repos in workspace
    #[hub_method(
        description = "Show which repos are migrated to new SSH approach",
        params(path = "Workspace path")
    )]
    pub async fn status(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let paths = self.paths.clone();
        stream! {
            let router = WorkspaceReposRouter::new(paths);
            match router.resolve_repos_activation(&path).await {
                Ok((_, org_name, workspace_path)) => {
                    let svc = WorkspaceService::new(router.paths.clone());
                    let repos_result = svc.org_storage(&org_name).load_repos().await;

                    match repos_result {
                        Ok(repos_config) => {
                            let mut repo_infos = Vec::new();
                            for (name, cfg) in &repos_config.repos {
                                if cfg.delete { continue; }
                                let repo_path = workspace_path.join(name);
                                let cloned = repo_path.join(".git").exists();
                                let migrated = if cloned {
                                    tokio::process::Command::new("git")
                                        .current_dir(&repo_path)
                                        .args(["config", "--get", "hyperforge.org"])
                                        .output()
                                        .await
                                        .map(|o| o.status.success())
                                        .unwrap_or(false)
                                } else {
                                    false
                                };
                                repo_infos.push(WorkspaceRepoInfo {
                                    name: name.clone(),
                                    cloned,
                                    migrated,
                                });
                            }
                            repo_infos.sort_by(|a, b| a.name.cmp(&b.name));
                            yield WorkspaceEvent::ReposListed {
                                org_name,
                                workspace_path,
                                repos: repo_infos,
                            };
                        }
                        Err(e) => {
                            yield WorkspaceEvent::Error { message: e.to_string() };
                        }
                    }
                }
                Err(e) => {
                    yield WorkspaceEvent::Error { message: e };
                }
            }
        }
    }
}

#[async_trait]
impl ChildRouter for WorkspaceReposRouter {
    fn router_namespace(&self) -> &str {
        "repos"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None
    }
}
