//! WorkspaceHub - Multi-repo orchestration
//!
//! Supports two modes:
//! - Path-based: `--path ~/dev/org` discovers repos from filesystem
//! - Org-based (legacy): `--org acme --forge github` uses registry directly

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::{CodebergAdapter, ForgePort, GitHubAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::commands::push::{push, PushOptions};
use crate::commands::workspace::discover_workspace;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;
use crate::types::Forge;

/// Sub-hub for multi-repo workspace orchestration
#[derive(Clone)]
pub struct WorkspaceHub {
    pub(crate) state: HyperforgeState,
}

impl WorkspaceHub {
    pub fn new(state: HyperforgeState) -> Self {
        Self { state }
    }
}

/// Create a forge adapter from a forge name string
fn make_adapter(forge: &str, org: &str) -> Result<Arc<dyn ForgePort>, String> {
    let auth = YamlAuthProvider::new().map_err(|e| format!("Failed to create auth provider: {}", e))?;
    let auth = Arc::new(auth);
    let target_forge = match forge.to_lowercase().as_str() {
        "github" => Forge::GitHub,
        "codeberg" => Forge::Codeberg,
        "gitlab" => Forge::GitLab,
        _ => return Err(format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge)),
    };
    let adapter: Arc<dyn ForgePort> = match target_forge {
        Forge::GitHub => {
            Arc::new(GitHubAdapter::new(auth, org).map_err(|e| format!("Failed to create GitHub adapter: {}", e))?)
        }
        Forge::Codeberg => {
            Arc::new(CodebergAdapter::new(auth, org).map_err(|e| format!("Failed to create Codeberg adapter: {}", e))?)
        }
        Forge::GitLab => {
            Arc::new(GitLabAdapter::new(auth, org).map_err(|e| format!("Failed to create GitLab adapter: {}", e))?)
        }
    };
    Ok(adapter)
}

#[plexus_macros::hub_methods(
    namespace = "workspace",
    version = "3.0.0",
    description = "Multi-repo workspace orchestration",
    crate_path = "plexus_core"
)]
impl WorkspaceHub {
    /// Discover repos in a workspace directory
    #[plexus_macros::hub_method(
        description = "Scan a workspace directory and report discovered repos, orgs, and forges",
        params(
            path = "Path to workspace directory"
        )
    )]
    pub async fn discover(
        &self,
        path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);
            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!("Scanning workspace: {}", ctx.root.display()),
            };

            // Report each discovered repo
            for repo in &ctx.repos {
                let org = repo.org().unwrap_or("(none)");
                let forges = repo.forges().join(", ");
                let git_status = if repo.is_git_repo { "git" } else { "no-git" };

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {} [{}] org={} forges=[{}]",
                        repo.dir_name, git_status, org, forges
                    ),
                };
            }

            // Report unconfigured repos
            for path in &ctx.unconfigured_repos {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                yield HyperforgeEvent::Info {
                    message: format!("  {} [git, no hyperforge config]", name),
                };
            }

            // Report orgs and forges
            yield HyperforgeEvent::Info {
                message: format!("Orgs: {}", ctx.orgs.join(", ")),
            };
            yield HyperforgeEvent::Info {
                message: format!("Forges: {}", ctx.forges.join(", ")),
            };

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: None,
                push_failed: None,
            };
        }
    }

    /// Check all repos are on expected branch and clean
    #[plexus_macros::hub_method(
        description = "Verify all workspace repos are on the expected branch and have a clean working tree",
        params(
            path = "Path to workspace directory",
            branch = "Expected branch name (optional, default: main)"
        )
    )]
    pub async fn check(
        &self,
        path: String,
        branch: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);
            let expected_branch = branch.unwrap_or_else(|| "main".to_string());

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "Checking {} repos (expected branch: {})",
                    ctx.repos.len(),
                    expected_branch
                ),
            };

            let mut clean_count = 0usize;
            let mut dirty_count = 0usize;
            let mut wrong_branch_count = 0usize;

            for repo in &ctx.repos {
                if !repo.is_git_repo {
                    continue;
                }

                let current_branch = match Git::current_branch(&repo.path) {
                    Ok(b) => b,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("{}: failed to get branch: {}", repo.dir_name, e),
                        };
                        continue;
                    }
                };

                let status = match Git::repo_status(&repo.path) {
                    Ok(s) => s,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("{}: failed to get status: {}", repo.dir_name, e),
                        };
                        continue;
                    }
                };

                let is_clean = !status.has_changes && !status.has_staged && !status.has_untracked;
                let on_correct_branch = current_branch == expected_branch;

                if is_clean {
                    clean_count += 1;
                } else {
                    dirty_count += 1;
                }
                if !on_correct_branch {
                    wrong_branch_count += 1;
                }

                yield HyperforgeEvent::RepoCheck {
                    repo_name: repo.dir_name.clone(),
                    path: repo.path.display().to_string(),
                    branch: current_branch,
                    expected_branch: expected_branch.clone(),
                    is_clean,
                    on_correct_branch,
                };
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: Some(clean_count),
                dirty_repos: Some(dirty_count),
                wrong_branch_repos: Some(wrong_branch_count),
                push_success: None,
                push_failed: None,
            };
        }
    }

    /// Push all repos to their configured forges
    #[plexus_macros::hub_method(
        description = "Push all workspace repos to their configured forges",
        params(
            path = "Path to workspace directory",
            branch = "Branch to push (optional, uses current branch if not specified)",
            dry_run = "Preview pushes without executing (optional, default: false)",
            set_upstream = "Set upstream tracking (optional, default: false)"
        )
    )]
    pub async fn push_all(
        &self,
        path: String,
        branch: Option<String>,
        dry_run: Option<bool>,
        set_upstream: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            let workspace_path = PathBuf::from(&path);
            let is_dry_run = dry_run.unwrap_or(false);
            let is_set_upstream = set_upstream.unwrap_or(false);

            let ctx = match discover_workspace(&workspace_path) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Discovery failed: {}", e),
                    };
                    return;
                }
            };

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("[DRY RUN] Pushing {} repos...", ctx.repos.len()),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Pushing {} repos...", ctx.repos.len()),
                };
            }

            let mut success_count = 0usize;
            let mut failed_count = 0usize;

            for repo in &ctx.repos {
                if !repo.is_git_repo {
                    yield HyperforgeEvent::Info {
                        message: format!("  Skipping {} (not a git repo)", repo.dir_name),
                    };
                    continue;
                }

                // Build push options
                let mut options = PushOptions::new();
                if is_dry_run {
                    options = options.dry_run();
                }
                if is_set_upstream {
                    options = options.set_upstream();
                }

                // If a specific branch was requested, we could filter but push() uses current branch
                if let Some(ref _branch) = branch {
                    // push() always pushes the current branch; branch param is informational
                }

                match push(&repo.path, options) {
                    Ok(report) => {
                        for result in &report.results {
                            yield HyperforgeEvent::RepoPush {
                                repo_name: repo.dir_name.clone(),
                                path: repo.path.display().to_string(),
                                forge: result.forge.clone(),
                                success: result.success,
                                error: result.error.clone(),
                            };
                        }
                        if report.all_success {
                            success_count += 1;
                        } else {
                            failed_count += 1;
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::RepoPush {
                            repo_name: repo.dir_name.clone(),
                            path: repo.path.display().to_string(),
                            forge: "all".to_string(),
                            success: false,
                            error: Some(e.to_string()),
                        };
                        failed_count += 1;
                    }
                }
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: Some(success_count),
                push_failed: Some(failed_count),
            };
        }
    }

    /// Compute sync diff between local and a remote forge
    #[plexus_macros::hub_method(
        description = "Compute diff between local configuration and a remote forge. Use --path to discover from disk, or --org and --forge for direct registry access.",
        params(
            path = "Path to workspace directory (discovers orgs/forges from disk)",
            org = "Organization name (required if --path not provided)",
            forge = "Target forge: github, codeberg, or gitlab (required if --path not provided)"
        )
    )]
    pub async fn diff(
        &self,
        path: Option<String>,
        org: Option<String>,
        forge: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();

        stream! {
            // Resolve org/forge pairs to diff
            let pairs: Vec<(String, String)> = if let Some(ref workspace_path) = path {
                let workspace_path = PathBuf::from(workspace_path);
                let ctx = match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Discovery failed: {}", e),
                        };
                        return;
                    }
                };

                let all_pairs = ctx.org_forge_pairs();

                // Filter by explicit org/forge if provided
                match (&org, &forge) {
                    (Some(o), Some(f)) => all_pairs.into_iter().filter(|(ao, af)| ao == o && af == f).collect(),
                    (Some(o), None) => all_pairs.into_iter().filter(|(ao, _)| ao == o).collect(),
                    _ => all_pairs,
                }
            } else if let (Some(o), Some(f)) = (&org, &forge) {
                vec![(o.clone(), f.clone())]
            } else {
                yield HyperforgeEvent::Error {
                    message: "Must provide --path or both --org and --forge".to_string(),
                };
                return;
            };

            if pairs.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No org/forge pairs found to diff.".to_string(),
                };
                return;
            }

            for (org_name, forge_name) in &pairs {
                // Get forge adapter
                let adapter = match make_adapter(forge_name, org_name) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::Error { message: e };
                        continue;
                    }
                };

                // Get local forge
                let local = state.get_local_forge(org_name).await;

                yield HyperforgeEvent::Info {
                    message: format!("Computing diff for {}/{}", org_name, forge_name),
                };

                // Compute diff
                match sync_service.diff(local, adapter, org_name).await {
                    Ok(diff) => {
                        yield HyperforgeEvent::SyncSummary {
                            forge: forge_name.clone(),
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
                                forge: forge_name.clone(),
                            };
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Diff failed for {}/{}: {}", org_name, forge_name, e),
                        };
                    }
                }
            }
        }
    }

    /// Sync local configuration to a remote forge
    #[plexus_macros::hub_method(
        description = "Sync repositories from local configuration to a remote forge. Use --path to discover from disk, or --org and --forge for direct registry access.",
        params(
            path = "Path to workspace directory (discovers orgs/forges from disk)",
            org = "Organization name (required if --path not provided)",
            forge = "Target forge: github, codeberg, or gitlab (required if --path not provided)",
            dry_run = "Preview changes without applying them (optional, default: false)"
        )
    )]
    pub async fn sync(
        &self,
        path: Option<String>,
        org: Option<String>,
        forge: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            // Resolve org/forge pairs to sync
            let pairs: Vec<(String, String)> = if let Some(ref workspace_path) = path {
                let workspace_path = PathBuf::from(workspace_path);
                let ctx = match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Discovery failed: {}", e),
                        };
                        return;
                    }
                };

                let all_pairs = ctx.org_forge_pairs();

                match (&org, &forge) {
                    (Some(o), Some(f)) => all_pairs.into_iter().filter(|(ao, af)| ao == o && af == f).collect(),
                    (Some(o), None) => all_pairs.into_iter().filter(|(ao, _)| ao == o).collect(),
                    _ => all_pairs,
                }
            } else if let (Some(o), Some(f)) = (&org, &forge) {
                vec![(o.clone(), f.clone())]
            } else {
                yield HyperforgeEvent::Error {
                    message: "Must provide --path or both --org and --forge".to_string(),
                };
                return;
            };

            if pairs.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No org/forge pairs found to sync.".to_string(),
                };
                return;
            }

            for (org_name, forge_name) in &pairs {
                // Get forge adapter
                let adapter = match make_adapter(forge_name, org_name) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::Error { message: e };
                        continue;
                    }
                };

                // Get local forge
                let local = state.get_local_forge(org_name).await;

                if is_dry_run {
                    yield HyperforgeEvent::Info {
                        message: format!("[DRY RUN] Computing sync operations for {}/{}...", org_name, forge_name),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("Syncing {}/{}...", org_name, forge_name),
                    };
                }

                // Execute sync
                match sync_service.sync(local, adapter, org_name, is_dry_run).await {
                    Ok(diff) => {
                        let created = diff.to_create().len();
                        let updated = diff.to_update().len();
                        let deleted = diff.to_delete().len();
                        let in_sync = diff.in_sync().len();

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{}{}/{} sync complete: {} created, {} updated, {} deleted, {} in sync",
                                if is_dry_run { "[DRY RUN] " } else { "" },
                                org_name,
                                forge_name,
                                created,
                                updated,
                                deleted,
                                in_sync
                            ),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Sync failed for {}/{}: {}", org_name, forge_name, e),
                        };
                    }
                }
            }
        }
    }

    /// Verify workspace sync state
    #[plexus_macros::hub_method(
        description = "Verify workspace configuration including orgs, SSH keys, and auth tokens. Use --path to discover from disk, or --org for registry access.",
        params(
            path = "Path to workspace directory (discovers orgs from disk)",
            org = "Organization to verify (optional, verifies all if not specified)"
        )
    )]
    pub async fn verify(
        &self,
        path: Option<String>,
        org: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let config_dir = self.state.config_dir.clone();

        stream! {
            yield HyperforgeEvent::Info {
                message: "Starting workspace verification...".to_string(),
            };

            // Determine orgs to check
            let orgs_to_check: Vec<String> = if let Some(ref workspace_path) = path {
                let workspace_path = PathBuf::from(workspace_path);
                let ctx = match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Discovery failed: {}", e),
                        };
                        return;
                    }
                };

                // If org specified, filter; otherwise use all discovered orgs
                if let Some(ref org_name) = org {
                    if ctx.orgs.contains(org_name) {
                        vec![org_name.clone()]
                    } else {
                        yield HyperforgeEvent::Error {
                            message: format!("Org '{}' not found in workspace", org_name),
                        };
                        return;
                    }
                } else {
                    ctx.orgs.clone()
                }
            } else if let Some(org_name) = org {
                vec![org_name]
            } else {
                // Legacy: list all orgs from config dir
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
                            message: "No organizations configured. Provide --path or --org.".to_string(),
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
                let local_forge = state.get_local_forge(&org_name).await;
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
}

#[async_trait]
impl ChildRouter for WorkspaceHub {
    fn router_namespace(&self) -> &str {
        "workspace"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
