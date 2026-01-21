use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError, ChildSummary,
};
use hub_macro::hub_methods;

use crate::bridge::KeychainBridge;
use crate::storage::{GlobalConfig, HyperforgePaths, OrgConfig, OrgStorage};
use crate::events::OrgEvent;
use crate::types::{Org, OrgSummary, Forge, ForgesConfig, Visibility, ReposConfig, RepoConfig, SyncedState, ForgeSyncedState};

use super::OrgChildRouter;

pub struct OrgActivation {
    paths: Arc<HyperforgePaths>,
}

impl OrgActivation {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }

    /// Get child summaries for schema (orgs are children)
    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        // Load orgs from config synchronously for schema generation
        let config_file = self.paths.config_file();
        if let Ok(contents) = std::fs::read_to_string(&config_file) {
            if let Ok(config) = serde_yaml::from_str::<GlobalConfig>(&contents) {
                return config.organizations
                    .keys()
                    .map(|name| ChildSummary {
                        namespace: name.clone(),
                        description: format!("Organization: {}", name),
                        hash: name.clone(),
                    })
                    .collect();
            }
        }
        vec![]
    }
}

#[hub_methods(
    namespace = "org",
    version = "1.0.0",
    description = "Organization management",
    crate_path = "hub_core",
    hub
)]
impl OrgActivation {
    /// List all configured organizations
    #[hub_method(description = "List all configured organizations")]
    pub async fn list(&self) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            match GlobalConfig::load(&paths).await {
                Ok(config) => {
                    let orgs: Vec<OrgSummary> = config.organizations
                        .iter()
                        .map(|(name, cfg)| OrgSummary {
                            name: name.clone(),
                            owner: cfg.owner.clone(),
                            forges: cfg.forges.clone(),
                        })
                        .collect();

                    yield OrgEvent::Listed { orgs };
                }
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Show details of a specific organization
    #[hub_method(
        description = "Show organization details",
        params(org_name = "Name of the organization")
    )]
    pub async fn show(&self, org_name: String) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            match GlobalConfig::load(&paths).await {
                Ok(config) => {
                    if let Some(cfg) = config.organizations.get(&org_name) {
                        let org = Org {
                            name: org_name.clone(),
                            owner: cfg.owner.clone(),
                            ssh_key: cfg.ssh_key.clone(),
                            origin: cfg.origin.clone(),
                            forges: cfg.forges.clone(),
                            default_visibility: cfg.default_visibility,
                        };
                        yield OrgEvent::Details { org };
                    } else {
                        yield OrgEvent::Error {
                            message: format!("Organization not found: {}", org_name),
                        };
                    }
                }
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Create a new organization
    #[hub_method(
        description = "Create a new organization",
        params(
            org_name = "Organization name",
            owner = "Owner username on forges",
            ssh_key = "SSH key name",
            origin = "Primary forge (github, codeberg)",
            forges = "Comma-separated list of forges",
            default_visibility = "Default repo visibility (public, private)"
        )
    )]
    pub async fn create(
        &self,
        org_name: String,
        owner: String,
        ssh_key: String,
        origin: String,
        forges: String,
        default_visibility: Option<String>,
    ) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            let origin_forge: Forge = match origin.parse() {
                Ok(f) => f,
                Err(e) => {
                    yield OrgEvent::Error { message: e };
                    return;
                }
            };

            let forge_list: Result<Vec<Forge>, String> = forges
                .split(',')
                .map(|s| s.trim().parse())
                .collect();

            let forge_list = match forge_list {
                Ok(f) => f,
                Err(e) => {
                    yield OrgEvent::Error { message: e };
                    return;
                }
            };

            let visibility = match default_visibility.as_deref() {
                Some("private") => Visibility::Private,
                _ => Visibility::Public,
            };

            match GlobalConfig::load(&paths).await {
                Ok(mut config) => {
                    config.organizations.insert(org_name.clone(), OrgConfig {
                        owner,
                        ssh_key,
                        origin: origin_forge,
                        forges: ForgesConfig::from_forges(forge_list),
                        default_visibility: visibility,
                    });

                    if let Err(e) = config.save(&paths).await {
                        yield OrgEvent::Error { message: e.to_string() };
                        return;
                    }

                    yield OrgEvent::Created { org_name };
                }
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Remove an organization
    #[hub_method(
        description = "Remove an organization",
        params(org_name = "Organization name to remove")
    )]
    pub async fn remove(&self, org_name: String) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let paths = self.paths.clone();

        stream! {
            match GlobalConfig::load(&paths).await {
                Ok(mut config) => {
                    if config.organizations.remove(&org_name).is_some() {
                        if let Err(e) = config.save(&paths).await {
                            yield OrgEvent::Error { message: e.to_string() };
                            return;
                        }
                        yield OrgEvent::Removed { org_name };
                    } else {
                        yield OrgEvent::Error {
                            message: format!("Organization not found: {}", org_name),
                        };
                    }
                }
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                }
            }
        }
    }

    /// Import repositories from existing forges
    #[hub_method(
        description = "Initialize local config from existing forge repos",
        params(
            org_name = "Organization name",
            include_private = "Include private repositories",
            dry_run = "Preview without writing config"
        )
    )]
    pub async fn import(
        &self,
        org_name: String,
        include_private: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = OrgEvent> + Send + 'static {
        let paths = self.paths.clone();
        let include_priv = include_private.unwrap_or(false);
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            // Load org config
            let config = match GlobalConfig::load(&paths).await {
                Ok(c) => c,
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                    return;
                }
            };

            let org_config = match config.get_org(&org_name) {
                Some(c) => c.clone(),
                None => {
                    yield OrgEvent::Error {
                        message: format!("Organization not found: {}", org_name),
                    };
                    return;
                }
            };

            yield OrgEvent::ImportStarted {
                org_name: org_name.clone(),
                forges: org_config.forges.all_forges(),
            };

            // Collect repos from all forges
            let mut all_repos: HashMap<String, ImportedRepo> = HashMap::new();

            for forge in org_config.forges.all_forges() {
                let keychain = KeychainBridge::new(&org_name);
                let token_key = match &forge {
                    Forge::GitHub => "github-token",
                    Forge::Codeberg => "codeberg-token",
                    Forge::GitLab => {
                        yield OrgEvent::Error {
                            message: "GitLab import not yet implemented".to_string(),
                        };
                        continue;
                    }
                };

                let token = match keychain.get(token_key).await {
                    Ok(Some(t)) => t,
                    Ok(None) => {
                        yield OrgEvent::Error {
                            message: format!("No token configured for {}", forge),
                        };
                        continue;
                    }
                    Err(e) => {
                        yield OrgEvent::Error {
                            message: format!("Failed to get token for {}: {}", forge, e),
                        };
                        continue;
                    }
                };

                match query_forge_repos_full(&forge, &org_config.owner, &token).await {
                    Ok(repos) => {
                        for repo in repos {
                            // Skip private if not requested
                            if repo.private && !include_priv {
                                continue;
                            }

                            let entry = all_repos
                                .entry(repo.name.clone())
                                .or_insert_with(|| ImportedRepo {
                                    description: repo.description.clone(),
                                    visibility: if repo.private {
                                        Visibility::Private
                                    } else {
                                        Visibility::Public
                                    },
                                    forges: vec![],
                                    urls: HashMap::new(),
                                });

                            entry.forges.push(forge.clone());
                            entry.urls.insert(forge.clone(), repo.url.clone());
                        }
                    }
                    Err(e) => {
                        yield OrgEvent::Error {
                            message: format!("{} query failed: {}", forge, e),
                        };
                    }
                }
            }

            // Build repos config
            let storage = OrgStorage::new((*paths).clone(), org_name.clone());

            // Load existing repos to check for skips
            let existing_repos = match storage.load_repos().await {
                Ok(r) => r,
                Err(e) => {
                    yield OrgEvent::Error { message: e.to_string() };
                    return;
                }
            };

            let mut repos_config = ReposConfig {
                owner: org_config.owner.clone(),
                repos: existing_repos.repos.clone(),
            };

            let mut imported_count = 0;
            let mut skipped_count = 0;

            for (name, imported) in &all_repos {
                // Check if already exists
                if existing_repos.repos.contains_key(name) {
                    skipped_count += 1;
                    continue;
                }

                imported_count += 1;

                yield OrgEvent::RepoImported {
                    org_name: org_name.clone(),
                    repo_name: name.clone(),
                    forges: imported.forges.clone(),
                    description: imported.description.clone(),
                    visibility: imported.visibility,
                };

                // Build config with pre-filled _synced state
                let mut synced = SyncedState::default();
                for (forge, url) in &imported.urls {
                    synced.forges.insert(forge.clone(), ForgeSyncedState {
                        url: url.clone(),
                        id: None, // Could be filled from API response
                        synced_at: chrono::Utc::now(),
                    });
                }

                repos_config.repos.insert(name.clone(), RepoConfig {
                    description: imported.description.clone(),
                    visibility: Some(imported.visibility),
                    forges: Some(imported.forges.clone()),
                    protected: false,
                    delete: false,
                    synced: Some(synced),
                    discovered: None,
                    packages: vec![],
                    build: None,
                });
            }

            // Write config if not dry run
            if !is_dry_run && imported_count > 0 {
                if let Err(e) = storage.save_repos(&repos_config).await {
                    yield OrgEvent::Error { message: e.to_string() };
                    return;
                }
            }

            yield OrgEvent::ImportComplete {
                org_name,
                imported_count,
                skipped_count,
            };
        }
    }
}

/// Repository info discovered from a forge API (full version for import)
struct ForgeRepoFull {
    name: String,
    url: String,
    description: Option<String>,
    private: bool,
}

/// Collected import info for a repository
struct ImportedRepo {
    description: Option<String>,
    visibility: Visibility,
    forges: Vec<Forge>,
    urls: HashMap<Forge, String>,
}

/// Query forge API for list of repositories with full details
async fn query_forge_repos_full(
    forge: &Forge,
    owner: &str,
    token: &str,
) -> Result<Vec<ForgeRepoFull>, String> {
    let client = reqwest::Client::new();

    match forge {
        Forge::GitHub => {
            let url = format!("https://api.github.com/users/{}/repos?per_page=100", owner);
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("User-Agent", "hyperforge")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if !response.status().is_success() {
                return Err(format!("API returned {}", response.status()));
            }

            let repos: Vec<serde_json::Value> = response
                .json()
                .await
                .map_err(|e| e.to_string())?;

            Ok(repos.iter().filter_map(|r| {
                Some(ForgeRepoFull {
                    name: r.get("name")?.as_str()?.to_string(),
                    url: r.get("html_url")?.as_str()?.to_string(),
                    description: r.get("description").and_then(|d| d.as_str()).map(String::from),
                    private: r.get("private").and_then(|p| p.as_bool()).unwrap_or(false),
                })
            }).collect())
        }
        Forge::Codeberg => {
            let url = format!("https://codeberg.org/api/v1/users/{}/repos", owner);
            let response = client
                .get(&url)
                .header("Authorization", format!("token {}", token))
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if !response.status().is_success() {
                return Err(format!("API returned {}", response.status()));
            }

            let repos: Vec<serde_json::Value> = response
                .json()
                .await
                .map_err(|e| e.to_string())?;

            Ok(repos.iter().filter_map(|r| {
                Some(ForgeRepoFull {
                    name: r.get("name")?.as_str()?.to_string(),
                    url: r.get("html_url")?.as_str()?.to_string(),
                    description: r.get("description").and_then(|d| d.as_str()).map(String::from),
                    private: r.get("private").and_then(|p| p.as_bool()).unwrap_or(false),
                })
            }).collect())
        }
        Forge::GitLab => {
            Err("GitLab import not yet implemented".into())
        }
    }
}

#[async_trait]
impl ChildRouter for OrgActivation {
    fn router_namespace(&self) -> &str {
        "org"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        // Load config and check if org exists
        let config = GlobalConfig::load(&self.paths).await.ok()?;

        // Get the org config if it exists - this will be passed down to children
        let org_config = config.get_org(name)?.clone();

        Some(Box::new(OrgChildRouter::new(
            self.paths.clone(),
            name.to_string(),
            org_config,
        )))
    }
}
