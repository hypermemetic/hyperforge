use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError,
};
use hub_macro::hub_methods;

use crate::adapters::{GitHubAdapter, CodebergAdapter, LocalForge};
use crate::bridge::{GitRemoteBridge, KeychainBridge};
use crate::ports::ForgePort;
use crate::services::symmetric_sync::{SymmetricSyncService, SyncOptions, SyncOutcome};
use crate::storage::{HyperforgePaths, OrgStorage, GlobalConfig, OrgConfig};
use crate::events::RepoEvent;
use crate::types::{RepoDetails, Forge};

/// Child router for a specific repository (e.g., org.hypermemetic.repos.substrate)
/// Receives org-level configuration from parent ReposActivation.
pub struct RepoChildRouter {
    paths: Arc<HyperforgePaths>,
    org_name: String,
    repo_name: String,
    /// Organization config passed from parent - avoids reloading from disk
    org_config: OrgConfig,
}

impl RepoChildRouter {
    pub fn new(paths: Arc<HyperforgePaths>, org_name: String, repo_name: String, org_config: OrgConfig) -> Self {
        Self { paths, org_name, repo_name, org_config }
    }

    fn storage(&self) -> OrgStorage {
        OrgStorage::new((*self.paths).clone(), self.org_name.clone())
    }
}

#[hub_methods(
    namespace = "repo",
    version = "1.0.0",
    description = "Repository operations",
    crate_path = "hub_core"
)]
impl RepoChildRouter {
    /// Show repository details
    #[hub_method(description = "Show repository details")]
    pub async fn show(&self) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let storage = self.storage();
        let org_name = self.org_name.clone();
        let repo_name = self.repo_name.clone();

        stream! {
            match storage.load_repos().await {
                Ok(repos) => {
                    if let Some(config) = repos.repos.get(&repo_name) {
                        let details = RepoDetails {
                            name: repo_name.clone(),
                            description: config.description.clone(),
                            visibility: config.visibility.unwrap_or_default(),
                            forge_urls: std::collections::HashMap::new(),
                        };

                        yield RepoEvent::Details {
                            org_name,
                            repo: details,
                        };
                    } else {
                        yield RepoEvent::Error {
                            org_name,
                            repo_name: Some(repo_name),
                            message: "Repository not found".to_string(),
                        };
                    }
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

    /// Sync repository to forges
    #[hub_method(
        description = "Sync repository to forges",
        params(
            dry_run = "Preview changes without applying",
            yes = "Skip confirmation prompts"
        )
    )]
    pub async fn sync(&self, dry_run: Option<bool>, yes: Option<bool>) -> impl Stream<Item = RepoEvent> + Send + 'static {
        let org_name = self.org_name.clone();
        let repo_name = self.repo_name.clone();
        let org_config = self.org_config.clone();
        let paths = self.paths.clone();
        let is_dry_run = dry_run.unwrap_or(false);
        let _auto_yes = yes.unwrap_or(false); // Not needed - SymmetricSyncService doesn't prompt

        stream! {
            yield RepoEvent::SyncStarted {
                org_name: org_name.clone(),
                repo_count: 1,
            };

            // Load global config for workspace bindings (org config comes from parent)
            let global_config = match GlobalConfig::load(&paths).await {
                Ok(cfg) => cfg,
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: org_name.clone(),
                        repo_name: Some(repo_name.clone()),
                        message: format!("Failed to load config: {}", e),
                    };
                    return;
                }
            };

            // Org config comes from parent - no need to look it up

            // Load repos config to get this repo's settings
            let storage = OrgStorage::new((*paths).clone(), org_name.clone());
            let repos_config = match storage.load_repos().await {
                Ok(cfg) => cfg,
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: org_name.clone(),
                        repo_name: Some(repo_name.clone()),
                        message: format!("Failed to load repos: {}", e),
                    };
                    return;
                }
            };

            let repo_config = match repos_config.repos.get(&repo_name) {
                Some(cfg) => cfg.clone(),
                None => {
                    yield RepoEvent::Error {
                        org_name: org_name.clone(),
                        repo_name: Some(repo_name.clone()),
                        message: format!("Repository not found: {}", repo_name),
                    };
                    return;
                }
            };

            // Find workspace paths bound to this org
            let workspace_paths: Vec<PathBuf> = global_config.workspaces
                .iter()
                .filter(|(_, org)| *org == &org_name)
                .map(|(path, _)| path.clone())
                .collect();

            // Get forges for this repo (use repo-specific or org default)
            let org_forges = org_config.forges.all_forges();
            let forges = repo_config.forges.as_ref()
                .unwrap_or(&org_forges);

            // Find local repo path in any workspace
            let local_repo_path = workspace_paths
                .iter()
                .map(|ws| ws.join(&repo_name))
                .find(|path| path.join(".git").exists());

            // Validate git remotes if local repo exists
            if let Some(repo_path) = local_repo_path {
                let git_bridge = GitRemoteBridge::new(
                    repo_path,
                    org_name.clone(),
                    org_config.owner.clone(),
                );

                // Validate/setup remotes
                match git_bridge.setup_forge_remotes(forges, &repo_name).await {
                    Ok(added_remotes) => {
                        // Emit events for any remotes that were added
                        for remote_info in &added_remotes {
                            // Format is "name=url"
                            if let Some((name, url)) = remote_info.split_once('=') {
                                yield RepoEvent::RemoteAdded {
                                    org_name: org_name.clone(),
                                    repo_name: repo_name.clone(),
                                    remote: name.to_string(),
                                    url: url.to_string(),
                                };
                            }
                        }

                        // Emit validation event with all configured remotes
                        let all_remotes: Vec<String> = forges
                            .iter()
                            .map(|f| f.to_string())
                            .collect();

                        yield RepoEvent::RemotesValidated {
                            org_name: org_name.clone(),
                            repo_name: repo_name.clone(),
                            remotes: all_remotes,
                        };
                    }
                    Err(e) => {
                        yield RepoEvent::Error {
                            org_name: org_name.clone(),
                            repo_name: Some(repo_name.clone()),
                            message: format!("Failed to setup git remotes: {}", e),
                        };
                        return;
                    }
                }
            }

            // Build LocalForge from repos.yaml (source of truth)
            let repos_file = paths.repos_file(&org_name);
            let local_forge = match LocalForge::load(&repos_file) {
                Ok(forge) => forge,
                Err(e) => {
                    yield RepoEvent::Error {
                        org_name: org_name.clone(),
                        repo_name: Some(repo_name.clone()),
                        message: format!("Failed to load repos.yaml: {}", e),
                    };
                    return;
                }
            };

            // Build forge adapters for target forges
            let keychain = KeychainBridge::new(&org_name);
            let mut target_forges: Vec<(Forge, Arc<dyn ForgePort>)> = Vec::new();

            for forge in forges.iter() {
                match forge {
                    Forge::Local => continue, // Skip local forge - it's our source
                    Forge::GitLab => {
                        yield RepoEvent::Error {
                            org_name: org_name.clone(),
                            repo_name: Some(repo_name.clone()),
                            message: "GitLab not yet supported".to_string(),
                        };
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
                        target_forges.push((forge.clone(), adapter));
                    }
                    Ok(None) => {
                        yield RepoEvent::Error {
                            org_name: org_name.clone(),
                            repo_name: Some(repo_name.clone()),
                            message: format!("No token configured for {}", forge),
                        };
                    }
                    Err(e) => {
                        yield RepoEvent::Error {
                            org_name: org_name.clone(),
                            repo_name: Some(repo_name.clone()),
                            message: format!("Failed to get {} token: {}", forge, e),
                        };
                    }
                }
            }

            if target_forges.is_empty() {
                yield RepoEvent::Error {
                    org_name: org_name.clone(),
                    repo_name: Some(repo_name.clone()),
                    message: "No forge adapters available for sync".to_string(),
                };
                return;
            }

            yield RepoEvent::SyncProgress {
                org_name: org_name.clone(),
                repo_name: repo_name.clone(),
                stage: "sync".to_string(),
            };

            // Sync to each target forge
            let mut total_synced = 0;
            let mut sync_filter = HashSet::new();
            sync_filter.insert(repo_name.clone());

            let sync_options = SyncOptions::new()
                .filter_repos(sync_filter);
            let sync_options = if is_dry_run { sync_options.dry_run() } else { sync_options };

            for (forge, adapter) in target_forges {
                yield RepoEvent::SyncProgress {
                    org_name: org_name.clone(),
                    repo_name: repo_name.clone(),
                    stage: format!("syncing to {}", forge),
                };

                match SymmetricSyncService::sync(
                    &local_forge,
                    adapter.as_ref(),
                    &org_name,
                    sync_options.clone(),
                ).await {
                    Ok(report) => {
                        // Count successful syncs
                        for result in &report.results {
                            match &result.outcome {
                                SyncOutcome::Applied => total_synced += 1,
                                SyncOutcome::Skipped => {} // dry run
                                SyncOutcome::Failed { error } => {
                                    yield RepoEvent::Error {
                                        org_name: org_name.clone(),
                                        repo_name: Some(result.identity.name.clone()),
                                        message: format!("Failed on {}: {}", forge, error),
                                    };
                                }
                                SyncOutcome::NoOp => {} // already in sync
                            }
                        }
                    }
                    Err(e) => {
                        yield RepoEvent::Error {
                            org_name: org_name.clone(),
                            repo_name: Some(repo_name.clone()),
                            message: format!("Sync to {} failed: {}", forge, e),
                        };
                    }
                }
            }

            yield RepoEvent::SyncComplete {
                org_name: org_name.clone(),
                success: true,
                synced_count: total_synced,
            };
        }
    }
}

#[async_trait]
impl ChildRouter for RepoChildRouter {
    fn router_namespace(&self) -> &str {
        &self.repo_name
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None  // Repos have no children
    }
}
