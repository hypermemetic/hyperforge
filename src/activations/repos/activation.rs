//! Thin ReposActivation - delegates to DiffService and SyncService.
//!
//! This activation is responsible for:
//! - Parameter validation
//! - Service construction
//! - Event emission
//!
//! All business logic is delegated to services in `crate::services`.

use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError,
    ChildSummary,
};
use hub_macro::hub_methods;

use crate::storage::{HyperforgePaths, OrgStorage, OrgConfig};
use crate::types::{RepoSummary, RepoConfig, Visibility, Forge};
use crate::domain::{RepoDiff, ForgeAction, SyncPlan};
use crate::services::{DiffService, DiffOptions};

use super::RepoChildRouter;
use super::events::{RepoEvent, DiffStatus};

pub struct ReposActivation {
    paths: Arc<HyperforgePaths>,
    org_name: String,
    org_config: OrgConfig,
}

impl ReposActivation {
    pub fn new(paths: Arc<HyperforgePaths>, org_name: String, org_config: OrgConfig) -> Self {
        Self { paths, org_name, org_config }
    }

    fn storage(&self) -> OrgStorage {
        OrgStorage::new((*self.paths).clone(), self.org_name.clone())
    }

    /// Convert domain RepoDiff to event DiffStatus
    fn diff_to_status(diff: &RepoDiff) -> DiffStatus {
        if diff.marked_for_deletion {
            DiffStatus::ToDelete
        } else if !diff.is_tracked {
            DiffStatus::Untracked
        } else if diff.create_count() > 0 && diff.update_count() == 0 {
            DiffStatus::ToCreate
        } else if diff.update_count() > 0 {
            DiffStatus::ToUpdate
        } else {
            DiffStatus::InSync
        }
    }

    /// Convert domain ForgeAction to human-readable details
    fn action_to_details(action: &ForgeAction) -> String {
        match action {
            ForgeAction::Create { forge, .. } => format!("create on {}", forge),
            ForgeAction::Update { forge, changes } => {
                let mut parts = vec![];
                if changes.visibility.is_some() {
                    parts.push("visibility");
                }
                if changes.description.is_some() {
                    parts.push("description");
                }
                format!("update {} on {}", parts.join(", "), forge)
            }
            ForgeAction::Delete { forge, .. } => format!("delete from {}", forge),
            ForgeAction::NoOp { forge } => format!("in sync on {}", forge),
        }
    }

    /// Emit events from a SyncPlan
    fn emit_plan_events(plan: &SyncPlan, org_name: &str) -> Vec<RepoEvent> {
        let mut events = vec![];

        for diff in &plan.repo_diffs {
            let status = Self::diff_to_status(diff);
            let details: Vec<String> = diff.forge_actions
                .iter()
                .map(Self::action_to_details)
                .collect();

            events.push(RepoEvent::RepoDiff {
                org_name: org_name.to_string(),
                repo_name: diff.name().to_string(),
                status,
                details,
            });
        }

        events.push(RepoEvent::DiffSummary {
            org_name: org_name.to_string(),
            to_create: plan.summary.creates,
            to_update: plan.summary.updates,
            to_delete: plan.summary.deletes,
            in_sync: plan.summary.in_sync,
            untracked: plan.summary.untracked,
        });

        events
    }
}

#[hub_methods(
    namespace = "repos",
    version = "1.0.0",
    description = "Repository management",
    crate_path = "hub_core",
    hub
)]
impl ReposActivation {
    /// List repositories in this organization
    #[hub_method(
        description = "List repositories",
        params(staged = "Show staged repos instead of committed")
    )]
    pub async fn list(&self, staged: Option<bool>) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let storage = self.storage();
        let org_name = self.org_name.clone();
        let show_staged = staged.unwrap_or(false);

        stream! {
            let config = if show_staged {
                storage.load_staged().await
            } else {
                storage.load_repos().await
            };

            match config {
                Ok(repos_config) => {
                    let repos: Vec<RepoSummary> = repos_config.repos
                        .iter()
                        .filter(|(_, cfg)| !cfg.delete)
                        .map(|(name, cfg)| RepoSummary {
                            name: name.clone(),
                            visibility: cfg.visibility.unwrap_or_default(),
                            forges: cfg.forges.clone().unwrap_or_default(),
                            synced: !show_staged,
                        })
                        .collect();

                    yield RepoEvent::Listed {
                        org_name,
                        repos,
                        staged: show_staged,
                    };
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: None,
                        message: e.to_string(),
                    };
                }
            }
        }
    }

    /// Create/update a repository configuration
    #[hub_method(
        description = "Create or update a repository",
        params(
            repo_name = "Repository name",
            description = "Repository description",
            visibility = "public or private",
            forges = "Comma-separated forge list"
        )
    )]
    pub async fn create(
        &self,
        repo_name: String,
        description: Option<String>,
        visibility: Option<String>,
        forges: Option<String>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let storage = self.storage();
        let org_name = self.org_name.clone();
        let org_config = self.org_config.clone();

        stream! {
            // Parse visibility
            let vis = match visibility.as_deref() {
                Some("private") => Some(Visibility::Private),
                Some("public") => Some(Visibility::Public),
                None => None,
                Some(v) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: Some(repo_name),
                        message: format!("Invalid visibility: {}", v),
                    };
                    return;
                }
            };

            // Parse forges
            let forge_list: Option<Vec<Forge>> = match forges {
                Some(f) => {
                    let parsed: Result<Vec<Forge>, _> = f
                        .split(',')
                        .map(|s| s.trim().parse())
                        .collect();
                    match parsed {
                        Ok(list) => Some(list),
                        Err(e) => {
                            yield RepoEvent::Error {
                                org_name,
                                repo_name: Some(repo_name),
                                message: e,
                            };
                            return;
                        }
                    }
                }
                None => Some(org_config.forges.all_forges()),
            };

            let config = RepoConfig {
                description,
                visibility: vis,
                forges: forge_list,
                protected: false,
                delete: false,
                synced: None,
                discovered: None,
            };

            match storage.stage_repo(repo_name.clone(), config).await {
                Ok(()) => {
                    yield RepoEvent::Staged { org_name, repo_name };
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: Some(repo_name),
                        message: e.to_string(),
                    };
                }
            }
        }
    }

    /// Compare local desired state vs synced state (uses DiffService)
    #[hub_method(
        description = "Show differences between desired and synced state",
        params(refresh = "Query forges for fresh state (not yet implemented)")
    )]
    pub async fn diff(
        &self,
        refresh: Option<bool>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let storage = self.storage();
        let _ = refresh; // TODO: implement fresh query

        stream! {
            // Build a StoragePort adapter from our OrgStorage
            let storage_adapter = Arc::new(
                crate::adapters::OrgStorageAdapter::new(storage.clone())
            );

            // Create DiffService with no forge adapters (use cached state)
            let diff_service = DiffService::new(vec![], storage_adapter);

            // Compute the plan using cached state
            let options = DiffOptions::cached();

            match diff_service.compute_plan(&org_name, &options).await {
                Ok(plan) => {
                    for event in Self::emit_plan_events(&plan, &org_name) {
                        yield event;
                    }
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: None,
                        message: e.to_string(),
                    };
                }
            }
        }
    }

    /// Mark a repository for deletion
    #[hub_method(
        description = "Remove a repository",
        params(
            repo_name = "Repository to remove",
            force = "Force removal of protected repos"
        )
    )]
    pub async fn remove(
        &self,
        repo_name: String,
        force: Option<bool>,
    ) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let storage = self.storage();
        let org_name = self.org_name.clone();
        let force_delete = force.unwrap_or(false);

        stream! {
            // Check protection status
            let repos = match storage.load_repos().await {
                Ok(r) => r,
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: Some(repo_name),
                        message: e.to_string(),
                    };
                    return;
                }
            };

            if let Some(config) = repos.repos.get(&repo_name) {
                if config.protected && !force_delete {
                    yield RepoEvent::ProtectionError {
                        org_name,
                        repo_name,
                        message: "Repository is protected. Use --force true to delete.".into(),
                    };
                    return;
                }
            }

            match storage.stage_deletion(repo_name.clone()).await {
                Ok(()) => {
                    yield RepoEvent::MarkedForDeletion { org_name, repo_name };
                }
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name,
                        repo_name: Some(repo_name),
                        message: e.to_string(),
                    };
                }
            }
        }
    }

    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![]
    }
}

#[async_trait]
impl ChildRouter for ReposActivation {
    fn router_namespace(&self) -> &str {
        "repos"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        let storage = self.storage();
        let repos = storage.load_repos().await.ok()?;

        if repos.repos.contains_key(name) {
            Some(Box::new(RepoChildRouter::new(
                self.paths.clone(),
                self.org_name.clone(),
                name.to_string(),
                self.org_config.clone(),
            )))
        } else {
            None
        }
    }
}
