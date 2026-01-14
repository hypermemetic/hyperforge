use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError, ChildSummary,
};
use hub_macro::hub_methods;

use crate::adapters::{GitHubAdapter, CodebergAdapter, YamlStorageAdapter};
use crate::bridge::{KeychainBridge, SshConfigBridge};
use crate::ports::ForgePort;
use crate::services::{ImportService, import::ImportOptions};
use crate::storage::{GlobalConfig, HyperforgePaths, OrgConfig};
use crate::events::OrgEvent;
use crate::types::{Org, OrgSummary, Forge, ForgesConfig, Visibility};

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
                Err(e) => yield OrgEvent::Error { message: e.to_string() },
            }
        }
    }

    /// Show details of a specific organization
    #[hub_method(description = "Show organization details", params(org_name = "Name of the organization"))]
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
                        yield OrgEvent::Error { message: format!("Organization not found: {}", org_name) };
                    }
                }
                Err(e) => yield OrgEvent::Error { message: e.to_string() },
            }
        }
    }

    /// Create a new organization
    #[hub_method(
        description = "Create a new organization",
        params(org_name = "Organization name", owner = "Owner username", ssh_key = "SSH key name",
               origin = "Primary forge", forges = "Comma-separated forges", default_visibility = "Default visibility")
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
            // Parse inputs
            let origin_forge: Forge = match origin.parse() {
                Ok(f) => f,
                Err(e) => { yield OrgEvent::Error { message: e }; return; }
            };
            let forge_list: Result<Vec<Forge>, String> = forges.split(',').map(|s| s.trim().parse()).collect();
            let forge_list = match forge_list {
                Ok(f) => f,
                Err(e) => { yield OrgEvent::Error { message: e }; return; }
            };
            let visibility = match default_visibility.as_deref() {
                Some("private") => Visibility::Private,
                _ => Visibility::Public,
            };

            // Save config
            match GlobalConfig::load(&paths).await {
                Ok(mut config) => {
                    let ssh_key_clone = ssh_key.clone();
                    let forge_list_clone = forge_list.clone();

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

                    yield OrgEvent::Created { org_name: org_name.clone() };

                    // Update SSH config
                    let ssh_bridge = SshConfigBridge::new();
                    match ssh_bridge.update_org(&org_name, &ssh_key_clone, &forge_list_clone).await {
                        Ok(hosts) => yield OrgEvent::SshConfigUpdated { org_name, hosts },
                        Err(e) => yield OrgEvent::Error { message: format!("Failed to update SSH config: {}", e) },
                    }
                }
                Err(e) => yield OrgEvent::Error { message: e.to_string() },
            }
        }
    }

    /// Remove an organization
    #[hub_method(description = "Remove an organization", params(org_name = "Organization name"))]
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
                        yield OrgEvent::Error { message: format!("Organization not found: {}", org_name) };
                    }
                }
                Err(e) => yield OrgEvent::Error { message: e.to_string() },
            }
        }
    }

    /// Import repositories from existing forges
    #[hub_method(
        description = "Initialize local config from existing forge repos",
        params(org_name = "Organization name", include_private = "Include private repos", dry_run = "Preview only")
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
            // Load config
            let config = match GlobalConfig::load(&paths).await {
                Ok(c) => c,
                Err(e) => { yield OrgEvent::Error { message: e.to_string() }; return; }
            };
            let org_config = match config.get_org(&org_name) {
                Some(c) => c.clone(),
                None => { yield OrgEvent::Error { message: format!("Organization not found: {}", org_name) }; return; }
            };

            // Build forge adapters
            let keychain = KeychainBridge::new(&org_name);
            let mut forge_adapters: Vec<Arc<dyn ForgePort>> = Vec::new();
            let mut token_errors: Vec<String> = Vec::new();

            for forge in org_config.forges.all_forges() {
                let token_key = match &forge {
                    Forge::GitHub => "github-token",
                    Forge::Codeberg => "codeberg-token",
                    Forge::GitLab => { token_errors.push("GitLab not yet supported".to_string()); continue; }
                };
                match keychain.get(token_key).await {
                    Ok(Some(token)) => {
                        let adapter: Arc<dyn ForgePort> = match &forge {
                            Forge::GitHub => Arc::new(GitHubAdapter::new(token)),
                            Forge::Codeberg => Arc::new(CodebergAdapter::new(token)),
                            Forge::GitLab => unreachable!(),
                        };
                        forge_adapters.push(adapter);
                    }
                    Ok(None) => token_errors.push(format!("No token for {}", forge)),
                    Err(e) => token_errors.push(format!("Token error for {}: {}", forge, e)),
                }
            }

            yield OrgEvent::ImportStarted { org_name: org_name.clone(), forges: org_config.forges.all_forges() };

            // Report token errors
            for err in token_errors {
                yield OrgEvent::Error { message: err };
            }

            if forge_adapters.is_empty() {
                yield OrgEvent::Error { message: "No forge adapters available".to_string() };
                return;
            }

            // Build storage adapter and import service
            let storage = Arc::new(YamlStorageAdapter::new(&paths.config_dir));
            let import_service = ImportService::new(forge_adapters, storage);

            let options = ImportOptions::all()
                .with_private(include_priv)
                .skip_existing();
            let options = if is_dry_run { options.dry_run() } else { options };

            // Execute import via service
            match import_service.import_org(&org_name, &options).await {
                Ok(result) => {
                    // Emit individual repo events
                    for repo in &result.imported {
                        yield OrgEvent::RepoImported {
                            org_name: org_name.clone(),
                            repo_name: repo.name().to_string(),
                            forges: repo.forges.iter().cloned().collect(),
                            description: repo.description.clone(),
                            visibility: repo.visibility.clone(),
                        };
                    }
                    yield OrgEvent::ImportComplete {
                        org_name,
                        imported_count: result.imported.len(),
                        skipped_count: result.skipped.len(),
                    };
                }
                Err(e) => yield OrgEvent::Error { message: e.to_string() },
            }
        }
    }
}

#[async_trait]
impl ChildRouter for OrgActivation {
    fn router_namespace(&self) -> &str { "org" }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        let config = GlobalConfig::load(&self.paths).await.ok()?;
        let org_config = config.get_org(name)?.clone();
        Some(Box::new(OrgChildRouter::new(self.paths.clone(), name.to_string(), org_config)))
    }
}
