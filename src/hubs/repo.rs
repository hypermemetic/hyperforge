//! RepoHub - Single-repo operations and registry CRUD

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;
use std::sync::Arc;

use crate::adapters::{CodebergAdapter, ForgePort, GitHubAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::commands::{init, push, status};
use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;
use crate::types::{Forge, Repo, Visibility};

/// Sub-hub for single-repo operations and registry CRUD
#[derive(Clone)]
pub struct RepoHub {
    pub(crate) state: HyperforgeState,
}

impl RepoHub {
    pub fn new(state: HyperforgeState) -> Self {
        Self { state }
    }
}

#[plexus_macros::hub_methods(
    namespace = "repo",
    description = "Single-repo operations and registry CRUD",
    crate_path = "plexus_core"
)]
impl RepoHub {
    /// List repositories for an organization (from LocalForge)
    #[plexus_macros::hub_method(
        description = "List all repositories in the local forge for an organization",
        params(org = "Organization name")
    )]
    pub async fn list(
        &self,
        org: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            let local = state.get_local_forge(&org).await;

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
                            staged_for_deletion: repo.staged_for_deletion,
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
    pub async fn create(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

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
            let local = state.get_local_forge(&org).await;

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
                        staged_for_deletion: repo.staged_for_deletion,
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
    pub async fn update(
        &self,
        org: String,
        name: String,
        description: Option<String>,
        visibility: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            let local = state.get_local_forge(&org).await;

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
    pub async fn delete(
        &self,
        org: String,
        name: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            let local = state.get_local_forge(&org).await;

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

    /// Rename a repository on remote forge(s) and in local config
    #[plexus_macros::hub_method(
        description = "Rename a repository on remote forge(s) and update local configuration",
        params(
            org = "Organization name",
            old_name = "Current repository name",
            new_name = "New repository name",
            forges = "Comma-separated forges to rename on (optional, defaults to origin only)"
        )
    )]
    pub async fn rename(
        &self,
        org: String,
        old_name: String,
        new_name: String,
        forges: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            // Get local forge and verify repo exists
            let local = state.get_local_forge(&org).await;

            let repo = match local.get_repo(&org, &old_name).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Repository not found in local config: {}", e),
                    };
                    return;
                }
            };

            // Determine which forges to rename on
            let target_forges: Vec<Forge> = if let Some(forge_list) = forges {
                forge_list
                    .split(',')
                    .filter_map(|f| match f.trim().to_lowercase().as_str() {
                        "github" => Some(Forge::GitHub),
                        "codeberg" => Some(Forge::Codeberg),
                        "gitlab" => Some(Forge::GitLab),
                        _ => None,
                    })
                    .collect()
            } else {
                // Default to origin only
                vec![repo.origin.clone()]
            };

            // Get auth provider
            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            // Rename on each target forge
            let mut errors = Vec::new();
            for forge in &target_forges {
                let adapter: Box<dyn ForgePort> = match forge {
                    Forge::GitHub => {
                        match GitHubAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("GitHub: {}", e));
                                continue;
                            }
                        }
                    }
                    Forge::Codeberg => {
                        match CodebergAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("Codeberg: {}", e));
                                continue;
                            }
                        }
                    }
                    Forge::GitLab => {
                        match GitLabAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("GitLab: {}", e));
                                continue;
                            }
                        }
                    }
                };

                match adapter.rename_repo(&org, &old_name, &new_name).await {
                    Ok(_) => {
                        yield HyperforgeEvent::Info {
                            message: format!("Renamed on {:?}: {} -> {}", forge, old_name, new_name),
                        };
                    }
                    Err(e) => {
                        errors.push(format!("{:?}: {}", forge, e));
                    }
                }
            }

            // Report any errors from remote renames
            for error in &errors {
                yield HyperforgeEvent::Error {
                    message: format!("Remote rename failed - {}", error),
                };
            }

            // Update local config regardless of remote errors (user may want to fix manually)
            match local.rename_repo(&org, &old_name, &new_name).await {
                Ok(_) => {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save repos.yaml: {}", e),
                        };
                        return;
                    }

                    yield HyperforgeEvent::Info {
                        message: format!("Local config updated: {} -> {}", old_name, new_name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to update local config: {}", e),
                    };
                }
            }

            if errors.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("Renamed repository: {} -> {}", old_name, new_name),
                };
            } else {
                yield HyperforgeEvent::Error {
                    message: format!("Rename completed with {} error(s)", errors.len()),
                };
            }
        }
    }

    /// Set the default branch on remote forges and optionally checkout locally
    #[plexus_macros::hub_method(
        description = "Set the default branch on remote forges for a repository, and optionally git checkout locally",
        params(
            org = "Organization name",
            name = "Repository name",
            branch = "Branch to set as default",
            checkout = "Also run git checkout locally (optional, default: false)",
            path = "Local repo path for checkout (required if --checkout is true)"
        )
    )]
    pub async fn set_default_branch(
        &self,
        org: String,
        name: String,
        branch: String,
        checkout: Option<bool>,
        path: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            // Get local forge to find repo config (forges)
            let local = state.get_local_forge(&org).await;

            let repo = match local.get_repo(&org, &name).await {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Repository not found in local config: {}", e),
                    };
                    return;
                }
            };

            // Collect all forges (origin + mirrors)
            let mut target_forges = vec![repo.origin.clone()];
            for mirror in &repo.mirrors {
                if !target_forges.contains(mirror) {
                    target_forges.push(mirror.clone());
                }
            }

            // Get auth provider
            let auth = match YamlAuthProvider::new() {
                Ok(provider) => Arc::new(provider),
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create auth provider: {}", e),
                    };
                    return;
                }
            };

            // Set default branch on each forge
            let mut errors = Vec::new();
            for forge in &target_forges {
                let adapter: Box<dyn ForgePort> = match forge {
                    Forge::GitHub => {
                        match GitHubAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("GitHub: {}", e));
                                continue;
                            }
                        }
                    }
                    Forge::Codeberg => {
                        match CodebergAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("Codeberg: {}", e));
                                continue;
                            }
                        }
                    }
                    Forge::GitLab => {
                        match GitLabAdapter::new(auth.clone(), &org) {
                            Ok(a) => Box::new(a),
                            Err(e) => {
                                errors.push(format!("GitLab: {}", e));
                                continue;
                            }
                        }
                    }
                };

                match adapter.set_default_branch(&org, &name, &branch).await {
                    Ok(_) => {
                        yield HyperforgeEvent::Info {
                            message: format!("Set default branch to '{}' on {:?}", branch, forge),
                        };
                    }
                    Err(e) => {
                        errors.push(format!("{:?}: {}", forge, e));
                    }
                }
            }

            // Report errors
            for error in &errors {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to set default branch - {}", error),
                };
            }

            // Optionally checkout locally
            if checkout.unwrap_or(false) {
                if let Some(ref repo_path) = path {
                    let repo_path = std::path::Path::new(repo_path);
                    match crate::git::Git::checkout(repo_path, &branch) {
                        Ok(_) => {
                            yield HyperforgeEvent::Info {
                                message: format!("Checked out '{}' locally", branch),
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("Git checkout failed: {}", e),
                            };
                        }
                    }
                } else {
                    yield HyperforgeEvent::Error {
                        message: "--path is required when --checkout is true".to_string(),
                    };
                }
            }

            if errors.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("Default branch set to '{}' on all forges", branch),
                };
            } else {
                yield HyperforgeEvent::Error {
                    message: format!("Completed with {} error(s)", errors.len()),
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
    pub async fn import(
        &self,
        org: String,
        forge: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

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
            let local = state.get_local_forge(&org).await;

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
                            staged_for_deletion: repo.staged_for_deletion,
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
            dry_run = "Preview changes without applying (optional, default: false)",
            no_hooks = "Skip installing pre-push hook (optional, default: false)",
            no_ssh_wrapper = "Skip configuring SSH wrapper (optional, default: false)"
        )
    )]
    pub async fn init(
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
        no_hooks: Option<bool>,
        no_ssh_wrapper: Option<bool>,
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

            if no_hooks.unwrap_or(false) {
                options = options.no_hooks();
            }

            if no_ssh_wrapper.unwrap_or(false) {
                options = options.no_ssh_wrapper();
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

                    if report.hooks_installed {
                        yield HyperforgeEvent::Info {
                            message: "Installed pre-push hook".to_string(),
                        };
                    }

                    if report.ssh_configured {
                        yield HyperforgeEvent::Info {
                            message: "Configured SSH wrapper (hyperforge-ssh)".to_string(),
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
    pub async fn status(
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

                    // SSH configuration health
                    match (&report.ssh_command, &report.hyperforge_org) {
                        (Some(cmd), Some(org)) if cmd == "hyperforge-ssh" => {
                            yield HyperforgeEvent::Info {
                                message: format!("SSH: hyperforge-ssh (org: {})", org),
                            };
                        }
                        (Some(cmd), None) if cmd == "hyperforge-ssh" => {
                            yield HyperforgeEvent::Error {
                                message: "SSH: hyperforge-ssh configured but hyperforge.org NOT SET — pushes will use wrong key".to_string(),
                            };
                        }
                        (Some(cmd), _) => {
                            yield HyperforgeEvent::Info {
                                message: format!("SSH: custom ({})", cmd),
                            };
                        }
                        (None, _) => {
                            yield HyperforgeEvent::Info {
                                message: "SSH: system default".to_string(),
                            };
                        }
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
    pub async fn push(
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

    /// Clone a repository from LocalForge
    #[plexus_macros::hub_method(
        description = "Clone a repository by name from LocalForge, auto-initialize with hyperforge config",
        params(
            org = "Organization name",
            name = "Repository name (must exist in LocalForge)",
            path = "Target directory path (optional, defaults to ./<name>)",
            forge = "Preferred forge to clone from (optional, defaults to first in present_on)"
        )
    )]
    pub async fn clone(
        &self,
        org: String,
        name: String,
        path: Option<String>,
        forge: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();

        stream! {
            // 1. Lookup repo in LocalForge
            let local = state.get_local_forge(&org).await;

            let record = match local.get_record(&name) {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Repository not found in LocalForge: {}", e),
                    };
                    return;
                }
            };

            // 2. Pick clone forge
            let clone_forge = if let Some(ref forge_str) = forge {
                match crate::config::HyperforgeConfig::parse_forge(forge_str) {
                    Some(f) => {
                        if !record.present_on.contains(&f) {
                            yield HyperforgeEvent::Error {
                                message: format!("Repository not present on forge: {}", forge_str),
                            };
                            return;
                        }
                        f
                    }
                    None => {
                        yield HyperforgeEvent::Error {
                            message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge_str),
                        };
                        return;
                    }
                }
            } else {
                // Use first forge from present_on
                match record.present_on.iter().next() {
                    Some(f) => f.clone(),
                    None => {
                        yield HyperforgeEvent::Error {
                            message: "Repository has no forges in present_on".to_string(),
                        };
                        return;
                    }
                }
            };

            // 3. Build clone URL
            let forge_str = format!("{:?}", clone_forge).to_lowercase();
            let clone_url = crate::git::build_remote_url(&forge_str, &org, &name);

            // 4. Determine target path
            let target_path = path.unwrap_or_else(|| name.clone());

            yield HyperforgeEvent::Info {
                message: format!("Cloning {} from {} into {}", name, forge_str, target_path),
            };

            // 5. Clone
            if let Err(e) = crate::git::Git::clone(&clone_url, &target_path) {
                yield HyperforgeEvent::Error {
                    message: format!("Git clone failed: {}", e),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: "Clone successful".to_string(),
            };

            // 6. Check for .hyperforge/config.toml in clone
            let clone_path = std::path::Path::new(&target_path);
            let has_config = crate::config::HyperforgeConfig::exists(clone_path);

            if !has_config {
                // Generate config from RepoRecord metadata
                let forges: Vec<String> = record.present_on.iter()
                    .map(|f| format!("{:?}", f).to_lowercase())
                    .collect();

                let mut config = crate::config::HyperforgeConfig::new(forges.clone())
                    .with_org(&org)
                    .with_repo_name(&name)
                    .with_visibility(record.visibility.clone());

                if let Some(ref desc) = record.description {
                    config = config.with_description(desc);
                }

                if let Err(e) = config.save(clone_path) {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to create .hyperforge/config.toml: {}", e),
                    };
                    // Continue anyway - clone succeeded
                } else {
                    yield HyperforgeEvent::Info {
                        message: "Generated .hyperforge/config.toml from LocalForge metadata".to_string(),
                    };
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: "Found existing .hyperforge/config.toml".to_string(),
                };
            }

            // 7. Add remotes for all forges in present_on
            for f in &record.present_on {
                if *f == clone_forge {
                    continue; // Already set as "origin" by git clone
                }
                let f_str = format!("{:?}", f).to_lowercase();
                let remote_url = crate::git::build_remote_url(&f_str, &org, &name);
                match crate::git::Git::add_remote(clone_path, &f_str, &remote_url) {
                    Ok(_) => {
                        yield HyperforgeEvent::Info {
                            message: format!("Added remote {} → {}", f_str, remote_url),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to add remote {}: {}", f_str, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("Repository {} cloned and configured", name),
            };
        }
    }
}

#[async_trait]
impl ChildRouter for RepoHub {
    fn router_namespace(&self) -> &str {
        "repo"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
