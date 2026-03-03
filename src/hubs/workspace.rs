//! WorkspaceHub — Forge/registry operations for multi-repo workspaces.
//!
//! Methods here read/write LocalForge and talk to forge APIs. Development tools
//! (manifest generation, publishing, cross-repo execution) live in [`super::build`].

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;
use std::path::PathBuf;

use chrono::Utc;

use crate::adapters::{ForgePort, ForgeSyncState};
use crate::commands::init::{init, InitOptions};
use crate::commands::push::{push, PushOptions};
use crate::commands::runner::{collect_push_results, discover_or_bail, run_batch, run_batch_blocking, run_diff_batch, run_validation_gate};
use crate::commands::workspace::{repo_from_config, DiscoveredRepo, WorkspaceContext};
use crate::config::HyperforgeConfig;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::HyperforgeState;
use crate::hubs::utils::{dry_prefix, glob_match, make_adapter, workspace_summary};
use crate::services::SyncOp;
use crate::types::Visibility;
use std::collections::HashSet;

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

#[plexus_macros::hub_methods(
    namespace = "workspace",
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
            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
            };

            yield HyperforgeEvent::Info {
                message: format!("Scanning workspace: {}", ctx.root.display()),
            };

            // Report each discovered repo
            for repo in &ctx.repos {
                let org = repo.org().unwrap_or("(none)");
                let forges = repo.forges().join(", ");
                let git_status = if repo.is_git_repo { "git" } else { "no-git" };
                let bs_label = format!("{}", repo.build_system);

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {} [{}] org={} forges=[{}] build=[{}]",
                        repo.dir_name, git_status, org, forges, bs_label
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

            // Report orgs, forges, and build systems
            yield HyperforgeEvent::Info {
                message: format!("Orgs: {}", ctx.orgs.join(", ")),
            };
            yield HyperforgeEvent::Info {
                message: format!("Forges: {}", ctx.forges.join(", ")),
            };
            let bs_list: Vec<String> = ctx.build_systems().iter().map(|bs| format!("{}", bs)).collect();
            if !bs_list.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("Build systems: {}", bs_list.join(", ")),
                };
            }

            yield workspace_summary(&ctx);
        }
    }

    /// Initialize unconfigured repos in a workspace
    #[plexus_macros::hub_method(
        description = "Initialize hyperforge config for unconfigured repos in a workspace directory. Discovers repos, infers org/forges from existing configs, and creates .hyperforge/config.toml for each unconfigured repo.",
        params(
            path = "Path to workspace directory",
            org = "Organization name (inferred from workspace if only one org exists)",
            forges = "Forges to configure (inferred from existing configs if not specified)",
            dry_run = "Preview without writing configs (optional, default: false)",
            force = "Re-init repos that already have config (optional, default: false)",
            no_hooks = "Skip installing pre-push hook (optional, default: false)",
            no_ssh_wrapper = "Skip configuring SSH wrapper (optional, default: false)"
        )
    )]
    pub async fn init(
        &self,
        path: String,
        org: Option<String>,
        forges: Option<Vec<String>>,
        dry_run: Option<bool>,
        force: Option<bool>,
        no_hooks: Option<bool>,
        no_ssh_wrapper: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);
        let is_force = force.unwrap_or(false);
        let is_no_hooks = no_hooks.unwrap_or(false);
        let is_no_ssh_wrapper = no_ssh_wrapper.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = dry_prefix(is_dry_run);

            // ── Phase 1: Discover ──
            yield HyperforgeEvent::Info {
                message: format!("{}Discovering workspace...", dry_prefix),
            };

            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
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

            if !ctx.skipped_dirs.is_empty() {
                for dir in &ctx.skipped_dirs {
                    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    yield HyperforgeEvent::Info {
                        message: format!("  {} [no git — needs git init]", name),
                    };
                }
            }

            // ── Infer org and forges ──
            let inferred_org: Option<String> = org.clone().or_else(|| {
                if ctx.orgs.len() == 1 {
                    Some(ctx.orgs[0].clone())
                } else {
                    None
                }
            });

            let inferred_forges: Vec<String> = forges.clone().unwrap_or_else(|| ctx.forges.clone());

            // Determine targets: unconfigured repos, plus configured repos if --force
            let mut targets: Vec<PathBuf> = ctx.unconfigured_repos.clone();
            if is_force {
                for repo in &ctx.repos {
                    targets.push(repo.path.clone());
                }
            }

            if targets.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No repos to initialize.".to_string(),
                };
                yield workspace_summary(&ctx);
                return;
            }

            if inferred_org.is_none() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init repos: multiple orgs found and --org not specified.".to_string(),
                };
                return;
            }
            if inferred_forges.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "Cannot init repos: no forges found and --forges not specified.".to_string(),
                };
                return;
            }

            // ── Phase 2: Init ──
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Initializing {} repos (org={}, forges=[{}])...",
                    dry_prefix,
                    targets.len(),
                    inferred_org.as_deref().unwrap_or("?"),
                    inferred_forges.join(", "),
                ),
            };

            let mut inits_performed = 0usize;
            let mut init_failed = 0usize;

            for repo_path in &targets {
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
                if is_force {
                    opts = opts.force();
                }
                if is_no_hooks {
                    opts = opts.no_hooks();
                }
                if is_no_ssh_wrapper {
                    opts = opts.no_ssh_wrapper();
                }

                match init(repo_path, opts) {
                    Ok(_report) => {
                        inits_performed += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("  {}Initialized {}", dry_prefix, dir_name),
                        };
                    }
                    Err(e) => {
                        init_failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to init {}: {}", dir_name, e),
                        };
                    }
                }
            }

            // ── Phase 3: Re-discover ──
            let ctx = if inits_performed > 0 && !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("Re-discovering after {} inits...", inits_performed),
                };
                match discover_or_bail(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(event) => { yield event; return; }
                }
            } else {
                ctx
            };

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Init complete: {} initialized, {} failed",
                    dry_prefix, inits_performed, init_failed,
                ),
            };

            yield workspace_summary(&ctx);
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

            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
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

            // Collect inputs for run_batch_blocking
            let check_inputs: Vec<_> = ctx.repos.iter()
                .filter(|r| r.is_git_repo)
                .map(|r| (r.dir_name.clone(), r.path.clone(), expected_branch.clone()))
                .collect();

            let results = run_batch_blocking(check_inputs, 8, |(dir_name, path, exp_branch)| {
                let current_branch = Git::current_branch(&path)
                    .map_err(|e| format!("{}: failed to get branch: {}", dir_name, e));
                let status = Git::repo_status(&path)
                    .map_err(|e| format!("{}: failed to get status: {}", dir_name, e));
                let ssh_cmd = Git::config_get(&path, "core.sshCommand").ok().flatten();
                let hf_org = Git::config_get(&path, "hyperforge.org").ok().flatten();
                (dir_name, path, exp_branch, current_branch, status, ssh_cmd, hf_org)
            }).await;

            for result in results {
                let (dir_name, path, exp_branch, current_branch, status, ssh_cmd, hf_org) = match result {
                    Ok(v) => v,
                    Err(e) => { yield HyperforgeEvent::Error { message: e }; continue; }
                };

                let current_branch = match current_branch {
                    Ok(b) => b,
                    Err(e) => { yield HyperforgeEvent::Error { message: e }; continue; }
                };
                let status = match status {
                    Ok(s) => s,
                    Err(e) => { yield HyperforgeEvent::Error { message: e }; continue; }
                };

                let is_clean = !status.has_changes && !status.has_staged && !status.has_untracked;
                let on_correct_branch = current_branch == exp_branch;

                if is_clean { clean_count += 1; } else { dirty_count += 1; }
                if !on_correct_branch { wrong_branch_count += 1; }

                yield HyperforgeEvent::RepoCheck {
                    repo_name: dir_name.clone(),
                    path: path.display().to_string(),
                    branch: current_branch,
                    expected_branch: exp_branch,
                    is_clean,
                    on_correct_branch,
                };

                if ssh_cmd.as_deref() == Some("hyperforge-ssh") && hf_org.is_none() {
                    yield HyperforgeEvent::Error {
                        message: format!("{}: SSH misconfigured — hyperforge-ssh set but hyperforge.org missing", dir_name),
                    };
                }
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
                validation_passed: None,
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
            set_upstream = "Set upstream tracking (optional, default: false)",
            validate = "Run containerized validation before pushing (optional, default: false)"
        )
    )]
    pub async fn push_all(
        &self,
        path: String,
        branch: Option<String>,
        dry_run: Option<bool>,
        set_upstream: Option<bool>,
        validate: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let _ = branch; // push() uses current branch; param is informational
        stream! {
            let workspace_path = PathBuf::from(&path);
            let is_dry_run = dry_run.unwrap_or(false);
            let is_set_upstream = set_upstream.unwrap_or(false);
            let is_validate = validate.unwrap_or(false);

            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
            };

            // Validation gate (if --validate)
            if is_validate {
                yield HyperforgeEvent::Info {
                    message: "Validation: Running containerized build check...".to_string(),
                };

                let gate = run_validation_gate(&ctx.repos, &ctx.root, is_dry_run);
                for event in gate.events {
                    yield event;
                }
                if gate.passed == Some(false) {
                    return;
                }
            }

            yield HyperforgeEvent::Info {
                message: format!("{}Pushing {} repos...", dry_prefix(is_dry_run), ctx.repos.len()),
            };

            // Skip non-git repos
            for repo in ctx.repos.iter().filter(|r| !r.is_git_repo) {
                yield HyperforgeEvent::Info {
                    message: format!("  Skipping {} (not a git repo)", repo.dir_name),
                };
            }

            // Parallel push via run_batch_blocking
            let push_inputs: Vec<_> = ctx.repos.iter()
                .filter(|r| r.is_git_repo)
                .map(|r| {
                    let mut options = PushOptions::new();
                    if is_dry_run { options = options.dry_run(); }
                    if is_set_upstream { options = options.set_upstream(); }
                    (r.dir_name.clone(), r.path.clone(), options)
                })
                .collect();

            let push_results = run_batch_blocking(push_inputs, 8, |(dir_name, path, options)| {
                let result = push(&path, options);
                (dir_name, path, result)
            }).await;

            let batch = collect_push_results(push_results);
            for event in batch.events {
                yield event;
            }

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
                configured_repos: ctx.repos.len(),
                unconfigured_repos: ctx.unconfigured_repos.len(),
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: Some(batch.success_count),
                push_failed: Some(batch.failed_count),
                validation_passed: None,
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
                let ctx = match discover_or_bail(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(event) => { yield event; return; }
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

            // Parallel diff: spawn per org/forge pair
            {
                let results = run_diff_batch(&pairs, &state, &sync_service).await;

                for result in results {
                    let entry = match result {
                        Ok(v) => v,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                            continue;
                        }
                    };

                    yield HyperforgeEvent::Info {
                        message: format!("Computing diff for {}/{}", entry.org_name, entry.forge_name),
                    };

                    match entry.diff_result {
                        Ok(diff) => {
                            yield HyperforgeEvent::SyncSummary {
                                forge: entry.forge_name.clone(),
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
                                    forge: entry.forge_name.clone(),
                                };
                            }
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                        }
                    }
                }
            }
        }
    }

    /// Full safe sync pipeline: discover → init → register → import → diff → apply (no deletes) → push
    #[plexus_macros::hub_method(
        description = "Sync workspace to remote forges. With --reflect, retire remote-only repos. With --purge, delete previously staged repos.",
        params(
            path = "Path to workspace directory (required)",
            org = "Organization name (inferred from workspace if only one org exists)",
            forges = "Forges for unconfigured repos (inferred from existing configs if not specified)",
            dry_run = "Preview all phases without making changes (optional, default: false)",
            no_push = "Skip the git push phase (optional, default: false)",
            no_init = "Skip initializing unconfigured repos (optional, default: false)",
            validate = "Run containerized validation before pushing (optional, default: false)",
            reflect = "Enable reflect mode: retire remote-only repos (optional, default: false)",
            purge = "Delete repos previously staged for deletion. Implies --reflect (optional, default: false)"
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
        validate: Option<bool>,
        reflect: Option<bool>,
        purge: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let sync_service = self.state.sync_service.clone();
        let is_dry_run = dry_run.unwrap_or(false);
        let is_no_push = no_push.unwrap_or(false);
        let is_no_init = no_init.unwrap_or(false);
        let is_validate = validate.unwrap_or(false);
        let is_purge = purge.unwrap_or(false);
        let is_reflect = reflect.unwrap_or(false) || is_purge;

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = dry_prefix(is_dry_run);

            // ── Phase 1: Discover ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 1/8: Discovering workspace...", dry_prefix),
            };

            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
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
            let inits_performed;

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

                let (events, count) = sync_init_unconfigured(
                    &ctx.unconfigured_repos, &inferred_org, &inferred_forges, is_dry_run, dry_prefix,
                );
                inits_performed = count;
                for event in events { yield event; }
            } else {
                inits_performed = 0;
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 2/8: Init skipped.", dry_prefix),
                };
            }

            // ── Phase 3: Re-discover ──
            let ctx = if inits_performed > 0 && !is_dry_run {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 3/8: Re-discovering after {} inits...", dry_prefix, inits_performed),
                };
                match discover_or_bail(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(event) => { yield event; return; }
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

            let (events, registered, already_registered, unstaged) =
                sync_register_repos(&ctx.repos, &ctx.orgs, &state, is_reflect, is_dry_run, dry_prefix).await;
            for event in events { yield event; }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {} newly registered, {} already in LocalForge{}",
                    registered, already_registered,
                    if unstaged > 0 { format!(", {} unstaged", unstaged) } else { String::new() },
                ),
            };

            // ── Phase 5: Import remote-only repos into LocalForge (ETag-based) ──
            let pairs = ctx.org_forge_pairs();

            if !is_reflect {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 5/8: Importing remote-only repos for {} org/forge pairs...", dry_prefix, pairs.len()),
                };

                let events = sync_import_remote(&pairs, &ctx.orgs, &state, is_dry_run, dry_prefix).await;
                for event in events { yield event; }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 5/8: Import skipped (reflect mode).", dry_prefix),
                };
            }

            // ── Phase 6: Diff ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 6/8: Computing diffs...", dry_prefix),
            };

            // Collect diffs for phase 7 (parallel)
            let mut all_diffs: Vec<(String, String, crate::services::SyncDiff)> = Vec::new();

            {
                let results = run_diff_batch(&pairs, &state, &sync_service).await;

                for result in results {
                    let entry = match result {
                        Ok(v) => v,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                            continue;
                        }
                    };

                    match entry.diff_result {
                        Ok(diff) => {
                            yield HyperforgeEvent::SyncSummary {
                                forge: entry.forge_name.clone(),
                                total: diff.ops.len(),
                                to_create: diff.to_create().len(),
                                to_update: diff.to_update().len(),
                                to_delete: diff.to_delete().len(),
                                in_sync: diff.in_sync().len(),
                            };

                            if !diff.to_delete().is_empty() {
                                yield HyperforgeEvent::Info {
                                    message: format!(
                                        "  {} repos would be deleted on {} — skipped by sync (use 'workspace apply' to delete)",
                                        diff.to_delete().len(),
                                        entry.forge_name,
                                    ),
                                };
                            }

                            all_diffs.push((entry.org_name, entry.forge_name, diff));
                        }
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: e };
                        }
                    }
                }
            }

            // ── Phase 7: Apply (safe — no deletes) ──
            yield HyperforgeEvent::Info {
                message: format!("{}Phase 7/8: Applying creates and updates (no deletes)...", dry_prefix),
            };

            let (events, total_created, total_updated) = sync_apply_diffs(all_diffs, &state, is_dry_run).await;
            for event in events { yield event; }

            yield HyperforgeEvent::Info {
                message: format!(
                    "  {}{} created, {} updated on remotes",
                    dry_prefix, total_created, total_updated,
                ),
            };

            // ── Phase 7.5: Retire remote-only repos (reflect mode) ──
            if is_reflect {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "{}Retire: {}retiring remote-only repos...",
                        dry_prefix,
                        if is_purge { "Purging previously " } else { "Staging and " },
                    ),
                };

                let (events, staged_count, purged_count, protected_skipped) =
                    sync_retire_remote_only(&pairs, &ctx, &state, is_dry_run, is_purge).await;
                for event in events { yield event; }

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {}{} staged, {} purged, {} protected (skipped)",
                        dry_prefix, staged_count, purged_count, protected_skipped,
                    ),
                };
            }

            // ── Validation gate (if --validate) ──
            let validation_passed_result: Option<bool> = if is_validate {
                yield HyperforgeEvent::Info {
                    message: format!("{}Validation: Running containerized build check...", dry_prefix),
                };

                let gate = run_validation_gate(&ctx.repos, &ctx.root, is_dry_run);
                for event in gate.events {
                    yield event;
                }
                gate.passed
            } else {
                None
            };

            // ── Phase 8: Push git content ──
            let skip_push_for_validation = validation_passed_result == Some(false);
            if is_no_push || skip_push_for_validation {
                if skip_push_for_validation {
                    yield HyperforgeEvent::Info {
                        message: format!("{}Phase 8/8: Push skipped (validation failed).", dry_prefix),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("{}Phase 8/8: Push skipped (--no_push).", dry_prefix),
                    };
                }
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("{}Phase 8/8: Pushing {} repos...", dry_prefix, ctx.repos.len()),
                };

                // Parallel push: spawn_blocking per repo
                let push_inputs: Vec<_> = ctx.repos.iter()
                    .filter(|r| r.is_git_repo)
                    .map(|repo| {
                        let dir_name = repo.dir_name.clone();
                        let path = repo.path.clone();
                        let mut options = PushOptions::new();
                        if is_dry_run { options = options.dry_run(); }
                        (dir_name, path, options)
                    })
                    .collect();

                let push_results = run_batch_blocking(push_inputs, 8, |(dir_name, path, options)| {
                    let result = push(&path, options);
                    (dir_name, path, result)
                }).await;

                let batch = collect_push_results(push_results);
                for event in batch.events {
                    yield event;
                }

                yield HyperforgeEvent::Info {
                    message: format!(
                        "  {}{} pushed successfully, {} failed",
                        dry_prefix, batch.success_count, batch.failed_count,
                    ),
                };
            }

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!("{}{}pipeline complete.", dry_prefix,
                    if is_reflect { "Reflect " } else { "Sync " }),
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
                validation_passed: validation_passed_result,
            };
        }
    }

    /// Set default branch on all repos in a workspace
    #[plexus_macros::hub_method(
        description = "Set the default branch on all remote forges for every repo in a workspace, and optionally git checkout locally",
        params(
            path = "Path to workspace directory",
            branch = "Branch to set as default",
            checkout = "Also run git checkout locally in each repo (optional, default: false)",
            dry_run = "Preview changes without applying (optional, default: false)"
        )
    )]
    pub async fn set_default_branch(
        &self,
        path: String,
        branch: String,
        checkout: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let is_dry_run = dry_run.unwrap_or(false);
        let is_checkout = checkout.unwrap_or(false);

        stream! {
            let workspace_path = PathBuf::from(&path);
            let dry_prefix = dry_prefix(is_dry_run);

            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Setting default branch to '{}' for {} repos...",
                    dry_prefix, branch, ctx.repos.len(),
                ),
            };

            let mut success_count = 0usize;
            let mut error_count = 0usize;

            // Parallel set_default_branch: spawn per repo
            {
                // Filter repos that have config and org
                let eligible: Vec<_> = ctx.repos.iter()
                    .filter(|r| r.config.is_some() && r.org().is_some())
                    .cloned()
                    .collect();

                // Report repos without org
                for repo in &ctx.repos {
                    if repo.config.is_some() && repo.org().is_none() {
                        yield HyperforgeEvent::Error {
                            message: format!("  {}: no org configured, skipping", repo.dir_name),
                        };
                        error_count += 1;
                    }
                }

                let items: Vec<_> = eligible.into_iter().map(|repo| {
                    let config = repo.config.clone().unwrap();
                    let org = repo.org().unwrap().to_string();
                    let repo_name = config.repo_name.clone()
                        .unwrap_or_else(|| repo.dir_name.clone());
                    let dir_name = repo.dir_name.clone();
                    let forges: Vec<String> = repo.forges().into_iter().map(|s| s.to_string()).collect();
                    let branch = branch.clone();
                    let dry_prefix = dry_prefix.to_string();
                    let path = repo.path.clone();
                    let is_git = repo.is_git_repo;
                    (dir_name, org, repo_name, forges, branch, dry_prefix, path, is_git)
                }).collect();

                let dry_run = is_dry_run;
                let checkout = is_checkout;
                let results = run_batch(items, 8, move |(dir_name, org, repo_name, forges, branch, dry_prefix, path, is_git)| async move {
                    let mut events = Vec::new();
                    let mut errors = Vec::new();

                    for forge_name in &forges {
                        if dry_run {
                            events.push(HyperforgeEvent::Info {
                                message: format!(
                                    "  {}Would set default branch on {}/{} ({})",
                                    dry_prefix, repo_name, forge_name, branch,
                                ),
                            });
                            continue;
                        }

                        let adapter = match make_adapter(forge_name, &org, None) {
                            Ok(a) => a,
                            Err(e) => {
                                errors.push(format!("{}: {}", forge_name, e));
                                continue;
                            }
                        };

                        match adapter.set_default_branch(&org, &repo_name, &branch).await {
                            Ok(_) => {
                                events.push(HyperforgeEvent::Info {
                                    message: format!("  {} → {} default branch set to '{}'", repo_name, forge_name, branch),
                                });
                            }
                            Err(e) => {
                                errors.push(format!("{}: {}", forge_name, e));
                            }
                        }
                    }

                    // Optionally checkout locally (sync git op)
                    if checkout && is_git {
                        if dry_run {
                            events.push(HyperforgeEvent::Info {
                                message: format!("  {}Would checkout '{}' in {}", dry_prefix, branch, dir_name),
                            });
                        } else {
                            match tokio::task::spawn_blocking({
                                let path = path.clone();
                                let branch = branch.clone();
                                move || Git::checkout(&path, &branch)
                            }).await {
                                Ok(Ok(_)) => {
                                    events.push(HyperforgeEvent::Info {
                                        message: format!("  {} → checked out '{}'", dir_name, branch),
                                    });
                                }
                                Ok(Err(e)) => {
                                    errors.push(format!("checkout: {}", e));
                                }
                                Err(e) => {
                                    errors.push(format!("checkout spawn error: {}", e));
                                }
                            }
                        }
                    }

                    (dir_name, events, errors)
                }).await;

                for result in results {
                    let (dir_name, events, errors) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            yield HyperforgeEvent::Error { message: format!("Task join error: {}", e) };
                            error_count += 1;
                            continue;
                        }
                    };

                    for event in events {
                        yield event;
                    }

                    if errors.is_empty() {
                        success_count += 1;
                    } else {
                        error_count += 1;
                        for err in &errors {
                            yield HyperforgeEvent::Error {
                                message: format!("  {} error: {}", dir_name, err),
                            };
                        }
                    }
                }
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Set default branch complete: {} succeeded, {} failed",
                    dry_prefix, success_count, error_count,
                ),
            };
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
                let ctx = match discover_or_bail(&workspace_path) {
                    Ok(ctx) => ctx,
                    Err(event) => { yield event; return; }
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


    /// Clone all repos for an org from LocalForge into a workspace directory
    #[plexus_macros::hub_method(
        description = "Clone all repos for an org from LocalForge into a workspace directory. Skips repos already on disk.",
        params(
            org = "Organization name (must have repos in LocalForge)",
            path = "Target workspace directory",
            filter = "Filter repos by name glob (optional, e.g. 'plexus-*')",
            forge = "Preferred forge to clone from (optional, defaults to first in present_on)",
            concurrency = "Max parallel clones (optional, default: 4)"
        )
    )]
    pub async fn clone(
        &self,
        org: String,
        path: String,
        filter: Option<String>,
        forge: Option<String>,
        concurrency: Option<u32>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let max_concurrent = concurrency.unwrap_or(4) as usize;

        stream! {
            let workspace_path = PathBuf::from(&path);

            // 1. Load LocalForge
            let local = state.get_local_forge(&org).await;

            // 2. Get all records
            let records = match local.all_records() {
                Ok(r) => r,
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to load repos: {}", e),
                    };
                    return;
                }
            };

            if records.is_empty() {
                yield HyperforgeEvent::Error {
                    message: format!("No repos found in LocalForge for org '{}'", org),
                };
                return;
            }

            // 3. Filter by glob
            let filtered: Vec<_> = if let Some(ref pattern) = filter {
                records.into_iter().filter(|r| glob_match(pattern, &r.name)).collect()
            } else {
                records
            };

            // 4. Create workspace dir if needed
            if let Err(e) = std::fs::create_dir_all(&workspace_path) {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to create workspace directory: {}", e),
                };
                return;
            }

            // 5. Skip repos already on disk
            let total_filtered = filtered.len();
            let to_clone: Vec<_> = filtered.into_iter().filter(|r| {
                !workspace_path.join(&r.name).exists()
            }).collect();

            let skipped_count = total_filtered - to_clone.len();

            if to_clone.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!(
                        "All {} repos already exist on disk. Nothing to clone.",
                        total_filtered,
                    ),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "Cloning {} repos (skipping {} already on disk, concurrency: {})...",
                    to_clone.len(), skipped_count, max_concurrent,
                ),
            };

            // Validate forge preference if provided
            if let Some(ref f) = forge {
                if HyperforgeConfig::parse_forge(f).is_none() {
                    yield HyperforgeEvent::Error {
                        message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", f),
                    };
                    return;
                }
            }

            // 6. Clone in batches using run_batch
            let mut success_count = 0usize;
            let mut failed_count = 0usize;

            let clone_inputs: Vec<_> = to_clone.into_iter()
                .map(|r| (r, org.clone(), forge.clone(), workspace_path.clone()))
                .collect();

            let clone_results = crate::commands::runner::run_batch(
                clone_inputs,
                max_concurrent,
                |(record, org, forge_pref, ws_path)| async move {
                    // Pick forge to clone from
                    let clone_forge = if let Some(ref f) = forge_pref {
                        f.to_lowercase()
                    } else {
                        match record.present_on.iter().next() {
                            Some(f) => format!("{:?}", f).to_lowercase(),
                            None => {
                                return (record.name.clone(), Err("No forges in present_on".to_string()));
                            }
                        }
                    };

                    let clone_url = crate::git::build_remote_url(&clone_forge, &org, &record.name);
                    let target = ws_path.join(&record.name);
                    let target_str = target.display().to_string();

                    // Clone
                    if let Err(e) = crate::git::Git::clone(&clone_url, &target_str) {
                        return (record.name.clone(), Err(format!("Clone failed: {}", e)));
                    }

                    // Materialize config + remotes from the record
                    let opts = crate::commands::materialize::MaterializeOpts::default();
                    if let Err(e) = crate::commands::materialize::materialize(&org, &record, &target, opts) {
                        return (record.name.clone(), Err(format!("Cloned but materialize failed: {}", e)));
                    }

                    (record.name.clone(), Ok(()))
                },
            ).await;

            for result in clone_results {
                match result {
                    Ok((name, Ok(()))) => {
                        success_count += 1;
                        yield HyperforgeEvent::Info {
                            message: format!("  Cloned: {}", name),
                        };
                    }
                    Ok((name, Err(e))) => {
                        failed_count += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed: {} — {}", name, e),
                        };
                    }
                    Err(e) => {
                        failed_count += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("  Task error: {}", e),
                        };
                    }
                }
            }

            // Summary
            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: success_count + failed_count + skipped_count,
                configured_repos: success_count,
                unconfigured_repos: 0,
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: Some(success_count),
                push_failed: Some(failed_count),
                validation_passed: None,
            };
        }
    }


    /// Move repos from one workspace to another
    #[plexus_macros::hub_method(
        description = "Move repos from one workspace to another, updating config, git remotes, and LocalForge registry",
        params(
            path = "Source workspace directory",
            target_path = "Target workspace directory",
            target_org = "Target organization name (optional — inferred from target workspace if unambiguous)",
            repo = "Repo names to move (optional, repeat: --repo a --repo b)",
            filter = "Glob pattern to select repos (optional, e.g. 'orcha*')",
            dry_run = "Preview without making changes (optional, default: false)"
        )
    )]
    pub async fn move_repos(
        &self,
        path: String,
        target_path: String,
        target_org: Option<String>,
        repo: Option<Vec<String>>,
        filter: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            let dry_prefix = dry_prefix(is_dry_run);
            let source_path = PathBuf::from(&path);
            let dest_path = PathBuf::from(&target_path);

            // ── Discover both workspaces ──
            let ctx = match discover_or_bail(&source_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
            };
            let target_ctx = match discover_or_bail(&dest_path) {
                Ok(ctx) => ctx,
                Err(event) => { yield event; return; }
            };

            // ── Infer target_org if not provided ──
            let target_org: String = match target_org {
                Some(org) => org,
                None => {
                    match target_ctx.orgs.len() {
                        0 => {
                            yield HyperforgeEvent::Error {
                                message: "Target workspace has no configured orgs, specify --target-org".to_string(),
                            };
                            return;
                        }
                        1 => {
                            let org = target_ctx.orgs[0].clone();
                            yield HyperforgeEvent::Info {
                                message: format!("{}Inferred target org: {}", dry_prefix, org),
                            };
                            org
                        }
                        _ => {
                            yield HyperforgeEvent::Error {
                                message: format!(
                                    "Target workspace has multiple orgs ({}), specify --target-org",
                                    target_ctx.orgs.join(", "),
                                ),
                            };
                            return;
                        }
                    }
                }
            };

            // ── Build repo list from filter and/or explicit names ──
            if repo.is_none() && filter.is_none() {
                yield HyperforgeEvent::Error {
                    message: "No repos specified. Use --repo <name> and/or --filter <glob>.".to_string(),
                };
                return;
            }

            // Build a lookup of discovered repos by dir_name
            let mut discovered_map: std::collections::HashMap<String, &crate::commands::workspace::DiscoveredRepo> =
                std::collections::HashMap::new();
            for dr in &ctx.repos {
                discovered_map.insert(dr.dir_name.clone(), dr);
            }
            // Also check unconfigured repos (they have no DiscoveredRepo, just paths)
            let unconfigured_names: HashSet<String> = ctx.unconfigured_repos.iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .collect();

            // All known source names (configured + unconfigured)
            let all_source_names: Vec<String> = discovered_map.keys()
                .chain(unconfigured_names.iter())
                .cloned()
                .collect();

            // Collect from filter
            let mut selected: HashSet<String> = HashSet::new();
            if let Some(ref pattern) = filter {
                for name in &all_source_names {
                    if glob_match(pattern, name) {
                        selected.insert(name.clone());
                    }
                }
                if selected.is_empty() {
                    yield HyperforgeEvent::Error {
                        message: format!("Filter '{}' matched no repos in source workspace", pattern),
                    };
                    return;
                }
            }

            // Collect from explicit names
            if let Some(ref names) = repo {
                for name in names {
                    selected.insert(name.clone());
                }
            }

            // Stable sort for deterministic output
            let repo: Vec<String> = {
                let mut v: Vec<String> = selected.into_iter().collect();
                v.sort();
                v
            };

            // Build target lookup for collision detection
            let target_repo_names: HashSet<String> = target_ctx.repos.iter()
                .map(|r| r.dir_name.clone())
                .chain(
                    target_ctx.unconfigured_repos.iter()
                        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                )
                .collect();

            // Validate all requested repos exist in source and don't collide in target
            for name in &repo {
                if !discovered_map.contains_key(name.as_str()) && !unconfigured_names.contains(name.as_str()) {
                    yield HyperforgeEvent::Error {
                        message: format!("Repo '{}' not found in source workspace {}", name, source_path.display()),
                    };
                    return;
                }
                // Check target doesn't already have it (discovered repos OR directory on disk)
                if target_repo_names.contains(name.as_str()) {
                    yield HyperforgeEvent::Error {
                        message: format!("Target workspace already contains repo '{}'", name),
                    };
                    return;
                }
                let target_repo_path = dest_path.join(name);
                if target_repo_path.exists() {
                    yield HyperforgeEvent::Error {
                        message: format!("Target already contains '{}': {}", name, target_repo_path.display()),
                    };
                    return;
                }
            }

            let filter_info = match &filter {
                Some(pattern) => format!(" (filter: {})", pattern),
                None => String::new(),
            };
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Moving {} repo(s) from {} to {} (target org: {}){}\n{}",
                    dry_prefix, repo.len(), source_path.display(), dest_path.display(), target_org,
                    filter_info,
                    repo.iter().map(|n| format!("  - {}", n)).collect::<Vec<_>>().join("\n"),
                ),
            };

            let mut moved = 0usize;
            let mut failed = 0usize;

            for name in &repo {
                let repo_path = source_path.join(name);
                let target_repo_path = dest_path.join(name);
                let discovered = discovered_map.get(name.as_str()).copied();
                let source_org = discovered.and_then(|d| d.org().map(|s| s.to_string()));

                yield HyperforgeEvent::Info {
                    message: format!("{}── {} ──", dry_prefix, name),
                };

                // ── Step 1+2: Materialize config + remotes ──
                // Build a RepoRecord for the target org, then materialize
                let mut record = if let Some(ref src_org) = source_org {
                    let source_forge = state.get_local_forge(src_org).await;
                    source_forge.get_record(name).unwrap_or_else(|_| {
                        crate::types::RepoRecord::from_repo(
                            &crate::types::Repo::new(name.clone(), crate::types::Forge::GitHub),
                        )
                    })
                } else {
                    crate::types::RepoRecord::from_repo(
                        &crate::types::Repo::new(name.clone(), crate::types::Forge::GitHub),
                    )
                };

                // Absorb per-repo config if available
                if let Some(dr) = discovered {
                    if let Some(ref config) = dr.config {
                        record.merge_from_config(config);
                    }
                }
                record.local_path = Some(repo_path.clone());

                let materialize_opts = crate::commands::materialize::MaterializeOpts {
                    config: true,
                    remotes: true,
                    hooks: false,
                    ssh_wrapper: false,
                    dry_run: is_dry_run,
                };

                match crate::commands::materialize::materialize(&target_org, &record, &repo_path, materialize_opts) {
                    Ok(report) => {
                        let action = if report.config_written { "updated config" } else { "config unchanged" };
                        yield HyperforgeEvent::RepoMove {
                            repo_name: name.clone(),
                            step: "config".to_string(),
                            success: true,
                            message: format!("{}{}", dry_prefix, action),
                        };
                        for remote in &report.remotes_updated {
                            yield HyperforgeEvent::RepoMove {
                                repo_name: name.clone(),
                                step: "remotes".to_string(),
                                success: true,
                                message: format!("{}updated remote: {}", dry_prefix, remote),
                            };
                        }
                        for remote in &report.remotes_added {
                            yield HyperforgeEvent::RepoMove {
                                repo_name: name.clone(),
                                step: "remotes".to_string(),
                                success: true,
                                message: format!("{}added remote: {}", dry_prefix, remote),
                            };
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::RepoMove {
                            repo_name: name.clone(),
                            step: "config".to_string(),
                            success: false,
                            message: format!("Materialize failed: {}", e),
                        };
                        failed += 1;
                        continue;
                    }
                }

                // ── Step 3: Update LocalForge registry ──
                if let Some(ref src_org) = source_org {
                    let source_forge = state.get_local_forge(src_org).await;
                    let target_forge = state.get_local_forge(&target_org).await;

                    // Copy record to target, then remove from source
                    let record_opt = source_forge.all_records().ok().and_then(|records| {
                        records.into_iter().find(|r| r.name == *name)
                    });

                    if let Some(record) = record_opt {
                        if !is_dry_run {
                            if let Err(e) = target_forge.upsert_record(record) {
                                yield HyperforgeEvent::RepoMove {
                                    repo_name: name.clone(),
                                    step: "registry".to_string(),
                                    success: false,
                                    message: format!("Failed to add to target LocalForge: {}", e),
                                };
                            } else if let Err(e) = source_forge.remove_repo(name) {
                                yield HyperforgeEvent::RepoMove {
                                    repo_name: name.clone(),
                                    step: "registry".to_string(),
                                    success: false,
                                    message: format!("Added to target but failed to remove from source LocalForge: {}", e),
                                };
                            } else {
                                yield HyperforgeEvent::RepoMove {
                                    repo_name: name.clone(),
                                    step: "registry".to_string(),
                                    success: true,
                                    message: format!("{}moved {} → {} in LocalForge", dry_prefix, src_org, target_org),
                                };
                            }
                        } else {
                            yield HyperforgeEvent::RepoMove {
                                repo_name: name.clone(),
                                step: "registry".to_string(),
                                success: true,
                                message: format!("{}would move {} → {} in LocalForge", dry_prefix, src_org, target_org),
                            };
                        }
                    } else {
                        yield HyperforgeEvent::RepoMove {
                            repo_name: name.clone(),
                            step: "registry".to_string(),
                            success: true,
                            message: format!("{}not in source LocalForge, skipping registry", dry_prefix),
                        };
                    }
                } else {
                    yield HyperforgeEvent::RepoMove {
                        repo_name: name.clone(),
                        step: "registry".to_string(),
                        success: true,
                        message: format!("{}no source org, skipping registry", dry_prefix),
                    };
                }

                // ── Step 4: Move directory ──
                if !is_dry_run {
                    match tokio::fs::rename(&repo_path, &target_repo_path).await {
                        Ok(_) => {
                            yield HyperforgeEvent::RepoMove {
                                repo_name: name.clone(),
                                step: "directory".to_string(),
                                success: true,
                                message: format!("moved to {}", target_repo_path.display()),
                            };
                            moved += 1;
                        }
                        Err(e) => {
                            yield HyperforgeEvent::RepoMove {
                                repo_name: name.clone(),
                                step: "directory".to_string(),
                                success: false,
                                message: format!("Failed to move directory: {}", e),
                            };
                            failed += 1;
                        }
                    }
                } else {
                    yield HyperforgeEvent::RepoMove {
                        repo_name: name.clone(),
                        step: "directory".to_string(),
                        success: true,
                        message: format!("{}would move to {}", dry_prefix, target_repo_path.display()),
                    };
                    moved += 1;
                }
            }

            // ── Save LocalForge state ──
            if !is_dry_run {
                // Collect all source orgs that were affected
                let source_orgs: HashSet<String> = repo.iter()
                    .filter_map(|name| discovered_map.get(name.as_str()).and_then(|d| d.org().map(|s| s.to_string())))
                    .collect();

                for src_org in &source_orgs {
                    let forge = state.get_local_forge(src_org).await;
                    if let Err(e) = forge.save_to_yaml().await {
                        yield HyperforgeEvent::Error {
                            message: format!("Failed to save source LocalForge for {}: {}", src_org, e),
                        };
                    }
                }
                let target_forge = state.get_local_forge(&target_org).await;
                if let Err(e) = target_forge.save_to_yaml().await {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to save target LocalForge for {}: {}", target_org, e),
                    };
                }
            }

            // ── Summary ──
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Move complete: {} moved, {} failed (of {} requested)",
                    dry_prefix, moved, failed, repo.len(),
                ),
            };

            yield HyperforgeEvent::WorkspaceSummary {
                total_repos: repo.len(),
                configured_repos: moved,
                unconfigured_repos: failed,
                clean_repos: None,
                dirty_repos: None,
                wrong_branch_repos: None,
                push_success: None,
                push_failed: None,
                validation_passed: None,
            };
        }
    }
}

// ── Sync phase helpers (private) ──────────────────────────────────────────

/// Phase 2: Initialize unconfigured repos.
fn sync_init_unconfigured(
    unconfigured_repos: &[PathBuf],
    inferred_org: &Option<String>,
    inferred_forges: &[String],
    is_dry_run: bool,
    dry_prefix: &str,
) -> (Vec<HyperforgeEvent>, usize) {
    let mut events = Vec::new();
    let mut inits_performed = 0usize;

    for repo_path in unconfigured_repos {
        let dir_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");

        let mut opts = InitOptions::new(inferred_forges.to_vec());
        if let Some(ref o) = inferred_org {
            opts = opts.with_org(o.as_str());
        }
        if is_dry_run {
            opts = opts.dry_run();
        }

        match init(repo_path, opts) {
            Ok(_) => {
                inits_performed += 1;
                events.push(HyperforgeEvent::Info {
                    message: format!("  {}Initialized {}", dry_prefix, dir_name),
                });
            }
            Err(e) => {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to init {}: {}", dir_name, e),
                });
            }
        }
    }

    (events, inits_performed)
}

/// Phase 4: Register configured repos in LocalForge, merging config-first fields.
async fn sync_register_repos(
    repos: &[DiscoveredRepo],
    orgs: &[String],
    state: &HyperforgeState,
    is_reflect: bool,
    is_dry_run: bool,
    dry_prefix: &str,
) -> (Vec<HyperforgeEvent>, usize, usize, usize) {
    let mut events = Vec::new();
    let mut registered = 0usize;
    let mut already_registered = 0usize;
    let mut unstaged = 0usize;

    for discovered in repos {
        let repo = match repo_from_config(discovered) {
            Some(r) => r,
            None => continue,
        };
        let repo_org = match discovered.org() {
            Some(o) => o.to_string(),
            None => continue,
        };

        let local = state.get_local_forge(&repo_org).await;

        match local.repo_exists(&repo_org, &repo.name).await {
            Ok(true) => {
                if is_reflect {
                    match local.get_repo(&repo_org, &repo.name).await {
                        Ok(existing) if existing.staged_for_deletion => {
                            let mut updated = existing.clone();
                            updated.staged_for_deletion = false;
                            if let Err(e) = local.update_repo(&repo_org, &updated).await {
                                events.push(HyperforgeEvent::Error {
                                    message: format!("  Failed to unstage {}: {}", repo.name, e),
                                });
                            } else {
                                unstaged += 1;
                                events.push(HyperforgeEvent::Info {
                                    message: format!("  {}Unstaged {} (found locally)", dry_prefix, repo.name),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                already_registered += 1;
                if let Ok(mut record) = local.get_record(&repo.name) {
                    record.managed = true;
                    record.local_path = Some(discovered.path.clone());
                    if let Some(ref config) = discovered.config {
                        record.merge_from_config(config);
                    }
                    let _ = local.update_record(&record);
                }
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to check {}: {}", repo.name, e),
                });
                continue;
            }
        }

        // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
        if let Err(e) = local.create_repo(&repo_org, &repo).await {
            events.push(HyperforgeEvent::Error {
                message: format!("  Failed to register {}: {}", repo.name, e),
            });
            continue;
        }

        if let Ok(mut record) = local.get_record(&repo.name) {
            record.managed = true;
            record.local_path = Some(discovered.path.clone());
            if let Some(ref config) = discovered.config {
                record.merge_from_config(config);
            }
            let _ = local.update_record(&record);
        }

        registered += 1;
        events.push(HyperforgeEvent::Info {
            message: format!("  {}Registered {}", dry_prefix, repo.name),
        });
    }

    // Persist to disk only on real runs
    if !is_dry_run && (registered > 0 || unstaged > 0) {
        for org_name in orgs {
            let local = state.get_local_forge(org_name).await;
            if let Err(e) = local.save_to_yaml().await {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                });
            }
        }
    }

    (events, registered, already_registered, unstaged)
}

/// Phase 5: Import remote-only repos into LocalForge (ETag-based) + report unmanaged.
async fn sync_import_remote(
    pairs: &[(String, String)],
    orgs: &[String],
    state: &HyperforgeState,
    is_dry_run: bool,
    dry_prefix: &str,
) -> Vec<HyperforgeEvent> {
    let mut events = Vec::new();
    let mut imported = 0usize;

    for (org_name, forge_name) in pairs {
        let local = state.get_local_forge(org_name).await;
        let ot = local.owner_type();

        let adapter = match make_adapter(forge_name, org_name, ot) {
            Ok(a) => a,
            Err(e) => {
                events.push(HyperforgeEvent::Error { message: e });
                continue;
            }
        };

        let forge_enum = HyperforgeConfig::parse_forge(forge_name);
        let stored_etag = if let Some(ref fe) = forge_enum {
            local.forge_states().ok()
                .and_then(|states| states.get(fe).map(|s| s.etag.clone()))
                .flatten()
        } else {
            None
        };

        let list_result = match adapter.list_repos_incremental(org_name, stored_etag).await {
            Ok(lr) => lr,
            Err(e) => {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to list remote repos for {}/{}: {}", org_name, forge_name, e),
                });
                continue;
            }
        };

        if !list_result.modified {
            events.push(HyperforgeEvent::Info {
                message: format!("  {}/{}: not modified (ETag match)", org_name, forge_name),
            });
            if let Some(ref fe) = forge_enum {
                let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                    last_synced: Utc::now(),
                    etag: list_result.etag.clone(),
                });
            }
            continue;
        }

        let remote_repos = list_result.repos.as_deref().unwrap_or_default();

        for remote_repo in remote_repos {
            match local.repo_exists(org_name, &remote_repo.name).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    events.push(HyperforgeEvent::Error {
                        message: format!("  Failed to check {}: {}", remote_repo.name, e),
                    });
                    continue;
                }
            }

            // Always populate in-memory state (even in dry-run) so Phase 6 diff is accurate
            if let Err(e) = local.create_repo(org_name, remote_repo).await {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to import {}: {}", remote_repo.name, e),
                });
                continue;
            }

            imported += 1;
            events.push(HyperforgeEvent::Info {
                message: format!("  {}Imported {} from {}", dry_prefix, remote_repo.name, forge_name),
            });
        }

        if let Some(ref fe) = forge_enum {
            if !is_dry_run {
                let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                    last_synced: Utc::now(),
                    etag: list_result.etag.clone(),
                });
            }
        }

        if !is_dry_run && imported > 0 {
            if let Err(e) = local.save_to_yaml().await {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                });
            }
        }
    }

    events.push(HyperforgeEvent::Info {
        message: format!("  {} repos imported from remotes", imported),
    });

    // Phase 5.5: Report unmanaged repos
    for org_name in orgs {
        let local = state.get_local_forge(org_name).await;
        if let Ok(records) = local.all_records() {
            let unmanaged: Vec<_> = records.iter()
                .filter(|r| !r.managed && !r.dismissed)
                .collect();
            if !unmanaged.is_empty() {
                events.push(HyperforgeEvent::Info {
                    message: format!("  {} unmanaged repos in LocalForge for org '{}' (not on disk):",
                        unmanaged.len(), org_name),
                });
                for r in &unmanaged {
                    let forges: Vec<String> = r.present_on.iter()
                        .map(|f| format!("{:?}", f).to_lowercase())
                        .collect();
                    events.push(HyperforgeEvent::Info {
                        message: format!("    {} [{}]", r.name, forges.join(", ")),
                    });
                }
            }
        }
    }

    events
}

/// Phase 7: Apply diffs to remotes (creates, updates, privatizations — no deletes).
async fn sync_apply_diffs(
    all_diffs: Vec<(String, String, crate::services::SyncDiff)>,
    state: &HyperforgeState,
    is_dry_run: bool,
) -> (Vec<HyperforgeEvent>, usize, usize) {
    let mut all_events = Vec::new();
    let mut total_created = 0usize;
    let mut total_updated = 0usize;

    let dry_run = is_dry_run;
    let items: Vec<_> = all_diffs.into_iter()
        .map(|(org_name, forge_name, diff)| (org_name, forge_name, diff, state.clone()))
        .collect();

    let results = run_batch(items, 8, move |(org_name, forge_name, diff, state)| async move {
        let ot = state.get_local_forge(&org_name).await.owner_type();
        let adapter = match make_adapter(&forge_name, &org_name, ot) {
            Ok(a) => a,
            Err(e) => return (vec![HyperforgeEvent::Error { message: e }], 0usize, 0usize),
        };

        let mut events = Vec::new();
        let mut created = 0usize;
        let mut updated = 0usize;

        for repo_op in &diff.ops {
            match repo_op.op {
                SyncOp::Create => {
                    if !dry_run {
                        if let Err(e) = adapter.create_repo(&org_name, &repo_op.repo).await {
                            events.push(HyperforgeEvent::Error {
                                message: format!("  Failed to create {} on {}: {}", repo_op.repo.name, forge_name, e),
                            });
                            continue;
                        }
                    }
                    created += 1;
                    events.push(HyperforgeEvent::SyncOp {
                        repo_name: repo_op.repo.name.clone(),
                        operation: "create".to_string(),
                        forge: forge_name.clone(),
                    });
                }
                SyncOp::Update => {
                    if !dry_run {
                        if let Err(e) = adapter.update_repo(&org_name, &repo_op.repo).await {
                            events.push(HyperforgeEvent::Error {
                                message: format!("  Failed to update {} on {}: {}", repo_op.repo.name, forge_name, e),
                            });
                            continue;
                        }
                    }
                    updated += 1;
                    events.push(HyperforgeEvent::SyncOp {
                        repo_name: repo_op.repo.name.clone(),
                        operation: "update".to_string(),
                        forge: forge_name.clone(),
                    });
                }
                SyncOp::Delete => {
                    let local = state.get_local_forge(&org_name).await;
                    let record_info = local.get_record(&repo_op.repo.name).ok();

                    if record_info.as_ref().map_or(false, |r| r.protected) {
                        events.push(HyperforgeEvent::SyncOp {
                            repo_name: repo_op.repo.name.clone(),
                            operation: "skip_protected".to_string(),
                            forge: forge_name.clone(),
                        });
                        continue;
                    }

                    let already_privatized = record_info.as_ref()
                        .and_then(|rec| {
                            HyperforgeConfig::parse_forge(&forge_name)
                                .map(|fe| rec.privatized_on.contains(&fe))
                        })
                        .unwrap_or(false);

                    if already_privatized {
                        events.push(HyperforgeEvent::SyncOp {
                            repo_name: repo_op.repo.name.clone(),
                            operation: "already_privatized".to_string(),
                            forge: forge_name.clone(),
                        });
                    } else {
                        let private_repo = crate::types::Repo::new(
                            &repo_op.repo.name,
                            repo_op.repo.origin.clone(),
                        ).with_visibility(crate::types::Visibility::Private);

                        if !dry_run {
                            match adapter.update_repo(&org_name, &private_repo).await {
                                Ok(_) => {
                                    if let Some(forge_enum) = HyperforgeConfig::parse_forge(&forge_name) {
                                        if let Ok(mut rec) = local.get_record(&repo_op.repo.name) {
                                            rec.privatized_on.insert(forge_enum);
                                            let _ = local.update_record(&rec);
                                            let _ = local.save_to_yaml().await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    events.push(HyperforgeEvent::Error {
                                        message: format!("  Failed to privatize {} on {}: {}",
                                            repo_op.repo.name, forge_name, e),
                                    });
                                }
                            }
                        }
                        events.push(HyperforgeEvent::SyncOp {
                            repo_name: repo_op.repo.name.clone(),
                            operation: "privatize".to_string(),
                            forge: forge_name.clone(),
                        });
                    }
                }
                SyncOp::InSync => {}
            }
        }

        (events, created, updated)
    }).await;

    for result in results {
        match result {
            Ok((events, created, updated)) => {
                total_created += created;
                total_updated += updated;
                all_events.extend(events);
            }
            Err(e) => {
                all_events.push(HyperforgeEvent::Error { message: format!("Task join error: {}", e) });
            }
        }
    }

    (all_events, total_created, total_updated)
}

/// Phase 7.5: Retire remote-only repos (reflect/purge mode).
async fn sync_retire_remote_only(
    pairs: &[(String, String)],
    ctx: &WorkspaceContext,
    state: &HyperforgeState,
    is_dry_run: bool,
    is_purge: bool,
) -> (Vec<HyperforgeEvent>, usize, usize, usize) {
    let mut events = Vec::new();
    let mut staged_count = 0usize;
    let mut purged_count = 0usize;
    let mut protected_skipped = 0usize;

    // Build set of local repo names from workspace discovery
    let local_names: HashSet<String> = ctx.repos.iter()
        .filter_map(|r| repo_from_config(r).map(|repo| repo.name))
        .collect();

    for (org_name, forge_name) in pairs {
        let local = state.get_local_forge(org_name).await;
        let ot = local.owner_type();

        let adapter = match make_adapter(forge_name, org_name, ot) {
            Ok(a) => a,
            Err(e) => {
                events.push(HyperforgeEvent::Error { message: e });
                continue;
            }
        };

        let forge_enum = HyperforgeConfig::parse_forge(forge_name);
        let stored_etag = if let Some(ref fe) = forge_enum {
            local.forge_states().ok()
                .and_then(|states| states.get(fe).map(|s| s.etag.clone()))
                .flatten()
        } else {
            None
        };

        let list_result = match adapter.list_repos_incremental(org_name, stored_etag).await {
            Ok(lr) => lr,
            Err(e) => {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to list remote repos for {}/{}: {}", org_name, forge_name, e),
                });
                continue;
            }
        };

        if !list_result.modified {
            events.push(HyperforgeEvent::Info {
                message: format!("  {}/{}: not modified (ETag match)", org_name, forge_name),
            });
            if let Some(ref fe) = forge_enum {
                let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                    last_synced: Utc::now(),
                    etag: list_result.etag.clone(),
                });
            }
            continue;
        }

        let remote_repos = list_result.repos.as_deref().unwrap_or_default();

        for remote_repo in remote_repos {
            if local_names.contains(&remote_repo.name) {
                continue;
            }

            if remote_repo.protected {
                protected_skipped += 1;
                events.push(HyperforgeEvent::Info {
                    message: format!(
                        "  Skipping {} on {} (protected/archived)",
                        remote_repo.name, forge_name,
                    ),
                });
                continue;
            }

            if is_purge {
                // Purge mode: only delete repos that were previously staged
                let is_staged = match local.get_repo(org_name, &remote_repo.name).await {
                    Ok(existing) => existing.staged_for_deletion,
                    Err(_) => false,
                };

                if !is_staged {
                    // Not previously staged — stage it instead
                    let privatized = remote_repo.clone()
                        .with_visibility(Visibility::Private)
                        .with_staged_for_deletion(true);

                    if !is_dry_run {
                        let _ = adapter.update_repo(org_name, &privatized).await;
                    }

                    match local.repo_exists(org_name, &remote_repo.name).await {
                        Ok(true) => { let _ = local.update_repo(org_name, &privatized).await; }
                        Ok(false) => { let _ = local.create_repo(org_name, &privatized).await; }
                        Err(_) => {}
                    }

                    staged_count += 1;
                    events.push(HyperforgeEvent::SyncOp {
                        repo_name: remote_repo.name.clone(),
                        operation: "staged".to_string(),
                        forge: forge_name.clone(),
                    });
                    continue;
                }

                // Check protection before purging
                let is_protected = local.get_record(&remote_repo.name)
                    .ok()
                    .map_or(false, |r| r.protected);
                if is_protected {
                    protected_skipped += 1;
                    events.push(HyperforgeEvent::Info {
                        message: format!(
                            "  Skipping purge of {} on {} (protected)",
                            remote_repo.name, forge_name,
                        ),
                    });
                    continue;
                }

                // Actually delete from remote
                if !is_dry_run {
                    if let Err(e) = adapter.delete_repo(org_name, &remote_repo.name).await {
                        events.push(HyperforgeEvent::Error {
                            message: format!("  Failed to delete {} on {}: {}", remote_repo.name, forge_name, e),
                        });
                        continue;
                    }
                    let _ = local.delete_repo(org_name, &remote_repo.name).await;
                    let _ = local.save_to_yaml().await;
                }

                purged_count += 1;
                events.push(HyperforgeEvent::SyncOp {
                    repo_name: remote_repo.name.clone(),
                    operation: "purged".to_string(),
                    forge: forge_name.clone(),
                });
            } else {
                // Default: stage for deletion (make private + flag)
                let privatized = remote_repo.clone()
                    .with_visibility(Visibility::Private)
                    .with_staged_for_deletion(true);

                if !is_dry_run {
                    if let Err(e) = adapter.update_repo(org_name, &privatized).await {
                        events.push(HyperforgeEvent::Error {
                            message: format!("  Failed to make {} private on {}: {}", remote_repo.name, forge_name, e),
                        });
                        continue;
                    }
                }

                match local.repo_exists(org_name, &remote_repo.name).await {
                    Ok(true) => { let _ = local.update_repo(org_name, &privatized).await; }
                    Ok(false) => { let _ = local.create_repo(org_name, &privatized).await; }
                    Err(_) => {}
                }

                staged_count += 1;
                events.push(HyperforgeEvent::SyncOp {
                    repo_name: remote_repo.name.clone(),
                    operation: "staged".to_string(),
                    forge: forge_name.clone(),
                });
            }
        }

        if let Some(ref fe) = forge_enum {
            if !is_dry_run {
                let _ = local.set_forge_state(fe.clone(), ForgeSyncState {
                    last_synced: Utc::now(),
                    etag: list_result.etag.clone(),
                });
            }
        }

        if !is_dry_run {
            if let Err(e) = local.save_to_yaml().await {
                events.push(HyperforgeEvent::Error {
                    message: format!("  Failed to save LocalForge for {}: {}", org_name, e),
                });
            }
        }
    }

    (events, staged_count, purged_count, protected_skipped)
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
