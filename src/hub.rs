//! HyperforgeHub - Root activation for hyperforge

use async_stream::stream;
use futures::{Stream, StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::{ForgePort, LocalForge, GitHubAdapter, CodebergAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::commands::{init, status, push};
use crate::config::HyperforgeConfig;
use crate::git::{self, Git};
use crate::package::PackageRegistry as PackagePublisher;
use crate::packages;
use crate::services::SymmetricSyncService;
use crate::types::{Forge, Repo, Visibility};

/// Hyperforge event types
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeEvent {
    /// Status information
    Status {
        version: String,
        description: String,
    },
    /// General info message
    Info { message: String },
    /// Error message
    Error { message: String },
    /// Repository information
    Repo {
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Vec<String>,
        protected: bool,
    },
    /// Sync diff result - repo operation
    SyncOp {
        repo_name: String,
        operation: String, // "create", "update", "delete", "in_sync"
        forge: String,
    },
    /// Sync summary
    SyncSummary {
        forge: String,
        total: usize,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
    },
}

/// Root hub for hyperforge operations
#[derive(Clone)]
pub struct HyperforgeHub {
    sync_service: Arc<SymmetricSyncService>,
    /// Cached LocalForge instances per org
    local_forges: Arc<RwLock<HashMap<String, Arc<LocalForge>>>>,
    /// Base config directory
    config_dir: PathBuf,
}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("hyperforge");

        Self {
            sync_service: Arc::new(SymmetricSyncService::new()),
            local_forges: Arc::new(RwLock::new(HashMap::new())),
            config_dir,
        }
    }

    /// Get or create LocalForge for an org with file persistence.
    /// Always re-reads from disk to pick up external changes.
    async fn get_local_forge(&self, org: &str) -> Arc<LocalForge> {
        // Try to get existing — if cached, reload from disk before returning
        let existing = {
            let forges = self.local_forges.read().unwrap();
            forges.get(org).cloned()
        };

        if let Some(forge) = existing {
            let _ = forge.load_from_yaml().await; // always refresh
            return forge;
        }

        // First access: create, load, cache
        let yaml_path = self.config_dir.join("orgs").join(org).join("repos.yaml");
        let forge = Arc::new(LocalForge::with_config_path(org, yaml_path));
        let _ = forge.load_from_yaml().await;

        {
            let mut forges = self.local_forges.write().unwrap();
            forges.insert(org.to_string(), forge.clone());
        }

        forge
    }
}

impl Default for HyperforgeHub {
    fn default() -> Self {
        Self::new()
    }
}

#[plexus_macros::hub_methods(
    namespace = "hyperforge",
    version = "2.0.0",
    description = "Multi-forge repository management",
    crate_path = "plexus_core"
)]
impl HyperforgeHub {
    /// Show hyperforge status
    #[plexus_macros::hub_method(description = "Show hyperforge status and version")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Status {
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "Multi-forge repository management (LFORGE2)".to_string(),
            };
        }
    }

    /// Show version info
    #[plexus_macros::hub_method(description = "Show version information")]
    pub async fn version(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Info {
                message: format!(
                    "hyperforge {} (LFORGE2 - repo-local, git-native)",
                    env!("CARGO_PKG_VERSION")
                ),
            };
        }
    }

    /// Test workspace diff (demonstration)
    #[plexus_macros::hub_method(description = "Test workspace diff with sample data")]
    pub async fn test_diff(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let sync_service = self.sync_service.clone();

        stream! {
            // Create test local and target forges
            let local = Arc::new(LocalForge::new("testorg"));
            let target = Arc::new(LocalForge::new("testorg"));

            // Add some test repos to local
            let repo1 = Repo::new("test-repo-1", Forge::GitHub)
                .with_description("Test repository 1");
            let repo2 = Repo::new("test-repo-2", Forge::Codeberg)
                .with_visibility(Visibility::Private);

            if let Err(e) = local.create_repo("testorg", &repo1).await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create test repo: {}", e),
                };
                return;
            }

            if let Err(e) = local.create_repo("testorg", &repo2).await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create test repo: {}", e),
                };
                return;
            }

            // Compute diff
            match sync_service.diff(local, target, "testorg").await {
                Ok(diff) => {
                    // Yield summary
                    yield HyperforgeEvent::SyncSummary {
                        forge: "test".to_string(),
                        total: diff.ops.len(),
                        to_create: diff.to_create().len(),
                        to_update: diff.to_update().len(),
                        to_delete: diff.to_delete().len(),
                        in_sync: diff.in_sync().len(),
                    };

                    // Yield individual operations
                    for op in diff.ops {
                        yield HyperforgeEvent::SyncOp {
                            repo_name: op.repo.name.clone(),
                            operation: format!("{:?}", op.op).to_lowercase(),
                            forge: "test".to_string(),
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Diff failed: {}", e),
                    };
                }
            }
        }
    }

    /// List repositories for an organization (from LocalForge)
    #[plexus_macros::hub_method(
        description = "List all repositories in the local forge for an organization",
        params(org = "Organization name")
    )]
    pub async fn repos_list(
        &self,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            match local.list_repos(&org).await {
                Ok(repos) => {
                    for repo in repos {
                        yield HyperforgeEvent::Repo {
                            name: repo.name.clone(),
                            description: repo.description.clone(),
                            visibility: format!("{:?}", repo.visibility).to_lowercase(),
                            origin: format!("{:?}", repo.origin).to_lowercase(),
                            mirrors: repo.mirrors.iter()
                                .map(|f| format!("{:?}", f).to_lowercase())
                                .collect(),
                            protected: repo.protected,
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to list repos: {}", e),
                    };
                }
            }
        }
    }

    /// Create a new repository in LocalForge
    #[plexus_macros::hub_method(
        description = "Create a new repository configuration",
        params(
            org = "Organization name",
            name = "Repository name",
            description = "Repository description (optional)",
            visibility = "Repository visibility: public or private",
            origin = "Origin forge: github, codeberg, or gitlab",
            mirrors = "Mirror forges (optional, comma-separated)"
        )
    )]
    pub async fn repos_create(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            // Parse forge from string
            let origin_forge = match origin.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid origin forge: {}. Must be github, codeberg, or gitlab", origin),
                    };
                    return;
                }
            };

            // Parse visibility
            let vis = match visibility.to_lowercase().as_str() {
                "public" => Visibility::Public,
                "private" => Visibility::Private,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid visibility: {}. Must be public or private", visibility),
                    };
                    return;
                }
            };

            // Parse mirrors
            let mirror_forges: Vec<Forge> = if let Some(m) = mirrors {
                m.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| match s.to_lowercase().as_str() {
                        "github" => Some(Forge::GitHub),
                        "codeberg" => Some(Forge::Codeberg),
                        "gitlab" => Some(Forge::GitLab),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Build repo
            let mut repo = Repo::new(name, origin_forge).with_visibility(vis);
            if let Some(desc) = description {
                repo = repo.with_description(desc);
            }
            repo = repo.with_mirrors(mirror_forges);

            // Get or create LocalForge with persistence
            let local = hub.get_local_forge(&org).await;

            match local.create_repo(&org, &repo).await {
                Ok(_) => {
                    // Save to YAML
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Created repository: {}", repo.name),
                    };
                    yield HyperforgeEvent::Repo {
                        name: repo.name.clone(),
                        description: repo.description.clone(),
                        visibility: format!("{:?}", repo.visibility).to_lowercase(),
                        origin: format!("{:?}", repo.origin).to_lowercase(),
                        mirrors: repo.mirrors.iter()
                            .map(|f| format!("{:?}", f).to_lowercase())
                            .collect(),
                        protected: repo.protected,
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create repo: {}", e),
                    };
                }
            }
        }
    }

    /// Update an existing repository
    #[plexus_macros::hub_method(
        description = "Update repository configuration",
        params(
            org = "Organization name",
            name = "Repository name",
            description = "New repository description (optional)",
            visibility = "New visibility: public or private (optional)"
        )
    )]
    pub async fn repos_update(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            // Get existing repo
            let mut repo = match local.get_repo(&org, &name).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to get repo: {}", e),
                    };
                    return;
                }
            };

            // Update fields
            if let Some(desc) = description {
                repo.description = Some(desc);
            }

            if let Some(vis) = visibility {
                repo.visibility = match vis.to_lowercase().as_str() {
                    "public" => Visibility::Public,
                    "private" => Visibility::Private,
                    _ => {
                        yield HyperforgeEvent::Error {
                            message: format!("Invalid visibility: {}. Must be public or private", vis),
                        };
                        return;
                    }
                };
            }

            match local.update_repo(&org, &repo).await {
                Ok(_) => {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Updated repository: {}", repo.name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to update repo: {}", e),
                    };
                }
            }
        }
    }

    /// Delete a repository
    #[plexus_macros::hub_method(
        description = "Delete a repository from local configuration",
        params(
            org = "Organization name",
            name = "Repository name"
        )
    )]
    pub async fn repos_delete(
        &self,
        org: String,
        name: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let local = hub.get_local_forge(&org).await;

            match local.delete_repo(&org, &name).await {
                Ok(_) => {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Deleted repository: {}", name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to delete repo: {}", e),
                    };
                }
            }
        }
    }

    /// Atomic rename primitive: renames on forges, LocalForge, git remotes, config, path deps, and directory
    #[plexus_macros::hub_method(
        description = "Atomic rename: rename on remote forges, update LocalForge, git remotes, per-repo config, workspace path deps, and local directory",
        params(
            org = "Organization name",
            old_name = "Current repository name",
            new_name = "New repository name",
            path = "Path to repo on disk (optional, enables git remote + config + dir rename)",
            workspace_path = "Workspace root for path dependency updates (optional)",
            dry_run = "Preview without applying (optional, default: false)",
            forges = "Comma-separated forges to rename on (optional, defaults to all configured forges)"
        )
    )]
    pub async fn repos_rename(
        &self,
        org: String,
        old_name: String,
        new_name: String,
        path: Option<String>,
        workspace_path: Option<String>,
        dry_run: Option<bool>,
        forges: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            // Step 0: Validate
            if old_name == new_name {
                yield HyperforgeEvent::Info {
                    message: format!("{}Name unchanged: '{}' - nothing to do", mode, old_name),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("{}Renaming '{}' -> '{}'", mode, old_name, new_name),
            };

            // Step 1: Resolve target forges
            let local = hub.get_local_forge(&org).await;
            let repo_in_local = local.get_repo(&org, &old_name).await.ok();

            let target_forges: Vec<Forge> = if let Some(forge_list) = &forges {
                forge_list
                    .split(',')
                    .filter_map(|f| match f.trim().to_lowercase().as_str() {
                        "github" => Some(Forge::GitHub),
                        "codeberg" => Some(Forge::Codeberg),
                        "gitlab" => Some(Forge::GitLab),
                        _ => None,
                    })
                    .collect()
            } else if let Some(ref repo) = repo_in_local {
                repo.all_forges()
            } else {
                vec![Forge::GitHub, Forge::Codeberg, Forge::GitLab]
            };

            yield HyperforgeEvent::Info {
                message: format!("  Target forges: {}", target_forges.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ")),
            };

            // Step 2: Rename on each remote forge (with catch-up detection)
            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            let mut forge_errors = 0u32;
            let mut forge_successes = 0u32;

            for forge in &target_forges {
                let adapter: Box<dyn ForgePort> = match forge {
                    Forge::GitHub => {
                        match GitHubAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  {}: adapter error: {}", forge, e),
                                };
                                forge_errors += 1;
                                continue;
                            }
                        }
                    }
                    Forge::Codeberg => {
                        match CodebergAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  {}: adapter error: {}", forge, e),
                                };
                                forge_errors += 1;
                                continue;
                            }
                        }
                    }
                    Forge::GitLab => {
                        match GitLabAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  {}: adapter error: {}", forge, e),
                                };
                                forge_errors += 1;
                                continue;
                            }
                        }
                    }
                };

                // Catch-up detection: check if new_name already exists
                match adapter.repo_exists(&org, &new_name).await {
                    Ok(true) => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: already renamed, skipping", forge),
                        };
                        forge_successes += 1;
                        continue;
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}: could not check existence ({}), attempting rename", forge, e),
                        };
                    }
                    Ok(false) => {}
                }

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!("  {}: would rename {} -> {}", forge, old_name, new_name),
                    };
                    forge_successes += 1;
                } else {
                    match adapter.rename_repo(&org, &old_name, &new_name).await {
                        Ok(_) => {
                            yield HyperforgeEvent::Info {
                                message: format!("  {}: renamed {} -> {}", forge, old_name, new_name),
                            };
                            forge_successes += 1;
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  {}: rename failed: {}", forge, e),
                            };
                            forge_errors += 1;
                        }
                    }
                }
            }

            // Step 3: Update LocalForge (push old_name into aliases via rename_repo)
            if repo_in_local.is_some() {
                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!("  LocalForge: would update {} -> {}", old_name, new_name),
                    };
                } else {
                    match local.rename_repo(&org, &old_name, &new_name).await {
                        Ok(_) => {
                            if let Err(e) = local.save_to_yaml().await {
                                yield HyperforgeEvent::Error {
                                    message: format!("  LocalForge: failed to save repos.yaml: {}", e),
                                };
                            } else {
                                yield HyperforgeEvent::Info {
                                    message: format!("  LocalForge: updated {} -> {} (alias recorded)", old_name, new_name),
                                };
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  LocalForge: rename failed: {}", e),
                            };
                        }
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: "  LocalForge: repo not registered, skipping".to_string(),
                };
            }

            // Step 4: Update git remotes + per-repo config (only if path provided)
            if let Some(ref repo_path_str) = path {
                let repo_path = std::path::Path::new(repo_path_str);

                if !repo_path.exists() {
                    yield HyperforgeEvent::Error {
                        message: format!("  Repo path does not exist: {}", repo_path_str),
                    };
                } else {
                    // Step 4a: Update git remotes
                    if is_dry_run {
                        yield HyperforgeEvent::Info {
                            message: "  Git remotes: would update remote URLs".to_string(),
                        };
                    } else {
                        match Git::list_remotes(repo_path) {
                            Ok(remotes) => {
                                for remote in &remotes {
                                    if let Some((_forge_name, _remote_org, _repo_name)) = git::parse_remote_url(&remote.fetch_url) {
                                        let new_url = remote.fetch_url
                                            .replace(&format!("/{}.git", old_name), &format!("/{}.git", new_name))
                                            .replace(&format!("/{}", old_name), &format!("/{}", new_name));

                                        if new_url != remote.fetch_url {
                                            match Git::set_remote_url(repo_path, &remote.name, &new_url) {
                                                Ok(_) => {
                                                    yield HyperforgeEvent::Info {
                                                        message: format!("  Git remote '{}': {}", remote.name, new_url),
                                                    };
                                                }
                                                Err(e) => {
                                                    yield HyperforgeEvent::Error {
                                                        message: format!("  Git remote '{}': failed to update: {}", remote.name, e),
                                                    };
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  Git remotes: failed to list: {}", e),
                                };
                            }
                        }
                    }

                    // Step 4b: Update .hyperforge/config.toml
                    if HyperforgeConfig::exists(repo_path) {
                        if is_dry_run {
                            yield HyperforgeEvent::Info {
                                message: format!("  Config: would set repo_name = {}", new_name),
                            };
                        } else {
                            match HyperforgeConfig::load(repo_path) {
                                Ok(mut config) => {
                                    config.repo_name = Some(new_name.clone());
                                    if let Err(e) = config.save(repo_path) {
                                        yield HyperforgeEvent::Error {
                                            message: format!("  Config: failed to save: {}", e),
                                        };
                                    } else {
                                        yield HyperforgeEvent::Info {
                                            message: format!("  Config: repo_name = {}", new_name),
                                        };
                                    }
                                }
                                Err(e) => {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Config: failed to load: {}", e),
                                    };
                                }
                            }
                        }
                    }
                }
            }

            // Step 5: Update workspace path dependencies (only if workspace_path provided)
            if let Some(ref ws_path_str) = workspace_path {
                let ws_path = std::path::Path::new(ws_path_str);
                let registry = packages::PackageRegistry::new();

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!("  Path deps: would update ../{} -> ../{}", old_name, new_name),
                    };
                } else {
                    match registry.update_workspace_path_deps(ws_path, &old_name, &new_name) {
                        Ok(modified) => {
                            if modified.is_empty() {
                                yield HyperforgeEvent::Info {
                                    message: "  Path deps: no changes needed".to_string(),
                                };
                            } else {
                                for p in modified {
                                    yield HyperforgeEvent::Info {
                                        message: format!("  Path deps: updated {}", p.display()),
                                    };
                                }
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Path deps: failed: {}", e),
                            };
                        }
                    }
                }
            }

            // Step 6: Rename local directory (only if path provided, done LAST)
            if let Some(ref repo_path_str) = path {
                let repo_path = std::path::Path::new(repo_path_str);

                if let Some(parent) = repo_path.parent() {
                    let new_dir = parent.join(&new_name);

                    if new_dir.exists() {
                        yield HyperforgeEvent::Error {
                            message: format!("  Directory rename: target already exists: {}", new_dir.display()),
                        };
                    } else if is_dry_run {
                        yield HyperforgeEvent::Info {
                            message: format!("  Directory: would rename {} -> {}", repo_path.display(), new_dir.display()),
                        };
                    } else if repo_path.exists() {
                        match std::fs::rename(repo_path, &new_dir) {
                            Ok(_) => {
                                yield HyperforgeEvent::Info {
                                    message: format!("  Directory: {} -> {}", repo_path.display(), new_dir.display()),
                                };
                            }
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  Directory rename failed: {}", e),
                                };
                            }
                        }
                    }
                }
            }

            // Step 7: Summary
            if forge_errors == 0 {
                yield HyperforgeEvent::Info {
                    message: format!("{}Rename complete: {} -> {} ({} forge(s) updated)", mode, old_name, new_name, forge_successes),
                };
            } else {
                yield HyperforgeEvent::Error {
                    message: format!("{}Rename finished with {} error(s), {} success(es)", mode, forge_errors, forge_successes),
                };
            }
        }
    }

    /// Import repositories from a remote forge
    #[plexus_macros::hub_method(
        description = "Import repository configurations from a remote forge (GitHub, Codeberg, GitLab)",
        params(
            org = "Organization name",
            forge = "Source forge: github, codeberg, or gitlab"
        )
    )]
    pub async fn repos_import(
        &self,
        org: String,
        forge: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            // Parse forge
            let source_forge = match forge.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge),
                    };
                    return;
                }
            };

            // Get forge adapter
            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };
            let adapter: Arc<dyn ForgePort> = match source_forge {
                Forge::GitHub => {
                    match GitHubAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitHub adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::Codeberg => {
                    match CodebergAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create Codeberg adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::GitLab => {
                    match GitLabAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitLab adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Fetching repositories from {} for {}...", forge, org),
            };

            // List repos from remote forge
            let repos = match adapter.list_repos(&org).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to list repos from {}: {}", forge, e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Found {} repositories", repos.len()),
            };

            // Get local forge
            let local = hub.get_local_forge(&org).await;

            // Import each repo
            let mut imported = 0;
            let mut skipped = 0;
            let mut errors = 0;

            for repo in repos {
                // Check if already exists
                let exists = match local.repo_exists(&org, &repo.name).await {
                    Ok(exists) => exists,
                    Err(e) => {
                        errors += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to check if {} exists: {}", repo.name, e),
                        };
                        continue;
                    }
                };

                if exists {
                    skipped += 1;
                    continue;
                }

                // Create in local forge
                match local.create_repo(&org, &repo).await {
                    Ok(_) => {
                        imported += 1;
                        yield HyperforgeEvent::Repo {
                            name: repo.name.clone(),
                            description: repo.description.clone(),
                            visibility: format!("{:?}", repo.visibility).to_lowercase(),
                            origin: format!("{:?}", repo.origin).to_lowercase(),
                            mirrors: repo.mirrors.iter()
                                .map(|f| format!("{:?}", f).to_lowercase())
                                .collect(),
                            protected: repo.protected,
                        };
                    }
                    Err(e) => {
                        errors += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to import {}: {}", repo.name, e),
                        };
                    }
                }
            }

            // Save to YAML
            if let Err(e) = local.save_to_yaml().await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to save repos.yaml: {}", e),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Import complete: {} imported, {} skipped (already exist), {} errors",
                    imported, skipped, errors
                ),
            };
        }
    }

    /// Compute sync diff between local and remote forge(s)
    #[plexus_macros::hub_method(
        description = "Compute diff between local configuration and remote forge(s). Checks all forges for the org if forge is omitted.",
        params(
            org = "Organization name",
            forge = "Target forge: github, codeberg, or gitlab (optional — omit to check all)"
        )
    )]
    pub async fn workspace_diff(
        &self,
        org: String,
        forge: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let sync_service = self.sync_service.clone();

        stream! {
            // Get local forge
            let local = hub.get_local_forge(&org).await;

            // Determine which forges to diff
            let target_forges: Vec<Forge> = if let Some(ref forge_str) = forge {
                match forge_str.to_lowercase().as_str() {
                    "github" => vec![Forge::GitHub],
                    "codeberg" => vec![Forge::Codeberg],
                    "gitlab" => vec![Forge::GitLab],
                    _ => {
                        yield HyperforgeEvent::Error {
                            message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge_str),
                        };
                        return;
                    }
                }
            } else {
                // Collect all unique forges across all repos in this org
                let repos = match local.all_repos() {
                    Ok(r) => r,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to read local repos: {}", e),
                        };
                        return;
                    }
                };
                let mut seen = std::collections::HashSet::new();
                let mut forges = Vec::new();
                for repo in &repos {
                    for f in repo.all_forges() {
                        if seen.insert(f.as_str().to_string()) {
                            forges.push(f);
                        }
                    }
                }
                if forges.is_empty() {
                    yield HyperforgeEvent::Info {
                        message: "No repos in LocalForge — nothing to diff".to_string(),
                    };
                    return;
                }
                forges
            };

            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            for target_forge in &target_forges {
                let forge_name = target_forge.as_str();

                let adapter: Arc<dyn ForgePort> = match target_forge {
                    Forge::GitHub => {
                        match GitHubAdapter::new(auth.clone(), &org) {
                            Ok(a) => Arc::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("Failed to create GitHub adapter: {}", e),
                                };
                                continue;
                            }
                        }
                    }
                    Forge::Codeberg => {
                        match CodebergAdapter::new(auth.clone(), &org) {
                            Ok(a) => Arc::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("Failed to create Codeberg adapter: {}", e),
                                };
                                continue;
                            }
                        }
                    }
                    Forge::GitLab => {
                        match GitLabAdapter::new(auth.clone(), &org) {
                            Ok(a) => Arc::new(a),
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("Failed to create GitLab adapter: {}", e),
                                };
                                continue;
                            }
                        }
                    }
                };

                yield HyperforgeEvent::Info {
                    message: format!("Computing diff with {}...", forge_name),
                };

                match sync_service.diff(local.clone(), adapter, &org).await {
                    Ok(diff) => {
                        yield HyperforgeEvent::SyncSummary {
                            forge: forge_name.to_string(),
                            total: diff.ops.len(),
                            to_create: diff.to_create().len(),
                            to_update: diff.to_update().len(),
                            to_delete: diff.to_delete().len(),
                            in_sync: diff.in_sync().len(),
                        };

                        for op in diff.ops {
                            yield HyperforgeEvent::SyncOp {
                                repo_name: op.repo.name.clone(),
                                operation: format!("{:?}", op.op).to_lowercase(),
                                forge: forge_name.to_string(),
                            };
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Diff with {} failed: {}", forge_name, e),
                        };
                    }
                }
            }
        }
    }

    /// Check sync status across all forges
    #[plexus_macros::hub_method(
        description = "Check sync status for all forges (GitHub and Codeberg)",
        params(
            org = "Organization/username name"
        )
    )]
    pub async fn workspace_sync_status(
        &self,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let sync_service = self.sync_service.clone();
        let hub = self.clone();

        stream! {
            yield HyperforgeEvent::Info {
                message: format!("Checking sync status for '{}'...\n", org),
            };

            let auth: Arc<dyn crate::auth::AuthProvider> = match YamlAuthProvider::new() {
                Ok(a) => Arc::new(a),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            let local = hub.get_local_forge(&org).await;
            let forges: Vec<(&str, Arc<dyn ForgePort>)> = vec![
                ("github", Arc::new(GitHubAdapter::new(auth.clone(), &org).unwrap()) as Arc<dyn ForgePort>),
                ("codeberg", Arc::new(CodebergAdapter::new(auth.clone(), &org).unwrap()) as Arc<dyn ForgePort>),
            ];

            let mut total_in_sync = 0;
            let mut total_to_create = 0;
            let mut total_to_update = 0;
            let mut all_synced = true;

            for (forge_name, adapter) in forges {
                match sync_service.diff(local.clone(), adapter, &org).await {
                    Ok(diff) => {
                        let created = diff.to_create().len();
                        let updated = diff.to_update().len();
                        let in_sync = diff.in_sync().len();

                        total_in_sync += in_sync;
                        total_to_create += created;
                        total_to_update += updated;

                        let status = if created == 0 && updated == 0 {
                            "✓"
                        } else {
                            all_synced = false;
                            "○"
                        };

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{} {}: {} in sync, {} to create, {} to update",
                                status, forge_name, in_sync, created, updated
                            ),
                        };
                    }
                    Err(e) => {
                        all_synced = false;
                        yield HyperforgeEvent::Error {
                            message: format!("✗ {}: {}", forge_name, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: "".to_string(),
            };

            if all_synced {
                yield HyperforgeEvent::Info {
                    message: format!("All synced! {} repos across all forges", total_in_sync),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "Pending: {} to create, {} to update",
                        total_to_create, total_to_update
                    ),
                };
                yield HyperforgeEvent::Info {
                    message: "Run 'workspace_sync' to apply changes".to_string(),
                };
            }
        }
    }

    /// Sync local configuration to a remote forge
    #[plexus_macros::hub_method(
        description = "Sync repositories from local configuration to a remote forge",
        params(
            org = "Organization name",
            forge = "Target forge: github, codeberg, or gitlab",
            dry_run = "Preview changes without applying them (optional, default: false)",
            no_delete = "Skip deletion of repos not in local config (optional, default: true)"
        )
    )]
    pub async fn workspace_sync(
        &self,
        org: String,
        forge: String,
        dry_run: Option<bool>,
        no_delete: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let sync_service = self.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);
        let skip_delete = no_delete.unwrap_or(true); // Default to NOT deleting

        stream! {
            // Parse forge
            let target_forge = match forge.to_lowercase().as_str() {
                "github" => Forge::GitHub,
                "codeberg" => Forge::Codeberg,
                "gitlab" => Forge::GitLab,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge),
                    };
                    return;
                }
            };

            // Get forge adapter
            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };
            let adapter: Arc<dyn ForgePort> = match target_forge {
                Forge::GitHub => {
                    match GitHubAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitHub adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::Codeberg => {
                    match CodebergAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create Codeberg adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
                Forge::GitLab => {
                    match GitLabAdapter::new(auth, &org) {
                        Ok(a) => Arc::new(a),
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to create GitLab adapter: {}", e),
                            };
                            return;
                        }
                    }
                }
            };

            // Get local forge
            let local = hub.get_local_forge(&org).await;

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("[DRY RUN] Computing sync operations for {}...", forge),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Syncing to {}...", forge),
                };
            }

            // Execute sync
            match sync_service.sync_with_options(local, adapter, &org, is_dry_run, skip_delete).await {
                Ok(diff) => {
                    let created = diff.to_create().len();
                    let updated = diff.to_update().len();
                    let deleted = diff.to_delete().len();
                    let in_sync = diff.in_sync().len();
                    let skipped_info = if skip_delete { " (deletes skipped)" } else { "" };

                    yield HyperforgeEvent::Info {
                        message: format!(
                            "{} sync complete: {} created, {} updated, {} deleted, {} in sync{}",
                            if is_dry_run { "[DRY RUN]" } else { "" },
                            created,
                            updated,
                            deleted,
                            in_sync,
                            skipped_info
                        ),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Sync failed: {}", e),
                    };
                }
            }
        }
    }

    /// Initialize hyperforge for a git repository
    #[plexus_macros::hub_method(
        description = "Initialize hyperforge configuration for a repository",
        params(
            path = "Repository path (absolute)",
            forges = "Comma-separated list of forges (github,codeberg,gitlab)",
            org = "Organization/username on forges",
            repo_name = "Repository name (optional, defaults to directory name)",
            visibility = "Repository visibility: public or private (optional, default: public)",
            description = "Repository description (optional)",
            ssh_keys = "SSH keys per forge in format 'forge:path,forge:path' (optional)",
            force = "Force reinitialize even if config exists (optional, default: false)",
            dry_run = "Preview changes without applying (optional, default: false)"
        )
    )]
    pub async fn git_init(
        &self,
        path: String,
        forges: String,
        org: String,
        repo_name: Option<String>,
        visibility: Option<String>,
        description: Option<String>,
        ssh_keys: Option<String>,
        force: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            // Parse forges
            let forge_list: Vec<String> = forges.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if forge_list.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "At least one forge required".to_string(),
                };
                return;
            }

            // Parse visibility
            let vis = match visibility.as_deref() {
                Some("private") => Visibility::Private,
                Some("public") | None => Visibility::Public,
                Some(other) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid visibility: {}. Must be public or private", other),
                    };
                    return;
                }
            };

            // Parse SSH keys
            let mut ssh_key_pairs = Vec::new();
            if let Some(keys_str) = ssh_keys {
                for pair in keys_str.split(',') {
                    let parts: Vec<&str> = pair.trim().split(':').collect();
                    if parts.len() == 2 {
                        ssh_key_pairs.push((parts[0].to_string(), parts[1].to_string()));
                    }
                }
            }

            // Build options
            let mut options = init::InitOptions::new(forge_list)
                .with_org(org)
                .with_visibility(vis);

            if let Some(name) = repo_name {
                options = options.with_repo_name(name);
            }

            if let Some(desc) = description {
                options = options.with_description(desc);
            }

            for (forge, key_path) in ssh_key_pairs {
                options = options.with_ssh_key(forge, key_path);
            }

            if force.unwrap_or(false) {
                options = options.force();
            }

            if dry_run.unwrap_or(false) {
                options = options.dry_run();
            }

            // Run init
            let repo_path = std::path::Path::new(&path);
            match init::init(repo_path, options) {
                Ok(report) => {
                    if report.dry_run {
                        yield HyperforgeEvent::Info {
                            message: "[DRY RUN] Would initialize hyperforge".to_string(),
                        };
                    }

                    if report.git_initialized {
                        yield HyperforgeEvent::Info {
                            message: "Initialized git repository".to_string(),
                        };
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Created config at {}", repo_path.join(".hyperforge/config.toml").display()),
                    };

                    for remote in report.remotes_added {
                        yield HyperforgeEvent::Info {
                            message: format!("Added remote {} → {}", remote.name, remote.url),
                        };
                    }

                    yield HyperforgeEvent::Info {
                        message: "Hyperforge initialized successfully".to_string(),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Init failed: {}", e),
                    };
                }
            }
        }
    }

    /// Show git repository status
    #[plexus_macros::hub_method(
        description = "Show git repository sync status across all configured forges",
        params(
            path = "Repository path (absolute)"
        )
    )]
    pub async fn git_status(
        &self,
        path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let repo_path = std::path::Path::new(&path);

            match status::status(repo_path) {
                Ok(report) => {
                    // Current branch
                    yield HyperforgeEvent::Info {
                        message: format!("On branch: {}", report.branch),
                    };

                    // Working tree status
                    if report.has_changes || report.has_staged {
                        yield HyperforgeEvent::Info {
                            message: "Working tree has changes".to_string(),
                        };
                    } else {
                        yield HyperforgeEvent::Info {
                            message: "Working tree clean".to_string(),
                        };
                    }

                    // Forge status
                    for forge_status in report.forges {
                        let symbol = if forge_status.is_up_to_date() {
                            "✓"
                        } else if forge_status.ahead > 0 && forge_status.behind > 0 {
                            "↕"
                        } else if forge_status.ahead > 0 {
                            "↑"
                        } else if forge_status.behind > 0 {
                            "↓"
                        } else {
                            "✗"
                        };

                        let mut msg = format!("{} {} ({})",
                            symbol,
                            forge_status.forge,
                            forge_status.remote_name
                        );

                        if forge_status.ahead > 0 || forge_status.behind > 0 {
                            msg.push_str(&format!(" ↑{} ↓{}", forge_status.ahead, forge_status.behind));
                        }

                        if let Some(err) = forge_status.error {
                            msg.push_str(&format!(" - {}", err));
                        }

                        yield HyperforgeEvent::Info { message: msg };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Status failed: {}", e),
                    };
                }
            }
        }
    }

    /// Push to configured forges
    #[plexus_macros::hub_method(
        description = "Push current branch to all configured forges",
        params(
            path = "Repository path (absolute)",
            set_upstream = "Set upstream tracking (optional, default: false)",
            force = "Force push (optional, default: false)",
            dry_run = "Preview push without executing (optional, default: false)",
            only_forges = "Only push to specific forges, comma-separated (optional)"
        )
    )]
    pub async fn git_push(
        &self,
        path: String,
        set_upstream: Option<bool>,
        force: Option<bool>,
        dry_run: Option<bool>,
        only_forges: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let repo_path = std::path::Path::new(&path);

            // Build options
            let mut options = push::PushOptions::new();

            if set_upstream.unwrap_or(false) {
                options = options.set_upstream();
            }

            if force.unwrap_or(false) {
                options = options.force();
            }

            if dry_run.unwrap_or(false) {
                options = options.dry_run();
            }

            if let Some(forges_str) = only_forges {
                let forges: Vec<String> = forges_str.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                options = options.only(forges);
            }

            // Execute push
            match push::push(repo_path, options) {
                Ok(report) => {
                    if report.dry_run {
                        yield HyperforgeEvent::Info {
                            message: "[DRY RUN] Would push to forges".to_string(),
                        };
                    }

                    for result in report.results {
                        if result.success {
                            yield HyperforgeEvent::Info {
                                message: format!("✓ Pushed {} to {} ({})",
                                    result.branch,
                                    result.forge,
                                    result.remote_name
                                ),
                            };
                        } else {
                            yield HyperforgeEvent::Error {
                                message: format!("✗ Failed to push to {}: {}",
                                    result.forge,
                                    result.error.as_deref().unwrap_or("unknown error")
                                ),
                            };
                        }
                    }

                    if report.all_success {
                        yield HyperforgeEvent::Info {
                            message: "All pushes succeeded".to_string(),
                        };
                    } else {
                        yield HyperforgeEvent::Error {
                            message: "Some pushes failed".to_string(),
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Push failed: {}", e),
                    };
                }
            }
        }
    }

    /// Verify workspace configuration and health
    #[plexus_macros::hub_method(
        description = "Verify workspace configuration including orgs, SSH keys, and auth tokens",
        params(
            org = "Organization to verify (optional, verifies all if not specified)"
        )
    )]
    pub async fn workspace_verify(
        &self,
        org: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let config_dir = self.config_dir.clone();

        stream! {
            yield HyperforgeEvent::Info {
                message: "Starting workspace verification...".to_string(),
            };

            // Get list of orgs to verify
            let orgs_to_check = if let Some(org_name) = org {
                vec![org_name]
            } else {
                // List all orgs
                let orgs_path = config_dir.join("orgs");
                match tokio::fs::read_dir(&orgs_path).await {
                    Ok(mut entries) => {
                        let mut orgs = Vec::new();
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Some(name) = entry.file_name().to_str() {
                                if name != "." && name != ".." {
                                    orgs.push(name.to_string());
                                }
                            }
                        }
                        orgs
                    }
                    Err(_) => {
                        yield HyperforgeEvent::Error {
                            message: "No organizations configured".to_string(),
                        };
                        return;
                    }
                }
            };

            let mut total_repos = 0;
            let mut total_issues = 0;

            // Verify each org
            for org_name in orgs_to_check {
                yield HyperforgeEvent::Info {
                    message: format!("Verifying org: {}", org_name),
                };

                // Check org repos.yaml exists
                let repos_yaml = config_dir.join("orgs").join(&org_name).join("repos.yaml");
                if !repos_yaml.exists() {
                    yield HyperforgeEvent::Error {
                        message: format!("  ✗ Missing repos.yaml for org: {}", org_name),
                    };
                    total_issues += 1;
                    continue;
                }

                // Load and count repos
                let local_forge = hub.get_local_forge(&org_name).await;
                match local_forge.all_repos() {
                    Ok(repos) => {
                        let repo_count = repos.len();
                        total_repos += repo_count;

                        yield HyperforgeEvent::Info {
                            message: format!("  ✓ Found {} repos in {}", repo_count, org_name),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  ✗ Failed to load repos: {}", e),
                        };
                        total_issues += 1;
                    }
                }

                // Check auth tokens for common forges
                for forge in &["github", "codeberg", "gitlab"] {
                    let _token_key = format!("{}/{}/token", forge, org_name);
                    // Note: We can't directly check auth hub from here without making it async
                    // This would require calling synapse, which is what YamlAuthProvider does
                    yield HyperforgeEvent::Info {
                        message: format!("  ℹ Auth check for {}/{} (use auth hub to verify)", forge, org_name),
                    };
                }
            }

            // Check SSH keys
            let ssh_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".ssh");

            if ssh_dir.exists() {
                let ssh_keys = vec!["hyperforge_ed25519", "id_ed25519", "id_rsa"];
                let mut found_keys = Vec::new();

                for key_name in ssh_keys {
                    let key_path = ssh_dir.join(key_name);
                    if key_path.exists() {
                        found_keys.push(key_name);
                    }
                }

                if found_keys.is_empty() {
                    yield HyperforgeEvent::Error {
                        message: "✗ No SSH keys found in ~/.ssh/".to_string(),
                    };
                    total_issues += 1;
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("✓ Found SSH keys: {}", found_keys.join(", ")),
                    };
                }
            } else {
                yield HyperforgeEvent::Error {
                    message: "✗ ~/.ssh/ directory not found".to_string(),
                };
                total_issues += 1;
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: "=== Verification Summary ===".to_string(),
            };
            yield HyperforgeEvent::Info {
                message: format!("Total repositories: {}", total_repos),
            };
            yield HyperforgeEvent::Info {
                message: format!("Issues found: {}", total_issues),
            };

            if total_issues == 0 {
                yield HyperforgeEvent::Info {
                    message: "✓ Workspace configuration verified successfully!".to_string(),
                };
            } else {
                yield HyperforgeEvent::Error {
                    message: format!("✗ Found {} issues that need attention", total_issues),
                };
            }
        }
    }

    /// Show organization configuration
    #[plexus_macros::hub_method(
        description = "Show configuration for an organization",
        params(
            org = "Organization name"
        )
    )]
    pub async fn config_show(
        &self,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();

        stream! {
            let config_path = config_dir.join("config.yaml");

            if !config_path.exists() {
                yield HyperforgeEvent::Error {
                    message: "No config.yaml found".to_string(),
                };
                return;
            }

            let content = match tokio::fs::read_to_string(&config_path).await {
                Ok(c) => c,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read config.yaml: {}", e),
                    };
                    return;
                }
            };

            let config: serde_yaml::Value = match serde_yaml::from_str(&content) {
                Ok(c) => c,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to parse config.yaml: {}", e),
                    };
                    return;
                }
            };

            // Get organization config
            if let Some(orgs) = config.get("organizations") {
                if let Some(org_config) = orgs.get(&org) {
                    yield HyperforgeEvent::Info {
                        message: format!("Configuration for org '{}':", org),
                    };

                    if let Some(owner) = org_config.get("owner").and_then(|v| v.as_str()) {
                        yield HyperforgeEvent::Info {
                            message: format!("  owner: {}", owner),
                        };
                    }

                    if let Some(ssh_key) = org_config.get("ssh_key").and_then(|v| v.as_str()) {
                        yield HyperforgeEvent::Info {
                            message: format!("  ssh_key: {}", ssh_key),
                        };
                    }

                    if let Some(origin) = org_config.get("origin").and_then(|v| v.as_str()) {
                        yield HyperforgeEvent::Info {
                            message: format!("  origin: {}", origin),
                        };
                    }

                    if let Some(forges) = org_config.get("forges").and_then(|v| v.as_sequence()) {
                        let forge_list: Vec<&str> = forges.iter()
                            .filter_map(|v| v.as_str())
                            .collect();
                        yield HyperforgeEvent::Info {
                            message: format!("  forges: {}", forge_list.join(", ")),
                        };
                    }

                    if let Some(vis) = org_config.get("default_visibility").and_then(|v| v.as_str()) {
                        yield HyperforgeEvent::Info {
                            message: format!("  default_visibility: {}", vis),
                        };
                    }
                } else {
                    yield HyperforgeEvent::Error {
                        message: format!("Organization '{}' not found in config", org),
                    };
                }
            } else {
                yield HyperforgeEvent::Error {
                    message: "No organizations configured".to_string(),
                };
            }
        }
    }

    /// Set SSH key for an organization
    #[plexus_macros::hub_method(
        description = "Set the SSH key for an organization in config.yaml",
        params(
            org = "Organization name",
            ssh_key = "SSH key filename (without path, assumed to be in ~/.ssh/)"
        )
    )]
    pub async fn config_set_ssh_key(
        &self,
        org: String,
        ssh_key: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();

        stream! {
            let config_path = config_dir.join("config.yaml");

            // Check SSH key exists
            let ssh_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".ssh");
            let key_path = ssh_dir.join(&ssh_key);

            if !key_path.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("SSH key not found: {}", key_path.display()),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Found SSH key: {}", key_path.display()),
            };

            // Read existing config
            let content = match tokio::fs::read_to_string(&config_path).await {
                Ok(c) => c,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read config.yaml: {}", e),
                    };
                    return;
                }
            };

            let mut config: serde_yaml::Value = match serde_yaml::from_str(&content) {
                Ok(c) => c,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to parse config.yaml: {}", e),
                    };
                    return;
                }
            };

            // Update SSH key for org
            if let Some(orgs) = config.get_mut("organizations") {
                if let Some(org_config) = orgs.get_mut(&org) {
                    if let Some(mapping) = org_config.as_mapping_mut() {
                        mapping.insert(
                            serde_yaml::Value::String("ssh_key".to_string()),
                            serde_yaml::Value::String(ssh_key.clone()),
                        );
                    }
                } else {
                    yield HyperforgeEvent::Error {
                        message: format!("Organization '{}' not found in config", org),
                    };
                    return;
                }
            } else {
                yield HyperforgeEvent::Error {
                    message: "No organizations configured".to_string(),
                };
                return;
            }

            // Write back config
            let yaml_str = match serde_yaml::to_string(&config) {
                Ok(s) => s,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to serialize config: {}", e),
                    };
                    return;
                }
            };

            if let Err(e) = tokio::fs::write(&config_path, &yaml_str).await {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to write config.yaml: {}", e),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Updated SSH key for org '{}' to '{}'", org, ssh_key),
            };
        }
    }

    /// Update SSH key in all git repos within a workspace
    #[plexus_macros::hub_method(
        description = "Ensure all git repos in a workspace directory use the configured SSH key via hyperforge-ssh",
        params(
            workspace_path = "Path to workspace directory",
            org = "Organization name (to set hyperforge.org in git config)"
        )
    )]
    pub async fn workspace_update_ssh(
        &self,
        workspace_path: String,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            // Find all git repos in workspace
            let mut repos_updated = 0;
            let mut repos_failed = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_dir = path.join(".git");

                if git_dir.exists() {
                    let repo_name = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");

                    // Set hyperforge.org and core.sshCommand
                    let ssh_script = dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(".hypermemetic-infra")
                        .join("scripts")
                        .join("hyperforge-ssh");

                    // Run git config commands
                    let org_result = tokio::process::Command::new("git")
                        .args(["-C", path.to_str().unwrap_or("."), "config", "hyperforge.org", &org])
                        .output()
                        .await;

                    let ssh_result = tokio::process::Command::new("git")
                        .args(["-C", path.to_str().unwrap_or("."), "config", "core.sshCommand", ssh_script.to_str().unwrap_or("")])
                        .output()
                        .await;

                    match (org_result, ssh_result) {
                        (Ok(org_out), Ok(ssh_out)) if org_out.status.success() && ssh_out.status.success() => {
                            repos_updated += 1;
                            yield HyperforgeEvent::Info {
                                message: format!("✓ Updated: {}", repo_name),
                            };
                        }
                        _ => {
                            repos_failed += 1;
                            yield HyperforgeEvent::Error {
                                message: format!("✗ Failed to update: {}", repo_name),
                            };
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Workspace SSH update complete: {} updated, {} failed", repos_updated, repos_failed),
            };
        }
    }

    /// Initialize .hyperforge/config.toml for an existing repository
    #[plexus_macros::hub_method(
        description = "Initialize .hyperforge/config.toml for an existing git repository",
        params(
            path = "Repository path (absolute)",
            org = "Organization name",
            forges = "Comma-separated forges (github,codeberg,gitlab)",
            visibility = "Visibility: public or private (optional, default: public)"
        )
    )]
    pub async fn repo_config_init(
        &self,
        path: String,
        org: String,
        forges: String,
        visibility: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let repo_path = std::path::Path::new(&path);

            if !repo_path.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Path does not exist: {}", path),
                };
                return;
            }

            // Check if .git exists
            if !repo_path.join(".git").exists() {
                yield HyperforgeEvent::Error {
                    message: "Not a git repository (no .git directory)".to_string(),
                };
                return;
            }

            // Check if config already exists
            if HyperforgeConfig::exists(repo_path) {
                yield HyperforgeEvent::Error {
                    message: "Config already exists. Use repo_config_show to view or repo_config_set_ssh_key to modify.".to_string(),
                };
                return;
            }

            // Parse forges
            let forge_list: Vec<String> = forges.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();

            if forge_list.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "At least one forge required".to_string(),
                };
                return;
            }

            // Parse visibility
            let vis = match visibility.as_deref() {
                Some("private") => Visibility::Private,
                Some("public") | None => Visibility::Public,
                Some(other) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid visibility: {}. Must be public or private", other),
                    };
                    return;
                }
            };

            // Get repo name from directory
            let repo_name = repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Create config
            let config = HyperforgeConfig::new(forge_list.clone())
                .with_org(&org)
                .with_repo_name(&repo_name)
                .with_visibility(vis);

            // Save config
            if let Err(e) = config.save(repo_path) {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to save config: {}", e),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Created .hyperforge/config.toml for '{}'", repo_name),
            };
            yield HyperforgeEvent::Info {
                message: format!("  org: {}", org),
            };
            yield HyperforgeEvent::Info {
                message: format!("  forges: {}", forge_list.join(", ")),
            };
        }
    }

    /// Show .hyperforge/config.toml for a repository
    #[plexus_macros::hub_method(
        description = "Show the .hyperforge/config.toml for a repository",
        params(
            path = "Repository path (absolute)"
        )
    )]
    pub async fn repo_config_show(
        &self,
        path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let repo_path = std::path::Path::new(&path);

            if !HyperforgeConfig::exists(repo_path) {
                yield HyperforgeEvent::Error {
                    message: "No .hyperforge/config.toml found. Use repo_config_init to create one.".to_string(),
                };
                return;
            }

            match HyperforgeConfig::load(repo_path) {
                Ok(config) => {
                    yield HyperforgeEvent::Info {
                        message: format!("Configuration for '{}':", config.get_repo_name(repo_path)),
                    };

                    if let Some(ref org) = config.org {
                        yield HyperforgeEvent::Info {
                            message: format!("  org: {}", org),
                        };
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("  forges: {}", config.forges.join(", ")),
                    };

                    yield HyperforgeEvent::Info {
                        message: format!("  visibility: {:?}", config.visibility).to_lowercase(),
                    };

                    if let Some(ref desc) = config.description {
                        yield HyperforgeEvent::Info {
                            message: format!("  description: {}", desc),
                        };
                    }

                    if !config.ssh.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: "  ssh keys:".to_string(),
                        };
                        for (forge, key) in &config.ssh {
                            yield HyperforgeEvent::Info {
                                message: format!("    {}: {}", forge, key),
                            };
                        }
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to load config: {}", e),
                    };
                }
            }
        }
    }

    /// Set SSH key in a repository's .hyperforge/config.toml
    #[plexus_macros::hub_method(
        description = "Set SSH key for a forge in the repository's .hyperforge/config.toml",
        params(
            path = "Repository path (absolute)",
            forge = "Forge name (github, codeberg, gitlab)",
            ssh_key = "SSH key filename (in ~/.ssh/)"
        )
    )]
    pub async fn repo_config_set_ssh_key(
        &self,
        path: String,
        forge: String,
        ssh_key: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let repo_path = std::path::Path::new(&path);

            // Check SSH key exists
            let ssh_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".ssh");
            let key_path = ssh_dir.join(&ssh_key);

            if !key_path.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("SSH key not found: {}", key_path.display()),
                };
                return;
            }

            if !HyperforgeConfig::exists(repo_path) {
                yield HyperforgeEvent::Error {
                    message: "No .hyperforge/config.toml found. Use repo_config_init first.".to_string(),
                };
                return;
            }

            match HyperforgeConfig::load(repo_path) {
                Ok(mut config) => {
                    // Add/update SSH key
                    config.ssh.insert(forge.to_lowercase(), ssh_key.clone());

                    // Save config
                    if let Err(e) = config.save(repo_path) {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save config: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Set SSH key for '{}' to '{}'", forge, ssh_key),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to load config: {}", e),
                    };
                }
            }
        }
    }

    /// Check if a path is a git worktree (has .git file instead of directory)
    fn is_worktree(path: &std::path::Path) -> bool {
        let git_path = path.join(".git");
        git_path.is_file() // Worktrees have a .git file, not directory
    }

    /// Helper: Process a single repo for set_default_branch (runs in parallel)
    async fn process_repo_default_branch(
        path: PathBuf,
        org: String,
        branch: String,
        should_push: bool,
        force_push: bool,
        auth: Arc<dyn crate::auth::AuthProvider>,
    ) -> (String, bool, String) {
        let repo_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let path_str = path.to_str().unwrap_or(".").to_string();

        // Check if branch exists locally
        let branch_exists = tokio::process::Command::new("git")
            .args(["-C", &path_str, "show-ref", "--verify", "--quiet", &format!("refs/heads/{}", branch)])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !branch_exists {
            // Check if we're on a different branch that should be renamed
            let current_branch = tokio::process::Command::new("git")
                .args(["-C", &path_str, "branch", "--show-current"])
                .output()
                .await
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if !current_branch.is_empty() && current_branch != branch {
                // Rename current branch to target branch
                let rename_result = tokio::process::Command::new("git")
                    .args(["-C", &path_str, "branch", "-M", &current_branch, &branch])
                    .output()
                    .await;

                if rename_result.map(|o| !o.status.success()).unwrap_or(true) {
                    return (repo_name, false, format!("failed to rename branch {} to {}", current_branch, branch));
                }
            }
        }

        // Get list of remotes
        let remotes_output = tokio::process::Command::new("git")
            .args(["-C", &path_str, "remote"])
            .output()
            .await;

        let remotes: Vec<String> = remotes_output
            .ok()
            .and_then(|o| if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            })
            .map(|s| s.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();

        let mut remote_results = Vec::new();

        for remote in &remotes {
            // Determine forge type from remote URL
            let url_output = tokio::process::Command::new("git")
                .args(["-C", &path_str, "remote", "get-url", remote])
                .output()
                .await;

            let url = url_output
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            let forge_type = if url.contains("github.com") {
                Some(Forge::GitHub)
            } else if url.contains("codeberg.org") {
                Some(Forge::Codeberg)
            } else if url.contains("gitlab.com") {
                Some(Forge::GitLab)
            } else {
                None
            };

            // Push the branch if requested
            if should_push {
                let push_args = if force_push {
                    vec!["-C", &path_str, "push", "-u", "--force-with-lease", remote, &branch]
                } else {
                    vec!["-C", &path_str, "push", "-u", remote, &branch]
                };

                let push_result = tokio::process::Command::new("git")
                    .args(&push_args)
                    .output()
                    .await;

                match push_result {
                    Ok(output) if !output.status.success() => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        // Capture full error for detailed output
                        let full_error = stderr.trim().to_string();
                        remote_results.push(format!("{}:push-failed", remote));
                        // Log full error separately if it contains useful info
                        if !full_error.is_empty() {
                            remote_results.push(format!("\n  └─ {}: {}", remote, full_error.replace('\n', "\n     ")));
                        }
                        continue;
                    }
                    Err(e) => {
                        remote_results.push(format!("{}:push-failed({})", remote, e));
                        continue;
                    }
                    _ => {}
                }
            }

            // Set default branch on remote via API
            if let Some(forge) = forge_type {
                let adapter: Box<dyn ForgePort> = match forge {
                    Forge::GitHub => {
                        match GitHubAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(_) => {
                                remote_results.push(format!("{}:auth-failed", remote));
                                continue;
                            }
                        }
                    }
                    Forge::Codeberg => {
                        match CodebergAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(_) => {
                                remote_results.push(format!("{}:auth-failed", remote));
                                continue;
                            }
                        }
                    }
                    Forge::GitLab => {
                        match GitLabAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(_) => {
                                remote_results.push(format!("{}:auth-failed", remote));
                                continue;
                            }
                        }
                    }
                };

                match adapter.set_default_branch(&org, &repo_name, &branch).await {
                    Ok(_) => {
                        remote_results.push(format!("{}:✓", remote));
                    }
                    Err(e) => {
                        remote_results.push(format!("{}:api-failed({})", remote, e));
                    }
                }
            } else {
                remote_results.push(format!("{}:unknown-forge", remote));
            }
        }

        // Update .hyperforge/config.toml with default_branch
        if HyperforgeConfig::exists(&path) {
            if let Ok(mut config) = HyperforgeConfig::load(&path) {
                config.default_branch = Some(branch.clone());
                let _ = config.save(&path);
            }
        }

        let success = remote_results.iter().all(|r| r.contains(":✓")) || remotes.is_empty();
        let status = if remotes.is_empty() {
            "no remotes".to_string()
        } else {
            remote_results.join(" ")
        };

        (repo_name, success, status)
    }

    /// Set default branch for all repos in a workspace (local + remote) - PARALLEL
    #[plexus_macros::hub_method(
        description = "Set the default branch for all repos in a workspace, including on remote forges",
        params(
            workspace_path = "Path to workspace directory",
            org = "Organization name",
            branch = "Branch name to set as default (e.g., 'main')",
            push = "Push the branch to remotes before setting default (optional, default: true)",
            force = "Force push with --force-with-lease (optional, default: false)",
            dry_run = "Preview changes without applying (optional, default: false)"
        )
    )]
    pub async fn workspace_set_default_branch(
        &self,
        workspace_path: String,
        org: String,
        branch: String,
        push: Option<bool>,
        force: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let should_push = push.unwrap_or(true);
        let force_push = force.unwrap_or(false);
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            // Get auth provider for remote operations
            let auth: Arc<dyn crate::auth::AuthProvider> = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            // Phase 1: Collect all repo paths
            let mut repo_paths = Vec::new();
            let mut repos_skipped = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                if !git_path.exists() {
                    continue;
                }

                // Skip worktrees
                if Self::is_worktree(&path) {
                    repos_skipped += 1;
                    continue;
                }

                repo_paths.push(path);
            }

            if is_dry_run {
                for path in &repo_paths {
                    let repo_name = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    yield HyperforgeEvent::Info {
                        message: format!("[DRY RUN] Would set default branch to '{}' for {}", branch, repo_name),
                    };
                }
                yield HyperforgeEvent::Info {
                    message: format!(
                        "[DRY RUN] Set default branch '{}': {} would be updated, {} skipped (worktrees)",
                        branch, repo_paths.len(), repos_skipped
                    ),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Processing {} repos in parallel...", repo_paths.len()),
            };

            // Phase 2: Process all repos in parallel
            let futures: Vec<_> = repo_paths.into_iter().map(|path| {
                let org = org.clone();
                let branch = branch.clone();
                let auth = auth.clone();
                Self::process_repo_default_branch(path, org, branch, should_push, force_push, auth)
            }).collect();

            let results = join_all(futures).await;

            // Phase 3: Yield results
            let mut repos_updated = 0;
            let mut repos_failed = 0;

            for (repo_name, success, status) in results {
                if success {
                    repos_updated += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("✓ {} [{}]", repo_name, status),
                    };
                } else {
                    repos_failed += 1;
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} [{}]", repo_name, status),
                    };
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Set default branch '{}': {} updated, {} failed, {} skipped (worktrees)",
                    branch, repos_updated, repos_failed, repos_skipped
                ),
            };
        }
    }

    /// Check for dirty (uncommitted changes) repos in a workspace
    #[plexus_macros::hub_method(
        description = "Check for repos with uncommitted changes in a workspace",
        params(
            workspace_path = "Path to workspace directory",
            include_worktrees = "Include worktrees in check (optional, default: false)"
        )
    )]
    pub async fn workspace_status(
        &self,
        workspace_path: String,
        include_worktrees: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let check_worktrees = include_worktrees.unwrap_or(false);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mut repos_clean = 0;
            let mut repos_dirty = 0;
            let mut repos_skipped = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                if !git_path.exists() {
                    continue;
                }

                let repo_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Skip worktrees unless requested
                let is_wt = Self::is_worktree(&path);
                if is_wt && !check_worktrees {
                    repos_skipped += 1;
                    continue;
                }

                let path_str = path.to_str().unwrap_or(".");
                let suffix = if is_wt { " (worktree)" } else { "" };

                // Check for uncommitted changes using git status --porcelain
                let status_output = tokio::process::Command::new("git")
                    .args(["-C", path_str, "status", "--porcelain"])
                    .output()
                    .await;

                let has_changes = status_output
                    .ok()
                    .map(|o| !o.stdout.is_empty())
                    .unwrap_or(false);

                // Check for unpushed commits
                let unpushed_output = tokio::process::Command::new("git")
                    .args(["-C", path_str, "log", "--oneline", "@{u}..HEAD"])
                    .output()
                    .await;

                let has_unpushed = unpushed_output
                    .ok()
                    .and_then(|o| if o.status.success() {
                        Some(!o.stdout.is_empty())
                    } else {
                        None // No upstream, can't check
                    })
                    .unwrap_or(false);

                // Get current branch
                let branch_output = tokio::process::Command::new("git")
                    .args(["-C", path_str, "branch", "--show-current"])
                    .output()
                    .await;

                let branch = branch_output
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "?".to_string());

                if has_changes || has_unpushed {
                    repos_dirty += 1;
                    let mut status_parts = Vec::new();
                    if has_changes {
                        status_parts.push("uncommitted");
                    }
                    if has_unpushed {
                        status_parts.push("unpushed");
                    }
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} [{}] ({}){}", repo_name, branch, status_parts.join(", "), suffix),
                    };
                } else {
                    repos_clean += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("✓ {} [{}]{}", repo_name, branch, suffix),
                    };
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Workspace status: {} clean, {} dirty, {} skipped",
                    repos_clean, repos_dirty, repos_skipped
                ),
            };
        }
    }

    /// Fetch/pull from all remotes for all repos in a workspace - PARALLEL with throttling
    #[plexus_macros::hub_method(
        description = "Fetch or pull from all remotes for all repos in a workspace",
        params(
            workspace_path = "Path to workspace directory",
            pull = "Pull (merge) instead of just fetch (optional, default: false)",
            max_parallel = "Maximum parallel connections (optional, default: 5)"
        )
    )]
    pub async fn workspace_pull(
        &self,
        workspace_path: String,
        pull: Option<bool>,
        max_parallel: Option<usize>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let should_pull = pull.unwrap_or(false);
        let parallel_limit = max_parallel.unwrap_or(5);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            // Phase 1: Collect repos
            let mut repo_paths = Vec::new();
            let mut repos_skipped = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                if !git_path.exists() {
                    continue;
                }

                // Skip worktrees
                if Self::is_worktree(&path) {
                    repos_skipped += 1;
                    continue;
                }

                repo_paths.push(path);
            }

            yield HyperforgeEvent::Info {
                message: format!("Processing {} repos (max {} parallel)...", repo_paths.len(), parallel_limit),
            };

            // Helper closure for processing a single repo
            async fn process_repo(path: PathBuf, action: &str) -> (PathBuf, String, bool, String) {
                let repo_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let path_str = path.to_str().unwrap_or(".").to_string();

                // Get list of remotes
                let remotes_output = tokio::process::Command::new("git")
                    .args(["-C", &path_str, "remote"])
                    .output()
                    .await;

                let remotes: Vec<String> = remotes_output
                    .ok()
                    .and_then(|o| if o.status.success() {
                        String::from_utf8(o.stdout).ok()
                    } else {
                        None
                    })
                    .map(|s| s.lines().map(|l| l.to_string()).collect())
                    .unwrap_or_default();

                if remotes.is_empty() {
                    return (path, repo_name, true, "no remotes".to_string());
                }

                let mut remote_results = Vec::new();

                for remote in &remotes {
                    let args = if action == "pull" {
                        vec!["-C", &path_str, "pull", remote]
                    } else {
                        vec!["-C", &path_str, "fetch", remote]
                    };

                    let result = tokio::process::Command::new("git")
                        .args(&args)
                        .output()
                        .await;

                    match result {
                        Ok(output) if output.status.success() => {
                            remote_results.push(format!("{}:✓", remote));
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let error_msg = stderr.lines()
                                .find(|l| l.contains("error:") || l.contains("fatal:") || l.contains("Could not"))
                                .or_else(|| stderr.lines().filter(|l| !l.trim().is_empty()).last())
                                .unwrap_or("failed")
                                .trim()
                                .to_string();
                            remote_results.push(format!("{}:✗({})", remote, error_msg));
                        }
                        Err(e) => {
                            remote_results.push(format!("{}:✗({})", remote, e));
                        }
                    }
                }

                let success = remote_results.iter().all(|r| r.contains(":✓"));
                (path, repo_name, success, remote_results.join(" "))
            }

            // Phase 2: Process in batches with throttling
            let action = if should_pull { "pull" } else { "fetch" };
            let mut all_results = Vec::new();
            let mut failed_paths = Vec::new();

            for chunk in repo_paths.chunks(parallel_limit) {
                let futures: Vec<_> = chunk.iter().map(|path| {
                    process_repo(path.clone(), action)
                }).collect();

                let batch_results = futures::future::join_all(futures).await;

                for (path, repo_name, success, status) in batch_results {
                    if !success {
                        failed_paths.push(path);
                    }
                    all_results.push((repo_name, success, status));
                }
            }

            // Phase 3: Retry failures (one at a time to avoid connection issues)
            if !failed_paths.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("Retrying {} failed repos...", failed_paths.len()),
                };

                for path in failed_paths {
                    let (_, repo_name, success, status) = process_repo(path, action).await;

                    // Update the result in all_results
                    if let Some(existing) = all_results.iter_mut().find(|(name, _, _)| name == &repo_name) {
                        *existing = (repo_name, success, status);
                    }
                }
            }

            // Phase 4: Yield results
            let mut repos_success = 0;
            let mut repos_failed = 0;

            for (repo_name, success, status) in all_results {
                if success {
                    repos_success += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("✓ {} [{}]", repo_name, status),
                    };
                } else {
                    repos_failed += 1;
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} [{}]", repo_name, status),
                    };
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Workspace {}: {} success, {} failed, {} skipped (worktrees)",
                    action, repos_success, repos_failed, repos_skipped
                ),
            };
        }
    }

    /// Verify and sync git config for all repos in a workspace
    #[plexus_macros::hub_method(
        description = "Verify and sync git SSH config for all repos in a workspace. Ensures core.sshCommand and hyperforge.org are set.",
        params(
            workspace_path = "Path to workspace directory",
            org = "Organization name",
            fix = "Fix missing/incorrect config (optional, default: false)"
        )
    )]
    pub async fn workspace_verify_sync(
        &self,
        workspace_path: String,
        org: String,
        fix: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let should_fix = fix.unwrap_or(false);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let ssh_script = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".hypermemetic-infra")
                .join("scripts")
                .join("hyperforge-ssh");

            let ssh_script_str = ssh_script.to_str().unwrap_or("");

            if !ssh_script.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("SSH script not found: {}", ssh_script.display()),
                };
                return;
            }

            let mut repos_ok = 0;
            let mut repos_fixed = 0;
            let mut repos_need_fix = 0;
            let mut repos_failed = 0;
            let mut repos_skipped_worktree = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                // Skip if no .git at all
                if !git_path.exists() {
                    continue;
                }

                // Skip worktrees (have .git file, not directory)
                if Self::is_worktree(&path) {
                    repos_skipped_worktree += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("⏭ {} (worktree)", path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown")),
                    };
                    continue;
                }

                let repo_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let path_str = path.to_str().unwrap_or(".");

                // Check current git config
                let current_ssh = tokio::process::Command::new("git")
                    .args(["-C", path_str, "config", "--get", "core.sshCommand"])
                    .output()
                    .await
                    .ok()
                    .and_then(|o| if o.status.success() {
                        String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
                    } else {
                        None
                    });

                let current_org = tokio::process::Command::new("git")
                    .args(["-C", path_str, "config", "--get", "hyperforge.org"])
                    .output()
                    .await
                    .ok()
                    .and_then(|o| if o.status.success() {
                        String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
                    } else {
                        None
                    });

                let ssh_ok = current_ssh.as_deref() == Some(ssh_script_str);
                let org_ok = current_org.as_deref() == Some(&org);

                if ssh_ok && org_ok {
                    repos_ok += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("✓ {}", repo_name),
                    };
                    continue;
                }

                if !should_fix {
                    repos_need_fix += 1;
                    let mut issues = Vec::new();
                    if !ssh_ok {
                        issues.push("sshCommand");
                    }
                    if !org_ok {
                        issues.push("hyperforge.org");
                    }
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} (missing: {})", repo_name, issues.join(", ")),
                    };
                    continue;
                }

                // Fix the config
                let mut fixed = true;

                if !ssh_ok {
                    let result = tokio::process::Command::new("git")
                        .args(["-C", path_str, "config", "core.sshCommand", ssh_script_str])
                        .output()
                        .await;
                    if result.map(|o| o.status.success()).unwrap_or(false) == false {
                        fixed = false;
                    }
                }

                if !org_ok {
                    let result = tokio::process::Command::new("git")
                        .args(["-C", path_str, "config", "hyperforge.org", &org])
                        .output()
                        .await;
                    if result.map(|o| o.status.success()).unwrap_or(false) == false {
                        fixed = false;
                    }
                }

                if fixed {
                    repos_fixed += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("⚡ {} (fixed)", repo_name),
                    };
                } else {
                    repos_failed += 1;
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} (fix failed)", repo_name),
                    };
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Verify complete: {} ok, {} fixed, {} need fix, {} failed, {} worktrees skipped",
                    repos_ok, repos_fixed, repos_need_fix, repos_failed, repos_skipped_worktree
                ),
            };

            if repos_need_fix > 0 && !should_fix {
                yield HyperforgeEvent::Info {
                    message: "Run with --fix true to automatically fix issues".to_string(),
                };
            }
        }
    }

    /// Test push connectivity for all repos in a workspace
    #[plexus_macros::hub_method(
        description = "Test SSH connectivity to all configured forges for repos in a workspace",
        params(
            workspace_path = "Path to workspace directory"
        )
    )]
    pub async fn workspace_test_push(
        &self,
        workspace_path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mut repos_ok = 0;
            let mut repos_failed = 0;
            let mut repos_skipped_worktree = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                if !git_path.exists() {
                    continue;
                }

                // Skip worktrees
                if Self::is_worktree(&path) {
                    repos_skipped_worktree += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("⏭ {} (worktree)", path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown")),
                    };
                    continue;
                }

                let repo_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let path_str = path.to_str().unwrap_or(".");

                // Get list of remotes
                let remotes_output = tokio::process::Command::new("git")
                    .args(["-C", path_str, "remote"])
                    .output()
                    .await;

                let remotes: Vec<String> = remotes_output
                    .ok()
                    .and_then(|o| if o.status.success() {
                        String::from_utf8(o.stdout).ok()
                    } else {
                        None
                    })
                    .map(|s| s.lines().map(|l| l.to_string()).collect())
                    .unwrap_or_default();

                if remotes.is_empty() {
                    yield HyperforgeEvent::Info {
                        message: format!("⏭ {} (no remotes)", repo_name),
                    };
                    continue;
                }

                let mut repo_ok = true;
                let mut results = Vec::new();

                for remote in &remotes {
                    // Test with git ls-remote (lightweight check)
                    let result = tokio::process::Command::new("git")
                        .args(["-C", path_str, "ls-remote", "--exit-code", remote, "HEAD"])
                        .output()
                        .await;

                    let success = result.map(|o| o.status.success()).unwrap_or(false);
                    if success {
                        results.push(format!("{}:✓", remote));
                    } else {
                        results.push(format!("{}:✗", remote));
                        repo_ok = false;
                    }
                }

                if repo_ok {
                    repos_ok += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("✓ {} [{}]", repo_name, results.join(" ")),
                    };
                } else {
                    repos_failed += 1;
                    yield HyperforgeEvent::Error {
                        message: format!("✗ {} [{}]", repo_name, results.join(" ")),
                    };
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Connectivity test: {} ok, {} failed, {} worktrees skipped", repos_ok, repos_failed, repos_skipped_worktree),
            };
        }
    }

    /// Initialize .hyperforge/config.toml for all repos in a workspace
    #[plexus_macros::hub_method(
        description = "Initialize .hyperforge/config.toml for all git repos in a workspace directory",
        params(
            workspace_path = "Path to workspace directory",
            org = "Organization name",
            forges = "Comma-separated forges (github,codeberg,gitlab)",
            ssh_key = "SSH key filename for all forges (optional)",
            default_branch = "Default branch name (optional, e.g., 'main')"
        )
    )]
    pub async fn workspace_init_configs(
        &self,
        workspace_path: String,
        org: String,
        forges: String,
        ssh_key: Option<String>,
        default_branch: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            // Parse forges
            let forge_list: Vec<String> = forges.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();

            if forge_list.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "At least one forge required".to_string(),
                };
                return;
            }

            // Check SSH key if provided
            if let Some(ref key) = ssh_key {
                let ssh_dir = dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".ssh");
                let key_path = ssh_dir.join(key);

                if !key_path.exists() {
                    yield HyperforgeEvent::Error {
                        message: format!("SSH key not found: {}", key_path.display()),
                    };
                    return;
                }
            }

            let mut repos_initialized = 0;
            let mut repos_skipped = 0;
            let mut repos_failed = 0;
            let mut repos_skipped_worktree = 0;

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                if !git_path.exists() {
                    continue;
                }

                // Skip worktrees
                if Self::is_worktree(&path) {
                    repos_skipped_worktree += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("⏭ {} (worktree)", path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown")),
                    };
                    continue;
                }

                let repo_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Skip if config already exists
                if HyperforgeConfig::exists(&path) {
                    repos_skipped += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("⏭ Skipped (config exists): {}", repo_name),
                    };
                    continue;
                }

                // Create config
                let mut config = HyperforgeConfig::new(forge_list.clone())
                    .with_org(&org)
                    .with_repo_name(&repo_name);

                // Add SSH key for all forges if provided
                if let Some(ref key) = ssh_key {
                    for forge in &forge_list {
                        config.ssh.insert(forge.clone(), key.clone());
                    }
                }

                // Set default branch if provided
                if let Some(ref branch) = default_branch {
                    config.default_branch = Some(branch.clone());
                }

                // Save config
                match config.save(&path) {
                    Ok(_) => {
                        repos_initialized += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("✓ Initialized: {}", repo_name),
                        };
                    }
                    Err(e) => {
                        repos_failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("✗ Failed {}: {}", repo_name, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Workspace init complete: {} initialized, {} skipped (exist), {} worktrees skipped, {} failed",
                    repos_initialized, repos_skipped, repos_skipped_worktree, repos_failed
                ),
            };
        }
    }

    /// Scan workspace for repos with .hyperforge/config.toml and register them in LocalForge
    #[plexus_macros::hub_method(
        description = "Scan workspace for repos with .hyperforge/config.toml and register them in LocalForge",
        params(
            workspace_path = "Path to workspace directory containing git repos",
            dry_run = "Preview what would be registered without modifying LocalForge (optional, default: false)"
        )
    )]
    pub async fn workspace_register(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();

        stream! {
            let workspace = std::path::Path::new(&workspace_path);
            let is_dry_run = dry_run.unwrap_or(false);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Scanning workspace for repos to register...", dry_prefix),
            };

            let mut registered: usize = 0;
            let mut skipped_exists: usize = 0;
            let mut skipped_no_config: usize = 0;
            let mut skipped_worktree: usize = 0;
            let mut failed: usize = 0;
            let mut modified_orgs: std::collections::HashSet<String> = std::collections::HashSet::new();

            let mut entries = match tokio::fs::read_dir(workspace).await {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace directory: {}", e),
                    };
                    return;
                }
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let git_path = path.join(".git");

                // Skip if no .git (not a git repo)
                if !git_path.exists() {
                    continue;
                }

                let dir_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Skip worktrees
                if Self::is_worktree(&path) {
                    skipped_worktree += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("Skipped (worktree): {}", dir_name),
                    };
                    continue;
                }

                // Skip if no .hyperforge/config.toml
                if !HyperforgeConfig::exists(&path) {
                    skipped_no_config += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("Skipped (no config): {}", dir_name),
                    };
                    continue;
                }

                // Load config
                let config = match HyperforgeConfig::load(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to read config: {}: {}", dir_name, e),
                        };
                        continue;
                    }
                };

                // Validate config
                if let Err(e) = config.validate() {
                    failed += 1;
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid config: {}: {}", dir_name, e),
                    };
                    continue;
                }

                // Extract repo metadata
                let repo_name = config.get_repo_name(&path);

                let org = match config.org {
                    Some(ref o) => o.clone(),
                    None => {
                        failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("No org specified in config for: {}", dir_name),
                        };
                        continue;
                    }
                };

                // Parse origin (first forge)
                let origin = match config.forges.first().and_then(|f| HyperforgeConfig::parse_forge(f)) {
                    Some(f) => f,
                    None => {
                        failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("No valid origin forge in config for: {}", dir_name),
                        };
                        continue;
                    }
                };

                // Parse mirrors (remaining forges)
                let mirrors: Vec<crate::types::Forge> = config.forges[1..]
                    .iter()
                    .filter_map(|f| HyperforgeConfig::parse_forge(f))
                    .collect();

                // Check if already in LocalForge
                let local = hub.get_local_forge(&org).await;
                match local.repo_exists(&org, &repo_name).await {
                    Ok(true) => {
                        skipped_exists += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("Skipped (already registered): {}", dir_name),
                        };
                        continue;
                    }
                    Ok(false) => { /* proceed */ }
                    Err(e) => {
                        failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to check repo existence for {}: {}", dir_name, e),
                        };
                        continue;
                    }
                }

                // Build Repo struct
                let mut repo = Repo::new(repo_name.clone(), origin.clone())
                    .with_visibility(config.visibility.clone())
                    .with_mirrors(mirrors.clone());
                if let Some(ref desc) = config.description {
                    repo = repo.with_description(desc.clone());
                }

                // Register unless dry run
                if !is_dry_run {
                    match local.create_repo(&org, &repo).await {
                        Ok(_) => {
                            modified_orgs.insert(org.clone());
                        }
                        Err(e) => {
                            failed += 1;
                            yield HyperforgeEvent::Error {
                                message: format!("Failed to register {}: {}", dir_name, e),
                            };
                            continue;
                        }
                    }
                }

                registered += 1;

                let mirrors_str: Vec<String> = mirrors.iter()
                    .map(|f| format!("{:?}", f).to_lowercase())
                    .collect();
                let origin_str = format!("{:?}", origin).to_lowercase();

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!(
                            "[DRY RUN] Would register: {} -> {} (origin: {}, mirrors: [{}])",
                            repo_name, org, origin_str, mirrors_str.join(", ")
                        ),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!(
                            "Registered: {} -> {} (origin: {}, mirrors: [{}])",
                            repo_name, org, origin_str, mirrors_str.join(", ")
                        ),
                    };
                }
            }

            // Save once per modified org
            if !is_dry_run {
                for org in &modified_orgs {
                    let local = hub.get_local_forge(org).await;
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml for org {}: {}", org, e),
                        };
                    }
                }
            }

            // Yield summary
            let register_word = if is_dry_run { "would register" } else { "registered" };
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Workspace register complete: {} {}, {} already registered, {} without config, {} worktrees skipped, {} failed",
                    dry_prefix, registered, register_word, skipped_exists, skipped_no_config, skipped_worktree, failed
                ),
            };
        }
    }

    // ============================================================
    // Package Management Commands
    // ============================================================

    /// Get the topological publish order for packages in a workspace
    #[plexus_macros::hub_method(
        description = "Analyze workspace packages and return publish order (dependencies first)",
        params(
            workspace_path = "Path to workspace directory"
        )
    )]
    pub async fn workspace_publish_order(
        &self,
        workspace_path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let registry = crate::packages::PackageRegistry::new();

            // Scan workspace
            yield HyperforgeEvent::Info {
                message: "Scanning workspace for packages...".to_string(),
            };

            let packages = match registry.scan_workspace(workspace) {
                Ok(p) => p,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to scan workspace: {}", e),
                    };
                    return;
                }
            };

            if packages.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace".to_string(),
                };
                return;
            }

            // Report found packages
            yield HyperforgeEvent::Info {
                message: format!("Found {} packages:", packages.len()),
            };

            for pkg in &packages {
                let deps_str = if pkg.workspace_deps.is_empty() {
                    "(no workspace deps)".to_string()
                } else {
                    format!("depends on: {}", pkg.workspace_deps.join(", "))
                };
                yield HyperforgeEvent::Info {
                    message: format!("  {} v{} [{}] {}", pkg.name, pkg.version, pkg.manager, deps_str),
                };
            }

            // Get publish order
            match registry.publish_order(&packages) {
                Ok(order) => {
                    yield HyperforgeEvent::Info {
                        message: "\nPublish order (dependencies first):".to_string(),
                    };
                    for (i, name) in order.iter().enumerate() {
                        yield HyperforgeEvent::Info {
                            message: format!("  {}. {}", i + 1, name),
                        };
                    }
                }
                Err(crate::packages::PackageError::CycleDetected { packages }) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Cycle detected in dependencies: {:?}", packages),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to compute publish order: {}", e),
                    };
                }
            }
        }
    }

    /// Update a package version across the workspace
    #[plexus_macros::hub_method(
        description = "Update a package version in its manifest and all dependents",
        params(
            workspace_path = "Path to workspace directory",
            package_name = "Name of the package to update",
            new_version = "New version string"
        )
    )]
    pub async fn workspace_update_package(
        &self,
        workspace_path: String,
        package_name: String,
        new_version: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let registry = crate::packages::PackageRegistry::new();

            yield HyperforgeEvent::Info {
                message: format!("Updating {} to version {}...", package_name, new_version),
            };

            match registry.update_package_version(workspace, &package_name, &new_version) {
                Ok(updated_paths) => {
                    yield HyperforgeEvent::Info {
                        message: format!("Updated {} files:", updated_paths.len()),
                    };
                    for path in updated_paths {
                        let rel_path = path.strip_prefix(workspace).unwrap_or(&path);
                        yield HyperforgeEvent::Info {
                            message: format!("  ✓ {}", rel_path.display()),
                        };
                    }
                }
                Err(crate::packages::PackageError::PackageNotFound { name }) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Package not found: {}", name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to update package: {}", e),
                    };
                }
            }
        }
    }

    /// List all packages in a workspace
    #[plexus_macros::hub_method(
        description = "List all packages in a workspace with their versions and dependencies",
        params(
            workspace_path = "Path to workspace directory"
        )
    )]
    pub async fn workspace_packages(
        &self,
        workspace_path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let registry = crate::packages::PackageRegistry::new();

            let packages = match registry.scan_workspace(workspace) {
                Ok(p) => p,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to scan workspace: {}", e),
                    };
                    return;
                }
            };

            if packages.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace".to_string(),
                };
                return;
            }

            // Group by manager
            let mut cargo_pkgs: Vec<_> = packages.iter().filter(|p| p.manager == crate::packages::PackageManager::Cargo).collect();
            let mut cabal_pkgs: Vec<_> = packages.iter().filter(|p| p.manager == crate::packages::PackageManager::Cabal).collect();

            cargo_pkgs.sort_by(|a, b| a.name.cmp(&b.name));
            cabal_pkgs.sort_by(|a, b| a.name.cmp(&b.name));

            if !cargo_pkgs.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("\nCargo packages ({}):", cargo_pkgs.len()),
                };
                for pkg in cargo_pkgs {
                    yield HyperforgeEvent::Info {
                        message: format!("  {} v{}", pkg.name, pkg.version),
                    };
                    if !pkg.workspace_deps.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: format!("    └─ depends: {}", pkg.workspace_deps.join(", ")),
                        };
                    }
                }
            }

            if !cabal_pkgs.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("\nCabal packages ({}):", cabal_pkgs.len()),
                };
                for pkg in cabal_pkgs {
                    yield HyperforgeEvent::Info {
                        message: format!("  {} v{}", pkg.name, pkg.version),
                    };
                    if !pkg.workspace_deps.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: format!("    └─ depends: {}", pkg.workspace_deps.join(", ")),
                        };
                    }
                }
            }
        }
    }

    /// Detect package renames (directory name differs from package name)
    #[plexus_macros::hub_method(
        description = "Detect packages where directory name differs from manifest package name (likely renames)",
        params(
            workspace_path = "Path to workspace directory"
        )
    )]
    pub async fn workspace_detect_renames(
        &self,
        workspace_path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let registry = crate::packages::PackageRegistry::new();

            match registry.detect_renames(workspace) {
                Ok(renames) => {
                    if renames.is_empty() {
                        yield HyperforgeEvent::Info {
                            message: "No package renames detected".to_string(),
                        };
                    } else {
                        yield HyperforgeEvent::Info {
                            message: format!("Found {} renamed packages:", renames.len()),
                        };
                        for rename in &renames {
                            yield HyperforgeEvent::Info {
                                message: format!(
                                    "  {} → {} (v{}) [{}]",
                                    rename.old_name, rename.new_name, rename.version, rename.manager
                                ),
                            };
                        }
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to detect renames: {}", e),
                    };
                }
            }
        }
    }

    /// Yank old package names from a Cargo registry
    #[plexus_macros::hub_method(
        description = "Yank old package names (directory names) from a Cargo registry after rename",
        params(
            workspace_path = "Path to workspace directory",
            registry = "Registry name (optional, default: crates-io)",
            dry_run = "Preview without yanking (optional, default: true)"
        )
    )]
    pub async fn workspace_yank_old_packages(
        &self,
        workspace_path: String,
        registry: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let registry_name = registry.unwrap_or_else(|| "crates-io".to_string());
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let pkg_registry = crate::packages::PackageRegistry::new();

            let renames = match pkg_registry.detect_renames(workspace) {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to detect renames: {}", e),
                    };
                    return;
                }
            };

            // Filter to Cargo packages only
            let cargo_renames: Vec<_> = renames
                .into_iter()
                .filter(|r| r.manager == crate::packages::PackageManager::Cargo)
                .collect();

            if cargo_renames.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No Cargo package renames to yank".to_string(),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Yanking {} old package names from '{}':", mode, cargo_renames.len(), registry_name),
            };

            for rename in &cargo_renames {
                yield HyperforgeEvent::Info {
                    message: format!("\n{}Yanking '{}' (renamed to '{}'):", mode, rename.old_name, rename.new_name),
                };

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!("  Would run: cargo yank --registry {} {}", registry_name, rename.old_name),
                    };
                } else {
                    let result = tokio::process::Command::new("cargo")
                        .args(["yank", "--registry", &registry_name, &rename.old_name])
                        .output()
                        .await;

                    match result {
                        Ok(output) if output.status.success() => {
                            yield HyperforgeEvent::Info {
                                message: format!("  ✓ Yanked '{}'", rename.old_name),
                            };
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to yank '{}': {} {}", rename.old_name, stderr, stdout),
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to run cargo yank: {}", e),
                            };
                        }
                    }
                }
            }

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "\nRun with --dry_run false to actually yank packages".to_string(),
                };
            }
        }
    }

    // workspace_rename_forge_repos: removed — subsumed by repos_rename

    // workspace_rename: removed — subsumed by repos_rename(path=Some(...), workspace_path=Some(...))

    /// Rename all packages where directory name differs from package name
    /// Thin orchestrator that delegates to repos_rename for each detected rename
    #[plexus_macros::hub_method(
        description = "Rename all packages where directory name differs from manifest package name (delegates to repos_rename)",
        params(
            workspace_path = "Path to workspace directory",
            org = "Organization name",
            dry_run = "Preview without applying (optional, default: true)"
        )
    )]
    pub async fn workspace_rename_all(
        &self,
        workspace_path: String,
        org: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let hub = self.clone();
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            // Detect all renames
            let registry = packages::PackageRegistry::new();
            let renames = match registry.detect_renames(workspace) {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to detect renames: {}", e),
                    };
                    return;
                }
            };

            if renames.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages need renaming - all directory names match package names".to_string(),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Found {} packages to rename:", mode, renames.len()),
            };

            for rename in &renames {
                yield HyperforgeEvent::Info {
                    message: format!("  {} -> {} ({})", rename.old_name, rename.new_name, rename.manager),
                };
            }

            let mut total_success = 0u32;
            let mut total_failed = 0u32;

            for rename in renames {
                yield HyperforgeEvent::Info {
                    message: format!("\n{}=== Renaming '{}' -> '{}' ===", mode, rename.old_name, rename.new_name),
                };

                let repo_dir = workspace.join(&rename.old_name);
                let repo_path = repo_dir.to_str().map(|s| s.to_string());

                let sub_stream = hub.repos_rename(
                    org.clone(),
                    rename.old_name.clone(),
                    rename.new_name.clone(),
                    repo_path,
                    Some(workspace_path.clone()),
                    Some(is_dry_run),
                    None,
                ).await;

                tokio::pin!(sub_stream);

                let mut had_error = false;
                while let Some(event) = sub_stream.next().await {
                    if matches!(&event, HyperforgeEvent::Error { .. }) {
                        had_error = true;
                    }
                    yield event;
                }

                if had_error {
                    total_failed += 1;
                } else {
                    total_success += 1;
                }
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: format!("\n{}=== Summary ===", mode),
            };

            yield HyperforgeEvent::Info {
                message: format!("Successful: {}, Failed: {}", total_success, total_failed),
            };

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "Run with --dry_run false to apply renames".to_string(),
                };
            }
        }
    }

    // ============================================================
    // Package Publishing Commands
    // ============================================================

    /// Publish a single package to its registry (crates.io or Hackage)
    #[plexus_macros::hub_method(
        description = "Publish a package to its registry (crates.io for Cargo, Hackage for Cabal)",
        params(
            package_path = "Path to the package directory",
            dry_run = "Preview without publishing (optional, default: true)"
        )
    )]
    pub async fn package_publish(
        &self,
        package_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let path = std::path::Path::new(&package_path);

            if !path.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Package path does not exist: {}", package_path),
                };
                return;
            }

            let publisher = crate::package::AutoPublisher::new();

            // Detect package type
            match publisher.detect(path).await {
                Ok(true) => {}
                Ok(false) => {
                    yield HyperforgeEvent::Error {
                        message: format!("No supported package manifest found at: {}", package_path),
                    };
                    return;
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to detect package type: {}", e),
                    };
                    return;
                }
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Publishing package at {}...", mode, package_path),
            };

            match publisher.publish(path, is_dry_run).await {
                Ok(_) => {
                    if is_dry_run {
                        yield HyperforgeEvent::Info {
                            message: "✓ Dry run successful - package is ready to publish".to_string(),
                        };
                        yield HyperforgeEvent::Info {
                            message: "\nRun with --dry_run false to actually publish".to_string(),
                        };
                    } else {
                        yield HyperforgeEvent::Info {
                            message: "✓ Package published successfully!".to_string(),
                        };
                    }
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("✗ Failed to publish: {}", e),
                    };
                }
            }
        }
    }

    /// Bump a package version (patch, minor, or major)
    #[plexus_macros::hub_method(
        description = "Bump a package version according to semver (patch, minor, major)",
        params(
            package_path = "Path to the package directory",
            bump = "Version bump type: patch, minor, or major"
        )
    )]
    pub async fn package_bump_version(
        &self,
        package_path: String,
        bump: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let path = std::path::Path::new(&package_path);

            if !path.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Package path does not exist: {}", package_path),
                };
                return;
            }

            let bump_type = match bump.to_lowercase().as_str() {
                "patch" => crate::types::VersionBump::Patch,
                "minor" => crate::types::VersionBump::Minor,
                "major" => crate::types::VersionBump::Major,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid bump type '{}'. Use: patch, minor, or major", bump),
                    };
                    return;
                }
            };

            let publisher = crate::package::AutoPublisher::new();

            match publisher.bump_version(path, bump_type).await {
                Ok(new_version) => {
                    yield HyperforgeEvent::Info {
                        message: format!("✓ Version bumped to {}", new_version),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("✗ Failed to bump version: {}", e),
                    };
                }
            }
        }
    }

    /// Publish all packages in a workspace in dependency order
    #[plexus_macros::hub_method(
        description = "Publish all packages in a workspace in topological order (dependencies first)",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without publishing (optional, default: true)"
        )
    )]
    pub async fn workspace_publish_all(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let registry = crate::packages::PackageRegistry::new();
            let publisher = crate::package::AutoPublisher::new();

            // Scan workspace
            yield HyperforgeEvent::Info {
                message: "Scanning workspace for packages...".to_string(),
            };

            let packages = match registry.scan_workspace(workspace) {
                Ok(p) => p,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to scan workspace: {}", e),
                    };
                    return;
                }
            };

            if packages.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace".to_string(),
                };
                return;
            }

            // Get publish order
            let order = match registry.publish_order(&packages) {
                Ok(o) => o,
                Err(crate::packages::PackageError::CycleDetected { packages }) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Cycle detected in dependencies: {:?}", packages),
                    };
                    return;
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to compute publish order: {}", e),
                    };
                    return;
                }
            };

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Publishing {} packages in order:", mode, order.len()),
            };

            for (i, name) in order.iter().enumerate() {
                yield HyperforgeEvent::Info {
                    message: format!("  {}. {}", i + 1, name),
                };
            }

            let mut success_count = 0;
            let mut failed_count = 0;

            for (i, name) in order.iter().enumerate() {
                // Find package path
                let pkg = match packages.iter().find(|p| &p.name == name) {
                    Some(p) => p,
                    None => {
                        yield HyperforgeEvent::Error {
                            message: format!("Package not found: {}", name),
                        };
                        failed_count += 1;
                        continue;
                    }
                };

                yield HyperforgeEvent::Info {
                    message: format!("\n{}[{}/{}] Publishing {}...", mode, i + 1, order.len(), name),
                };

                match publisher.publish(&pkg.path, is_dry_run).await {
                    Ok(_) => {
                        yield HyperforgeEvent::Info {
                            message: format!("  ✓ {}", name),
                        };
                        success_count += 1;
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  ✗ {}: {}", name, e),
                        };
                        failed_count += 1;

                        // Stop on failure in real mode (dependencies may be broken)
                        if !is_dry_run {
                            yield HyperforgeEvent::Error {
                                message: "Stopping due to publish failure".to_string(),
                            };
                            break;
                        }
                    }
                }
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: format!("\n{}═══ Summary ═══", mode),
            };

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("Validated: {}, Failed: {}", success_count, failed_count),
                };
                if failed_count == 0 {
                    yield HyperforgeEvent::Info {
                        message: "\nAll packages ready! Run with --dry_run false to publish".to_string(),
                    };
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Published: {}, Failed: {}", success_count, failed_count),
                };
            }
        }
    }

    /// Bump version for all packages in a workspace
    #[plexus_macros::hub_method(
        description = "Bump version for all packages in a workspace (patch, minor, or major)",
        params(
            workspace_path = "Path to workspace directory",
            bump = "Version bump type: patch, minor, or major (optional, default: patch)"
        )
    )]
    pub async fn workspace_bump_all(
        &self,
        workspace_path: String,
        bump: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let bump_str = bump.unwrap_or_else(|| "patch".to_string());
            let bump_type = match bump_str.to_lowercase().as_str() {
                "patch" => crate::types::VersionBump::Patch,
                "minor" => crate::types::VersionBump::Minor,
                "major" => crate::types::VersionBump::Major,
                _ => {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid bump type '{}'. Use: patch, minor, or major", bump_str),
                    };
                    return;
                }
            };

            let registry = crate::packages::PackageRegistry::new();
            let publisher = crate::package::AutoPublisher::new();

            // Scan workspace
            yield HyperforgeEvent::Info {
                message: format!("Scanning workspace for packages (bump: {})...", bump_str),
            };

            let packages = match registry.scan_workspace(workspace) {
                Ok(p) => p,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to scan workspace: {}", e),
                    };
                    return;
                }
            };

            if packages.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No packages found in workspace".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("Found {} packages to bump:", packages.len()),
            };

            let mut success_count = 0;
            let mut failed_count = 0;
            let mut results: Vec<(String, String, String)> = Vec::new(); // (name, old_ver, new_ver)

            for pkg in &packages {
                let old_version = pkg.version.clone();

                match publisher.bump_version(&pkg.path, bump_type.clone()).await {
                    Ok(new_version) => {
                        results.push((pkg.name.clone(), old_version, new_version));
                        success_count += 1;
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  ✗ {}: {}", pkg.name, e),
                        };
                        failed_count += 1;
                    }
                }
            }

            // Show results
            for (name, old_ver, new_ver) in &results {
                yield HyperforgeEvent::Info {
                    message: format!("  ✓ {} {} → {}", name, old_ver, new_ver),
                };
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: format!("\nBumped: {}, Failed: {}", success_count, failed_count),
            };
        }
    }

    /// Find and fix stale path dependencies in Cargo.toml files
    #[plexus_macros::hub_method(
        description = "Find path dependencies pointing to non-existent directories and fix them by finding the correct package location",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without applying fixes (optional, default: true)"
        )
    )]
    pub async fn workspace_fix_path_deps(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Scanning workspace for stale path dependencies...", mode),
            };

            // First, build a map of package_name -> directory_name for all packages
            let mut package_dirs: std::collections::HashMap<String, String> = std::collections::HashMap::new();

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace: {}", e),
                    };
                    return;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Skip hidden dirs and common non-package dirs
                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" {
                    continue;
                }

                // Check for Cargo.toml and extract package name
                let cargo_toml = path.join("Cargo.toml");
                if cargo_toml.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let Some(name) = doc.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
                                package_dirs.insert(name.to_string(), dir_name.clone());
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Found {} packages in workspace", package_dirs.len()),
            };

            // Now scan all Cargo.toml files for path dependencies
            let mut fixes: Vec<(std::path::PathBuf, String, String, String)> = Vec::new(); // (file, dep_name, old_path, new_path)
            let mut broken: Vec<(std::path::PathBuf, String, String)> = Vec::new(); // (file, dep_name, old_path) - can't fix

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(_) => return,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" {
                    continue;
                }

                let cargo_toml = path.join("Cargo.toml");
                if !cargo_toml.exists() {
                    continue;
                }

                let content = match std::fs::read_to_string(&cargo_toml) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let doc = match content.parse::<toml_edit::DocumentMut>() {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // Check all dependency sections
                for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(deps) = doc.get(*section).and_then(|d| d.as_table()) {
                        for (dep_name, dep_value) in deps.iter() {
                            // Check if it has a path dependency
                            let path_str = if let Some(table) = dep_value.as_inline_table() {
                                table.get("path").and_then(|p| p.as_str())
                            } else if let Some(table) = dep_value.as_table() {
                                table.get("path").and_then(|p| p.as_str())
                            } else {
                                None
                            };

                            if let Some(dep_path) = path_str {
                                // Resolve the path relative to the Cargo.toml
                                let resolved = path.join(dep_path);

                                // Check if path exists
                                if !resolved.exists() {
                                    // Try to find the package by name
                                    if let Some(correct_dir) = package_dirs.get(dep_name) {
                                        let new_path = format!("../{}", correct_dir);
                                        fixes.push((cargo_toml.clone(), dep_name.to_string(), dep_path.to_string(), new_path));
                                    } else {
                                        broken.push((cargo_toml.clone(), dep_name.to_string(), dep_path.to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Report findings
            if fixes.is_empty() && broken.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "✓ No stale path dependencies found".to_string(),
                };
                return;
            }

            if !fixes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("\nFound {} fixable path dependencies:", fixes.len()),
                };

                for (file, dep_name, old_path, new_path) in &fixes {
                    let rel_file = file.strip_prefix(workspace).unwrap_or(file);
                    yield HyperforgeEvent::Info {
                        message: format!("  {} in {}:", dep_name, rel_file.display()),
                    };
                    yield HyperforgeEvent::Info {
                        message: format!("    {} → {}", old_path, new_path),
                    };
                }
            }

            if !broken.is_empty() {
                yield HyperforgeEvent::Error {
                    message: format!("\nFound {} unfixable path dependencies (package not in workspace):", broken.len()),
                };

                for (file, dep_name, old_path) in &broken {
                    let rel_file = file.strip_prefix(workspace).unwrap_or(file);
                    yield HyperforgeEvent::Error {
                        message: format!("  {} in {}: {}", dep_name, rel_file.display(), old_path),
                    };
                }
            }

            // Apply fixes if not dry run
            if !is_dry_run && !fixes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "\nApplying fixes...".to_string(),
                };

                let mut success_count = 0;
                let mut fail_count = 0;

                // Group fixes by file
                let mut fixes_by_file: std::collections::HashMap<std::path::PathBuf, Vec<(String, String, String)>> = std::collections::HashMap::new();
                for (file, dep_name, old_path, new_path) in fixes {
                    fixes_by_file.entry(file).or_default().push((dep_name, old_path, new_path));
                }

                for (file, file_fixes) in fixes_by_file {
                    let content = match std::fs::read_to_string(&file) {
                        Ok(c) => c,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to read {}: {}", file.display(), e),
                            };
                            fail_count += 1;
                            continue;
                        }
                    };

                    let mut new_content = content.clone();
                    for (_dep_name, old_path, new_path) in &file_fixes {
                        // Simple string replacement - works for most cases
                        new_content = new_content.replace(
                            &format!("path = \"{}\"", old_path),
                            &format!("path = \"{}\"", new_path)
                        );
                    }

                    if new_content != content {
                        match std::fs::write(&file, &new_content) {
                            Ok(_) => {
                                let rel_file = file.strip_prefix(workspace).unwrap_or(&file);
                                yield HyperforgeEvent::Info {
                                    message: format!("  ✓ Fixed {}", rel_file.display()),
                                };
                                success_count += 1;
                            }
                            Err(e) => {
                                yield HyperforgeEvent::Error {
                                    message: format!("  ✗ Failed to write {}: {}", file.display(), e),
                                };
                                fail_count += 1;
                            }
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!("\nFixed: {}, Failed: {}", success_count, fail_count),
                };
            } else if is_dry_run && !fixes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "\nRun with --dry_run false to apply fixes".to_string(),
                };
            }
        }
    }

    /// Fix path dependency versions for crates.io publishing
    /// - Adds versions to runtime deps (dependencies, build-dependencies)
    /// - Removes versions from dev deps (dev-dependencies)
    #[plexus_macros::hub_method(
        description = "Fix path dependency versions: add to runtime deps, remove from dev deps",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without applying changes (optional, default: true)"
        )
    )]
    pub async fn workspace_fix_dep_versions(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Fixing path dependency versions for publishing...", mode),
            };

            // Build map of package_name -> version (strip prerelease)
            let mut package_versions: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let mut package_names: std::collections::HashSet<String> = std::collections::HashSet::new();

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace: {}", e),
                    };
                    return;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" { continue; }

                let cargo_toml = path.join("Cargo.toml");
                if cargo_toml.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let (Some(name), Some(version)) = (
                                doc.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()),
                                doc.get("package").and_then(|p| p.get("version")).and_then(|v| v.as_str())
                            ) {
                                let base_version = version.split('-').next().unwrap_or(version);
                                package_versions.insert(name.to_string(), base_version.to_string());
                                package_names.insert(name.to_string());
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Found {} workspace packages", package_versions.len()),
            };

            // Track changes: (file, add_versions: [(dep, ver)], remove_versions: [dep])
            struct FileChanges {
                path: std::path::PathBuf,
                add_versions: Vec<(String, String)>,    // runtime deps needing version
                remove_versions: Vec<String>,            // dev deps with version to remove
            }
            let mut all_changes: Vec<FileChanges> = Vec::new();

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(_) => return,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" { continue; }

                let cargo_toml = path.join("Cargo.toml");
                if !cargo_toml.exists() { continue; }

                let content = match std::fs::read_to_string(&cargo_toml) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let doc = match content.parse::<toml_edit::DocumentMut>() {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let mut changes = FileChanges {
                    path: cargo_toml.clone(),
                    add_versions: Vec::new(),
                    remove_versions: Vec::new(),
                };

                // Helper to check path dep
                let check_dep = |dep_value: &toml_edit::Item| -> (bool, bool) {
                    if let Some(table) = dep_value.as_inline_table() {
                        (table.get("path").is_some(), table.get("version").is_some())
                    } else if let Some(table) = dep_value.as_table() {
                        (table.get("path").is_some(), table.get("version").is_some())
                    } else {
                        (false, false)
                    }
                };

                // Check runtime deps - need version
                for section in &["dependencies", "build-dependencies"] {
                    if let Some(deps) = doc.get(*section).and_then(|d| d.as_table()) {
                        for (dep_name, dep_value) in deps.iter() {
                            if !package_names.contains(dep_name) { continue; }
                            let (has_path, has_version) = check_dep(dep_value);
                            if has_path && !has_version {
                                if let Some(ver) = package_versions.get(dep_name) {
                                    changes.add_versions.push((dep_name.to_string(), ver.clone()));
                                }
                            }
                        }
                    }
                }

                // Check dev deps - should NOT have version
                if let Some(deps) = doc.get("dev-dependencies").and_then(|d| d.as_table()) {
                    for (dep_name, dep_value) in deps.iter() {
                        if !package_names.contains(dep_name) { continue; }
                        let (has_path, has_version) = check_dep(dep_value);
                        if has_path && has_version {
                            changes.remove_versions.push(dep_name.to_string());
                        }
                    }
                }

                if !changes.add_versions.is_empty() || !changes.remove_versions.is_empty() {
                    all_changes.push(changes);
                }
            }

            if all_changes.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "✓ All path dependencies correctly configured".to_string(),
                };
                return;
            }

            // Report changes
            yield HyperforgeEvent::Info {
                message: format!("\nFound {} files needing updates:", all_changes.len()),
            };

            for changes in &all_changes {
                let rel_file = changes.path.strip_prefix(workspace).unwrap_or(&changes.path);
                yield HyperforgeEvent::Info {
                    message: format!("  {}:", rel_file.display()),
                };
                for (dep, ver) in &changes.add_versions {
                    yield HyperforgeEvent::Info {
                        message: format!("    + {} version = \"{}\" (runtime)", dep, ver),
                    };
                }
                for dep in &changes.remove_versions {
                    yield HyperforgeEvent::Info {
                        message: format!("    - {} remove version (dev-only)", dep),
                    };
                }
            }

            // Apply if not dry run
            if !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "\nApplying updates...".to_string(),
                };

                let mut success_count = 0;
                let mut fail_count = 0;

                for changes in all_changes {
                    let content = match std::fs::read_to_string(&changes.path) {
                        Ok(c) => c,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to read {}: {}", changes.path.display(), e),
                            };
                            fail_count += 1;
                            continue;
                        }
                    };

                    let mut doc = match content.parse::<toml_edit::DocumentMut>() {
                        Ok(d) => d,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to parse {}: {}", changes.path.display(), e),
                            };
                            fail_count += 1;
                            continue;
                        }
                    };

                    // Add versions to runtime deps
                    for (dep_name, version) in &changes.add_versions {
                        for section in &["dependencies", "build-dependencies"] {
                            if let Some(deps) = doc.get_mut(*section).and_then(|d| d.as_table_mut()) {
                                if let Some(dep) = deps.get_mut(dep_name) {
                                    if let Some(table) = dep.as_inline_table_mut() {
                                        if table.get("path").is_some() && table.get("version").is_none() {
                                            table.insert("version", version.as_str().into());
                                        }
                                    } else if let Some(table) = dep.as_table_mut() {
                                        if table.get("path").is_some() && table.get("version").is_none() {
                                            table.insert("version", toml_edit::value(version.as_str()));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Remove versions from dev deps
                    for dep_name in &changes.remove_versions {
                        if let Some(deps) = doc.get_mut("dev-dependencies").and_then(|d| d.as_table_mut()) {
                            if let Some(dep) = deps.get_mut(dep_name) {
                                if let Some(table) = dep.as_inline_table_mut() {
                                    table.remove("version");
                                } else if let Some(table) = dep.as_table_mut() {
                                    table.remove("version");
                                }
                            }
                        }
                    }

                    match std::fs::write(&changes.path, doc.to_string()) {
                        Ok(_) => {
                            let rel_file = changes.path.strip_prefix(workspace).unwrap_or(&changes.path);
                            yield HyperforgeEvent::Info {
                                message: format!("  ✓ Updated {}", rel_file.display()),
                            };
                            success_count += 1;
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ Failed to write {}: {}", changes.path.display(), e),
                            };
                            fail_count += 1;
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!("\nUpdated: {}, Failed: {}", success_count, fail_count),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: "\nRun with --dry_run false to apply updates".to_string(),
                };
            }
        }
    }

    /// Strip prerelease suffixes from package versions (e.g., -worktree, -alpha)
    #[plexus_macros::hub_method(
        description = "Strip prerelease suffixes (-worktree, -alpha, etc.) from package versions",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without applying changes (optional, default: true)"
        )
    )]
    pub async fn workspace_normalize_versions(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Scanning for prerelease versions to normalize...", mode),
            };

            let mut updates: Vec<(std::path::PathBuf, String, String)> = Vec::new(); // (file, old_ver, new_ver)

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace: {}", e),
                    };
                    return;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" { continue; }

                let cargo_toml = path.join("Cargo.toml");
                if cargo_toml.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let Some(version) = doc.get("package").and_then(|p| p.get("version")).and_then(|v| v.as_str()) {
                                if version.contains('-') {
                                    let base = version.split('-').next().unwrap_or(version);
                                    updates.push((cargo_toml.clone(), version.to_string(), base.to_string()));
                                }
                            }
                        }
                    }
                }

                // Also check .cabal files
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".cabal") {
                                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                    for line in content.lines() {
                                        let trimmed = line.trim_start().to_lowercase();
                                        if trimmed.starts_with("version:") {
                                            let ver = line.split(':').nth(1).map(|s| s.trim()).unwrap_or("");
                                            if ver.contains('-') {
                                                let base = ver.split('-').next().unwrap_or(ver);
                                                updates.push((entry.path(), ver.to_string(), base.to_string()));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if updates.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "✓ No prerelease versions found".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!("\nFound {} prerelease versions:", updates.len()),
            };

            for (file, old_ver, new_ver) in &updates {
                let rel_file = file.strip_prefix(workspace).unwrap_or(file);
                yield HyperforgeEvent::Info {
                    message: format!("  {}: {} → {}", rel_file.display(), old_ver, new_ver),
                };
            }

            if !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "\nApplying updates...".to_string(),
                };

                let mut success_count = 0;
                let mut fail_count = 0;

                for (file, old_ver, new_ver) in updates {
                    let content = match std::fs::read_to_string(&file) {
                        Ok(c) => c,
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ {}: {}", file.display(), e),
                            };
                            fail_count += 1;
                            continue;
                        }
                    };

                    let new_content = if file.extension().map(|e| e == "toml").unwrap_or(false) {
                        // Cargo.toml - use toml_edit to preserve formatting
                        match content.parse::<toml_edit::DocumentMut>() {
                            Ok(mut doc) => {
                                if let Some(pkg) = doc.get_mut("package") {
                                    if let Some(ver) = pkg.get_mut("version") {
                                        *ver = toml_edit::value(&new_ver);
                                    }
                                }
                                doc.to_string()
                            }
                            Err(_) => {
                                fail_count += 1;
                                continue;
                            }
                        }
                    } else {
                        // .cabal file - simple string replace
                        content.replace(&format!("version: {}", old_ver), &format!("version: {}", new_ver))
                              .replace(&format!("version:{}", old_ver), &format!("version: {}", new_ver))
                    };

                    match std::fs::write(&file, new_content) {
                        Ok(_) => {
                            let rel_file = file.strip_prefix(workspace).unwrap_or(&file);
                            yield HyperforgeEvent::Info {
                                message: format!("  ✓ {}", rel_file.display()),
                            };
                            success_count += 1;
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  ✗ {}: {}", file.display(), e),
                            };
                            fail_count += 1;
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!("\nUpdated: {}, Failed: {}", success_count, fail_count),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: "\nRun with --dry_run false to apply updates".to_string(),
                };
            }
        }
    }

    /// Prepare workspace for publishing - runs all fix commands in sequence
    #[plexus_macros::hub_method(
        description = "Prepare workspace for publishing: fix paths, normalize versions, fix dep versions",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without applying changes (optional, default: true)"
        )
    )]
    pub async fn workspace_prepare_publish(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);
        let ws_path = workspace_path.clone();

        stream! {
            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}══════ PREPARE WORKSPACE FOR PUBLISHING ══════", mode),
            };

            // Step 1: Fix stale path dependencies
            yield HyperforgeEvent::Info {
                message: format!("\n{}Step 1: Fix stale path dependencies", mode),
            };
            yield HyperforgeEvent::Info {
                message: "─".repeat(50),
            };

            // We need to inline the logic here since we can't easily call other stream methods
            let workspace = std::path::Path::new(&ws_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", ws_path),
                };
                return;
            }

            // Build package map
            let mut package_dirs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let mut package_versions: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let mut package_names: std::collections::HashSet<String> = std::collections::HashSet::new();

            if let Ok(entries) = std::fs::read_dir(workspace) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() { continue; }
                    let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" { continue; }

                    let cargo_toml = path.join("Cargo.toml");
                    if cargo_toml.exists() {
                        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                            if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                                if let (Some(name), Some(version)) = (
                                    doc.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()),
                                    doc.get("package").and_then(|p| p.get("version")).and_then(|v| v.as_str())
                                ) {
                                    let base_version = version.split('-').next().unwrap_or(version);
                                    package_dirs.insert(name.to_string(), dir_name.clone());
                                    package_versions.insert(name.to_string(), base_version.to_string());
                                    package_names.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Found {} packages in workspace", package_names.len()),
            };

            // === STEP 1: Fix stale paths ===
            let mut path_fixes = 0;
            if let Ok(entries) = std::fs::read_dir(workspace) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() { continue; }
                    let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if dir_name.starts_with('.') || dir_name == "target" { continue; }

                    let cargo_toml = path.join("Cargo.toml");
                    if !cargo_toml.exists() { continue; }

                    let content = match std::fs::read_to_string(&cargo_toml) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let mut new_content = content.clone();
                    let mut file_changed = false;

                    // Check for stale paths and fix them
                    for (pkg_name, correct_dir) in &package_dirs {
                        // Common stale patterns
                        let stale_patterns = vec![
                            format!("path = \"../hub-core\""),
                            format!("path = \"../hub-macro\""),
                            format!("path = \"../hub-transport\""),
                        ];
                        let correct_path = format!("path = \"../{}\"", correct_dir);

                        for stale in &stale_patterns {
                            if new_content.contains(stale) {
                                // Only fix if this stale path should map to this package
                                let stale_name = stale.split('/').last().unwrap_or("").trim_end_matches('"');
                                if (stale_name == "hub-core" && pkg_name == "plexus-core") ||
                                   (stale_name == "hub-macro" && pkg_name == "plexus-macros") ||
                                   (stale_name == "hub-transport" && pkg_name == "plexus-transport") {
                                    new_content = new_content.replace(stale, &correct_path);
                                    file_changed = true;
                                }
                            }
                        }
                    }

                    if file_changed && !is_dry_run {
                        if std::fs::write(&cargo_toml, &new_content).is_ok() {
                            path_fixes += 1;
                        }
                    } else if file_changed {
                        path_fixes += 1;
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("  Fixed {} stale path references", path_fixes),
            };

            // === STEP 2: Normalize versions ===
            yield HyperforgeEvent::Info {
                message: format!("\n{}Step 2: Normalize versions (strip prerelease)", mode),
            };
            yield HyperforgeEvent::Info {
                message: "─".repeat(50),
            };

            let mut version_fixes = 0;
            if let Ok(entries) = std::fs::read_dir(workspace) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() { continue; }
                    let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if dir_name.starts_with('.') || dir_name == "target" { continue; }

                    let cargo_toml = path.join("Cargo.toml");
                    if cargo_toml.exists() {
                        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                            if let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() {
                                // Get version info first
                                let version_info: Option<(String, String)> = doc
                                    .get("package")
                                    .and_then(|p| p.get("version"))
                                    .and_then(|v| v.as_str())
                                    .filter(|ver| ver.contains('-'))
                                    .map(|ver| {
                                        let base = ver.split('-').next().unwrap_or(ver);
                                        (ver.to_string(), base.to_string())
                                    });

                                if let Some((old_ver, new_ver)) = version_info {
                                    yield HyperforgeEvent::Info {
                                        message: format!("  {}: {} → {}", dir_name, old_ver, new_ver),
                                    };
                                    if !is_dry_run {
                                        if let Some(pkg) = doc.get_mut("package") {
                                            if let Some(v) = pkg.get_mut("version") {
                                                *v = toml_edit::value(&new_ver);
                                            }
                                        }
                                        let _ = std::fs::write(&cargo_toml, doc.to_string());
                                    }
                                    version_fixes += 1;
                                }
                            }
                        }
                    }
                }
            }

            if version_fixes == 0 {
                yield HyperforgeEvent::Info {
                    message: "  No prerelease versions found".to_string(),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("  Normalized {} versions", version_fixes),
                };
            }

            // === STEP 3: Fix dependency versions ===
            yield HyperforgeEvent::Info {
                message: format!("\n{}Step 3: Fix dependency versions", mode),
            };
            yield HyperforgeEvent::Info {
                message: "─".repeat(50),
            };
            yield HyperforgeEvent::Info {
                message: "  (runtime deps: add version, dev deps: remove version)".to_string(),
            };

            let mut dep_fixes = 0;
            if let Ok(entries) = std::fs::read_dir(workspace) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() { continue; }
                    let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if dir_name.starts_with('.') || dir_name == "target" { continue; }

                    let cargo_toml = path.join("Cargo.toml");
                    if !cargo_toml.exists() { continue; }

                    let content = match std::fs::read_to_string(&cargo_toml) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    let mut doc = match content.parse::<toml_edit::DocumentMut>() {
                        Ok(d) => d,
                        Err(_) => continue,
                    };

                    let mut file_changed = false;

                    // Helper to check path dep
                    let check_dep = |dep_value: &toml_edit::Item| -> (bool, bool) {
                        if let Some(table) = dep_value.as_inline_table() {
                            (table.get("path").is_some(), table.get("version").is_some())
                        } else if let Some(table) = dep_value.as_table() {
                            (table.get("path").is_some(), table.get("version").is_some())
                        } else {
                            (false, false)
                        }
                    };

                    // Add versions to runtime deps
                    for section in &["dependencies", "build-dependencies"] {
                        let deps_to_update: Vec<(String, String)> = {
                            let mut updates = Vec::new();
                            if let Some(deps) = doc.get(*section).and_then(|d| d.as_table()) {
                                for (dep_name, dep_value) in deps.iter() {
                                    if !package_names.contains(dep_name) { continue; }
                                    let (has_path, has_version) = check_dep(dep_value);
                                    if has_path && !has_version {
                                        if let Some(ver) = package_versions.get(dep_name) {
                                            updates.push((dep_name.to_string(), ver.clone()));
                                        }
                                    }
                                }
                            }
                            updates
                        };

                        for (dep_name, version) in deps_to_update {
                            if let Some(deps) = doc.get_mut(*section).and_then(|d| d.as_table_mut()) {
                                if let Some(dep) = deps.get_mut(&dep_name) {
                                    if let Some(table) = dep.as_inline_table_mut() {
                                        table.insert("version", version.as_str().into());
                                        file_changed = true;
                                    } else if let Some(table) = dep.as_table_mut() {
                                        table.insert("version", toml_edit::value(&version));
                                        file_changed = true;
                                    }
                                }
                            }
                        }
                    }

                    // Remove versions from dev deps
                    let dev_deps_to_fix: Vec<String> = {
                        let mut fixes = Vec::new();
                        if let Some(deps) = doc.get("dev-dependencies").and_then(|d| d.as_table()) {
                            for (dep_name, dep_value) in deps.iter() {
                                if !package_names.contains(dep_name) { continue; }
                                let (has_path, has_version) = check_dep(dep_value);
                                if has_path && has_version {
                                    fixes.push(dep_name.to_string());
                                }
                            }
                        }
                        fixes
                    };

                    for dep_name in dev_deps_to_fix {
                        if let Some(deps) = doc.get_mut("dev-dependencies").and_then(|d| d.as_table_mut()) {
                            if let Some(dep) = deps.get_mut(&dep_name) {
                                if let Some(table) = dep.as_inline_table_mut() {
                                    table.remove("version");
                                    file_changed = true;
                                } else if let Some(table) = dep.as_table_mut() {
                                    table.remove("version");
                                    file_changed = true;
                                }
                            }
                        }
                    }

                    if file_changed {
                        if !is_dry_run {
                            let _ = std::fs::write(&cargo_toml, doc.to_string());
                        }
                        dep_fixes += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("  ✓ {}/Cargo.toml", dir_name),
                        };
                    }
                }
            }

            if dep_fixes == 0 {
                yield HyperforgeEvent::Info {
                    message: "  All dependencies correctly configured".to_string(),
                };
            }

            // Summary
            yield HyperforgeEvent::Info {
                message: format!("\n{}══════ SUMMARY ══════", mode),
            };
            yield HyperforgeEvent::Info {
                message: format!("  Path fixes: {}", path_fixes),
            };
            yield HyperforgeEvent::Info {
                message: format!("  Version normalizations: {}", version_fixes),
            };
            yield HyperforgeEvent::Info {
                message: format!("  Dependency fixes: {}", dep_fixes),
            };

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "\nRun with --dry_run false to apply all changes".to_string(),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: "\n✓ Workspace prepared for publishing!".to_string(),
                };
                yield HyperforgeEvent::Info {
                    message: "Next: run workspace_publish_all to publish packages".to_string(),
                };
            }
        }
    }

    /// Setup [patch.crates-io] for local development
    ///
    /// Converts path+version dependencies to version-only with patches:
    ///   plexus-core = { path = "../plexus-core", version = "0.2.1" }
    /// Becomes:
    ///   plexus-core = "0.2.1"
    ///   [patch.crates-io]
    ///   plexus-core = { path = "../plexus-core" }
    ///
    /// This pattern allows:
    /// - Local development uses path patches (local changes)
    /// - cargo publish ignores [patch] sections (clean publish)
    #[plexus_macros::hub_method(
        description = "Setup [patch.crates-io] for local development: convert path deps to version-only + patches",
        params(
            workspace_path = "Path to workspace directory",
            dry_run = "Preview without applying changes (optional, default: true)"
        )
    )]
    pub async fn workspace_setup_patches(
        &self,
        workspace_path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(true);

        stream! {
            let workspace = std::path::Path::new(&workspace_path);

            if !workspace.exists() {
                yield HyperforgeEvent::Error {
                    message: format!("Workspace path does not exist: {}", workspace_path),
                };
                return;
            }

            let mode = if is_dry_run { "[DRY RUN] " } else { "" };

            yield HyperforgeEvent::Info {
                message: format!("{}Scanning workspace for packages...", mode),
            };

            // Build package map: name -> (version, absolute_path)
            let mut package_map: std::collections::HashMap<String, (String, PathBuf)> =
                std::collections::HashMap::new();

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to read workspace: {}", e),
                    };
                    return;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Skip hidden dirs and common non-package dirs
                if dir_name.starts_with('.')
                    || dir_name == "target"
                    || dir_name == "node_modules"
                    || dir_name == "worktrees"
                {
                    continue;
                }

                // Check for Cargo.toml and extract package info
                let cargo_toml = path.join("Cargo.toml");
                if cargo_toml.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let (Some(name), Some(version)) = (
                                doc.get("package")
                                    .and_then(|p| p.get("name"))
                                    .and_then(|n| n.as_str()),
                                doc.get("package")
                                    .and_then(|p| p.get("version"))
                                    .and_then(|v| v.as_str()),
                            ) {
                                package_map.insert(
                                    name.to_string(),
                                    (version.to_string(), path.clone()),
                                );
                            }
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Found {} Cargo packages", package_map.len()),
            };

            // Process each package
            let mut total_patches = 0;
            let mut packages_modified = 0;

            let entries = match std::fs::read_dir(workspace) {
                Ok(e) => e,
                Err(_) => return,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if dir_name.starts_with('.')
                    || dir_name == "target"
                    || dir_name == "node_modules"
                    || dir_name == "worktrees"
                {
                    continue;
                }

                let cargo_toml = path.join("Cargo.toml");
                if !cargo_toml.exists() {
                    continue;
                }

                // Get package name for reporting
                let pkg_name = std::fs::read_to_string(&cargo_toml)
                    .ok()
                    .and_then(|content| content.parse::<toml_edit::DocumentMut>().ok())
                    .and_then(|doc| {
                        doc.get("package")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| dir_name.clone());

                if is_dry_run {
                    // Dry run: just report what would be done
                    // We need to analyze without writing
                    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            let mut would_patch: Vec<String> = Vec::new();

                            // Check all dependency sections for path deps to workspace packages
                            for section in ["dependencies", "build-dependencies", "dev-dependencies"]
                            {
                                if let Some(deps) = doc.get(section).and_then(|d| d.as_table()) {
                                    for (dep_name, dep_value) in deps.iter() {
                                        // Check if it has a path dependency
                                        let has_path = if let Some(table) =
                                            dep_value.as_inline_table()
                                        {
                                            table.get("path").is_some()
                                        } else if let Some(table) = dep_value.as_table() {
                                            table.get("path").is_some()
                                        } else {
                                            false
                                        };

                                        if has_path && package_map.contains_key(dep_name) {
                                            would_patch.push(dep_name.to_string());
                                        }
                                    }
                                }
                            }

                            if !would_patch.is_empty() {
                                yield HyperforgeEvent::Info {
                                    message: format!(
                                        "  {}: would add {} patches: {:?}",
                                        pkg_name,
                                        would_patch.len(),
                                        would_patch
                                    ),
                                };
                                total_patches += would_patch.len();
                                packages_modified += 1;
                            }
                        }
                    }
                } else {
                    // Apply changes
                    match packages::cargo::CargoManifest::setup_patches(&path, &package_map) {
                        Ok(result) => {
                            if result.patches_added.is_empty() {
                                // No changes needed
                            } else {
                                let patch_names: Vec<&str> = result
                                    .patches_added
                                    .iter()
                                    .map(|(n, _)| n.as_str())
                                    .collect();
                                yield HyperforgeEvent::Info {
                                    message: format!(
                                        "  {}: added {} patches: {:?}",
                                        pkg_name,
                                        result.patches_added.len(),
                                        patch_names
                                    ),
                                };
                                total_patches += result.patches_added.len();
                                packages_modified += 1;
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  {}: {}", pkg_name, e),
                            };
                        }
                    }
                }
            }

            // Summary
            if total_patches == 0 {
                yield HyperforgeEvent::Info {
                    message: "✓ No path dependencies to convert (already using patches or no local deps)".to_string(),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "\n{}Summary: {} patches across {} packages",
                        mode, total_patches, packages_modified
                    ),
                };

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: "\nRun with --dry_run false to apply changes".to_string(),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: "\n✓ Patches configured! Run `cargo check` in each repo to verify.".to_string(),
                    };
                }
            }
        }
    }
}
