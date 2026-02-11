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
use crate::commands::init::{init, InitOptions};
use crate::commands::push::{push, PushOptions};
use crate::commands::workspace::{discover_workspace, repo_from_config};
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;
use crate::services::SyncOp;
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
    version = "3.1.0",
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

    /// Full safe sync pipeline: discover → init → register → import → diff → apply (no deletes) → push
    #[plexus_macros::hub_method(
        description = "Full safe sync pipeline. Discovers repos, initializes unconfigured ones, registers in LocalForge, imports remote-only repos, applies creates/updates (never deletes), and pushes git content.",
        params(
            path = "Path to workspace directory (required)",
            org = "Organization name (inferred from workspace if only one org exists)",
            forges = "Forges for unconfigured repos (inferred from existing configs if not specified)",
            dry_run = "Preview all phases without making changes (optional, default: false)",
            no_push = "Skip the git push phase (optional, default: false)",
            no_init = "Skip initializing unconfigured repos (optional, default: false)"
        )
    )]
    pub async fn sync(
        &self,
        path: String,
        org: Option<String>,
        forges: Option<Vec<String>>,
        dry_run: Option<bool>,
        no_push: Option<bool>,
        no_init: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);
        let is_no_push = no_push.unwrap_or(false);
        let is_no_init = no_init.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = if is_dry_run { "[DRY RUN] " } else { "" };

            // ── Phase 1: Discover ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 1/8: Discovering workspace...", dry_prefix),
            };

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
                    "  Found {} configured, {} unconfigured, {} non-git dirs. Orgs: [{}], Forges: [{}]",
                    ctx.repos.len(),
                    ctx.unconfigured_repos.len(),
                    ctx.skipped_dirs.len(),
                    ctx.orgs.join(", "),
                    ctx.forges.join(", "),
                ),
            };

            // Report non-git directories (no .git, no .hyperforge)
            if !ctx.skipped_dirs.is_empty() {
                for dir in &ctx.skipped_dirs {
                    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    yield HyperforgeEvent::Info {
                        message: format!("  {} [no git — needs git init]", name),
                    };
                }
            }

            // ── Infer org and forges for unconfigured repos ──
            let inferred_org: Option<String> = org.clone().or_else(|| {
                if ctx.orgs.len() == 1 {
                    Some(ctx.orgs[0].clone())
                } else {
                    None
                }
            });

            let inferred_forges: Vec<String> = forges.clone().unwrap_or_else(|| ctx.forges.clone());

            let has_unconfigured = !ctx.unconfigured_repos.is_empty();

            if has_unconfigured && inferred_org.is_none() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init unconfigured repos: multiple orgs found and --org not specified. Skipping init phase.".to_string(),
                };
            }
            if has_unconfigured && inferred_forges.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init unconfigured repos: no forges found and --forges not specified. Skipping init phase.".to_string(),
                };
            }

            // ── Phase 2: Init unconfigured repos ──
            let mut inits_performed = 0usize;

            if !is_no_init && has_unconfigured && inferred_org.is_some() && !inferred_forges.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "{}Phase 2/8: Initializing {} unconfigured repos (org={}, forges=[{}])...",
                        dry_prefix,
                        ctx.unconfigured_repos.len(),
                        inferred_org.as_deref().unwrap_or("?"),
                        inferred_forges.join(", "),
                    ),
                };

                for repo_path in &ctx.unconfigured_repos {
                    let dir_name = repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?");

                    let mut opts = InitOptions::new(inferred_forges.clone());
                    if let Some(ref o) = inferred_org {
                        opts = opts.with_org(o.as_str());
                    }
                    if is_dry_run {
                        opts = opts.dry_run();
                    }

                    match init(repo_path, opts) {
                        Ok(_report) => {
                            inits_performed += 1;
                            yield HyperforgeEvent::Info {
                                message: format!("  {}Initialized {}", dry_prefix, dir_name),
                            };
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to init {}: {}", dir_name, e),
                            };
                        }
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 2/8: Init skipped.", dry_prefix),
                };
            }

            // ── Phase 3: Re-discover ──
            let ctx = if inits_performed > 0 && !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 3/8: Re-discovering after {} inits...", dry_prefix, inits_performed),
                };
                match discover_workspace(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("Re-discovery failed: {}", e),
                        };
                        return;
                    }
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 3/8: Re-discover skipped.", dry_prefix),
                };
                ctx
            };

            // ── Phase 4: Register configured repos in LocalForge ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 4/8: Registering {} configured repos in LocalForge...", dry_prefix, ctx.repos.len()),
            };

            let mut registered = 0usize;
            let mut already_registered = 0usize;

            for discovered in &ctx.repos {
                let repo = match repo_from_config(discovered) {
                    Some(r) => r,
                    None => continue,
                };
                let repo_org = match discovered.org() {
                    Some(o) => o.to_string(),
                    None => continue,
                };

                let local = state.get_local_forge(&repo_org).await;

                // Check if already registered
                match local.repo_exists(&repo_org, &repo.name).await {
                    Ok(true) => {
                        already_registered += 1;
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to check {}: {}", repo.name, e),
                        };
                        continue;
                    }
                }

                // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
                if let Err(e) = local.create_repo(&repo_org, &repo).await {
                    yield HyperforgeEvent::Error {
                        message: format!("  Failed to register {}: {}", repo.name, e),
                    };
                    continue;
                }

                registered += 1;
                yield HyperforgeEvent::Info {
                    message: format!("  {}Registered {}", dry_prefix, repo.name),
                };
            }

            // Persist to disk only on real runs
            if !is_dry_run && registered > 0 {
                // Save each org's LocalForge
                for org_name in &ctx.orgs {
                    let local = state.get_local_forge(org_name).await;
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {} newly registered, {} already in LocalForge",
                    registered, already_registered,
                ),
            };

            // ── Phase 5: Import remote-only repos into LocalForge ──
            let pairs = ctx.org_forge_pairs();

            yield HyperforgeEvent::Info {
                message: format!("{}Phase 5/8: Importing remote-only repos for {} org/forge pairs...", dry_prefix, pairs.len()),
            };

            let mut imported = 0usize;

            for (org_name, forge_name) in &pairs {
                let adapter = match make_adapter(forge_name, org_name) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::Error { message: e };
                        continue;
                    }
                };

                let remote_repos = match adapter.list_repos(org_name).await {
                    Ok(repos) => repos,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to list remote repos for {}/{}: {}", org_name, forge_name, e),
                        };
                        continue;
                    }
                };

                let local = state.get_local_forge(org_name).await;

                for remote_repo in &remote_repos {
                    match local.repo_exists(org_name, &remote_repo.name).await {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => {
                            yield HyperforgeEvent::Error {
                                message: format!("  Failed to check {}: {}", remote_repo.name, e),
                            };
                            continue;
                        }
                    }

                    // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
                    if let Err(e) = local.create_repo(org_name, remote_repo).await {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to import {}: {}", remote_repo.name, e),
                        };
                        continue;
                    }

                    imported += 1;
                    yield HyperforgeEvent::Info {
                        message: format!("  {}Imported {} from {}", dry_prefix, remote_repo.name, forge_name),
                    };
                }

                // Persist to disk only on real runs
                if !is_dry_run && imported > 0 {
                    if let Err(e) = local.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                        };
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("  {} repos imported from remotes", imported),
            };

            // ── Phase 6: Diff ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 6/8: Computing diffs...", dry_prefix),
            };

            // Collect diffs for phase 7
            let mut all_diffs: Vec<(String, String, crate::services::SyncDiff)> = Vec::new();

            for (org_name, forge_name) in &pairs {
                let adapter = match make_adapter(forge_name, org_name) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::Error { message: e };
                        continue;
                    }
                };

                let local = state.get_local_forge(org_name).await;

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

                        if !diff.to_delete().is_empty() {
                            yield HyperforgeEvent::Info {
                                message: format!(
                                    "  ⚠ {} repos would be deleted on {} — skipped by sync (use 'workspace apply' to delete)",
                                    diff.to_delete().len(),
                                    forge_name,
                                ),
                            };
                        }

                        all_diffs.push((org_name.clone(), forge_name.clone(), diff));
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Diff failed for {}/{}: {}", org_name, forge_name, e),
                        };
                    }
                }
            }

            // ── Phase 7: Apply (safe — no deletes) ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 7/8: Applying creates and updates (no deletes)...", dry_prefix),
            };

            let mut total_created = 0usize;
            let mut total_updated = 0usize;

            for (org_name, forge_name, diff) in &all_diffs {
                let adapter = match make_adapter(forge_name, org_name) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::Error { message: e };
                        continue;
                    }
                };

                for repo_op in &diff.ops {
                    match repo_op.op {
                        SyncOp::Create => {
                            if !is_dry_run {
                                if let Err(e) = adapter.create_repo(org_name, &repo_op.repo).await {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Failed to create {} on {}: {}", repo_op.repo.name, forge_name, e),
                                    };
                                    continue;
                                }
                            }
                            total_created += 1;
                            yield HyperforgeEvent::SyncOp {
                                repo_name: repo_op.repo.name.clone(),
                                operation: "create".to_string(),
                                forge: forge_name.clone(),
                            };
                        }
                        SyncOp::Update => {
                            if !is_dry_run {
                                if let Err(e) = adapter.update_repo(org_name, &repo_op.repo).await {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Failed to update {} on {}: {}", repo_op.repo.name, forge_name, e),
                                    };
                                    continue;
                                }
                            }
                            total_updated += 1;
                            yield HyperforgeEvent::SyncOp {
                                repo_name: repo_op.repo.name.clone(),
                                operation: "update".to_string(),
                                forge: forge_name.clone(),
                            };
                        }
                        SyncOp::Delete => {
                            // Explicitly skip deletes in the safe pipeline
                            yield HyperforgeEvent::SyncOp {
                                repo_name: repo_op.repo.name.clone(),
                                operation: "skip_delete".to_string(),
                                forge: forge_name.clone(),
                            };
                        }
                        SyncOp::InSync => {
                            // Nothing to do
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {}{} created, {} updated on remotes",
                    dry_prefix, total_created, total_updated,
                ),
            };

            // ── Phase 8: Push git content ──
            if is_no_push {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 8/8: Push skipped (--no_push).", dry_prefix),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 8/8: Pushing {} repos...", dry_prefix, ctx.repos.len()),
                };

                let mut push_success = 0usize;
                let mut push_failed = 0usize;

                for repo in &ctx.repos {
                    if !repo.is_git_repo {
                        continue;
                    }

                    let mut options = PushOptions::new();
                    if is_dry_run {
                        options = options.dry_run();
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
                                push_success += 1;
                            } else {
                                push_failed += 1;
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
                            push_failed += 1;
                        }
                    }
                }

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {}{} pushed successfully, {} failed",
                        dry_prefix, push_success, push_failed,
                    ),
                };
            }

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!("{}Sync pipeline complete.", dry_prefix),
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

    /// Apply local configuration to a remote forge (creates, updates, AND DELETES)
    #[plexus_macros::hub_method(
        description = "Apply LocalForge state to remote forges. WARNING: this will DELETE repos on remotes that are not in LocalForge. Use 'workspace sync' for the safe pipeline. Use --path to discover from disk, or --org and --forge for direct registry access.",
        params(
            path = "Path to workspace directory (discovers orgs/forges from disk)",
            org = "Organization name (required if --path not provided)",
            forge = "Target forge: github, codeberg, or gitlab (required if --path not provided)",
            dry_run = "Preview changes without applying them (optional, default: false)"
        )
    )]
    pub async fn apply(
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
                    message: "No org/forge pairs found to apply.".to_string(),
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
                        message: format!("[DRY RUN] Computing apply operations for {}/{}...", org_name, forge_name),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("Applying {}/{}...", org_name, forge_name),
                    };
                }

                // Execute apply (creates, updates, AND deletes)
                match sync_service.sync(local, adapter, org_name, is_dry_run).await {
                    Ok(diff) => {
                        let created = diff.to_create().len();
                        let updated = diff.to_update().len();
                        let deleted = diff.to_delete().len();
                        let in_sync = diff.in_sync().len();

                        yield HyperforgeEvent::Info {
                            message: format!(
                                "{}{}/{} apply complete: {} created, {} updated, {} deleted, {} in sync",
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
                            message: format!("Apply failed for {}/{}: {}", org_name, forge_name, e),
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
