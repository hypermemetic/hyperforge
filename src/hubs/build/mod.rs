//! BuildHub — Development tools: manifest generation, publishing, cross-repo execution.
//!
//! These methods never write LocalForge or call forge APIs. They operate purely
//! on the filesystem via workspace discovery.

pub mod dirty;
pub mod execution;
pub mod gitignore;
pub mod large_files;
pub mod loc;
pub mod local_run;
pub mod manifest;
pub mod packaging;
pub mod release;
pub mod repo_size;

use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream};
use serde_json::Value;

use crate::hub::HyperforgeEvent;

/// Sub-hub for development tools: manifest generation, publishing, cross-repo execution.
#[derive(Clone)]
pub struct BuildHub;

impl BuildHub {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BuildHub {
    fn default() -> Self {
        Self::new()
    }
}

#[plexus_macros::hub_methods(
    namespace = "build",
    description = "Development tools: manifest generation, publishing, cross-repo execution",
    crate_path = "plexus_core"
)]
impl BuildHub {
    /// Generate/update native workspace manifests (Cargo.toml, cabal.project)
    #[plexus_macros::hub_method(
        description = "Generate workspace config files (.cargo/config.toml with [patch.crates-io], cabal.project) from detected build systems. Each repo stays independent while sibling crates resolve locally.",
        params(
            path = "Path to workspace directory",
            dry_run = "Preview without writing files (optional, default: false)"
        )
    )]
    pub async fn unify(
        &self,
        path: String,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        manifest::unify(path, dry_run)
    }

    /// Analyze workspace dependency graph and detect version mismatches
    #[plexus_macros::hub_method(
        description = "Analyze workspace dependency graph: show build tiers, dependency relationships, and version mismatches between pinned and local versions.",
        params(
            path = "Path to workspace directory",
            format = "Output format: 'summary' (default), 'graph', or 'mismatches'"
        )
    )]
    pub async fn analyze(
        &self,
        path: String,
        format: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        manifest::analyze(path, format)
    }

    /// Detect mismatches between directory names and package names
    #[plexus_macros::hub_method(
        description = "Detect repos where the directory name differs from the package name in the build manifest. Also reports git repos without hyperforge config (run `hyperforge init` to configure them).",
        params(
            path = "Path to workspace root directory"
        )
    )]
    pub async fn detect_name_mismatches(
        &self,
        path: String,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        manifest::detect_name_mismatches(path)
    }

    /// Compare local package versions against their registries
    #[plexus_macros::hub_method(
        description = "Show local vs published versions for workspace packages",
        params(
            path = "Path to workspace root directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)"
        )
    )]
    pub async fn package_diff(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::package_diff(path, include, exclude)
    }

    /// Publish packages with transitive dependency resolution
    #[plexus_macros::hub_method(
        description = "Publish workspace packages in dependency order, auto-publishing transitive deps first. Dry-run by default — pass --execute to actually publish.",
        params(
            path = "Path to workspace root directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            execute = "Actually publish to registries (default: false, dry-run unless set)",
            no_tag = "Skip creating git tags after publish (optional, default: false)",
            no_commit = "Skip auto-commit after version bumps (optional, default: false)",
            bump = "Version bump kind for auto-bump: patch, minor, major (optional, default: patch)"
        )
    )]
    pub async fn publish(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        execute: Option<bool>,
        no_tag: Option<bool>,
        no_commit: Option<bool>,
        bump: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::publish(path, include, exclude, execute, no_tag, no_commit, bump)
    }

    /// Bump versions for workspace packages
    #[plexus_macros::hub_method(
        description = "Bump package versions across the workspace",
        params(
            path = "Path to workspace root directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            bump = "Version bump kind: patch, minor, major (default: patch)",
            commit = "Auto-commit after bumping (optional, default: false)",
            dry_run = "Preview without writing changes (optional, default: false)"
        )
    )]
    pub async fn bump(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        bump: Option<String>,
        commit: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::bump(path, include, exclude, bump, commit, dry_run)
    }

    /// Run a command across all workspace repos
    #[plexus_macros::hub_method(
        description = "Execute an arbitrary shell command in every workspace repo directory. Runs in parallel by default.",
        params(
            path = "Path to workspace directory",
            command = "Shell command to execute in each repo",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            sequential = "Run sequentially instead of in parallel (optional, default: false)",
            dirty = "Only run on repos with uncommitted changes (optional, default: false)"
        )
    )]
    pub async fn exec(
        &self,
        path: String,
        command: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        sequential: Option<bool>,
        dirty: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        execution::exec(path, command, include, exclude, sequential, dirty)
    }

    /// Validate workspace builds in Docker containers
    #[plexus_macros::hub_method(
        description = "Run containerized builds and tests in dependency order. Uses Docker to validate the entire workspace compiles before pushing.",
        params(
            path = "Path to workspace directory",
            test = "Also run tests after builds (optional, default: false)",
            dry_run = "Preview validation plan without running Docker (optional, default: false)",
            image = "Docker image to use (optional, default: rust:latest)"
        )
    )]
    pub async fn validate(
        &self,
        path: String,
        test: Option<bool>,
        dry_run: Option<bool>,
        image: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        execution::validate(path, test, dry_run, image)
    }

    /// Run build/test commands using layered CI runners
    #[plexus_macros::hub_method(
        description = "Run build and test commands in dependency order using [ci] runners. Level 0 = quick check, level 1 = full build, level 2 = containerized. Without --level, runs all local runners.",
        params(
            path = "Path to workspace directory",
            test = "Also run test commands (optional, default: false)",
            level = "Runner level to execute (0, 1, 2...). Without this, runs all local runners.",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            dry_run = "Preview commands without executing (optional, default: false)",
            parallel = "Max concurrent repos per tier (optional, default: unbounded)"
        )
    )]
    pub async fn run(
        &self,
        path: String,
        test: Option<bool>,
        level: Option<usize>,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        dry_run: Option<bool>,
        parallel: Option<usize>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        local_run::run(path, test, level, include, exclude, dry_run, parallel)
    }

    /// Initialize CI configs for repos that lack them
    #[plexus_macros::hub_method(
        description = "Generate default [ci] runner configs for workspace repos that don't have one. Detects build system (Cargo/Cabal/Node) and writes layered runners to .hyperforge/config.toml. Idempotent — repos with existing CI config are untouched.",
        params(
            path = "Path to workspace directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            dry_run = "Preview without writing files (optional, default: false)"
        )
    )]
    pub async fn init_configs(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        local_run::init_configs(path, include, exclude, dry_run)
    }

    /// Ensure sane .gitignore patterns across all workspace repos
    #[plexus_macros::hub_method(
        description = "Add missing .gitignore patterns across all workspace repos. Includes OS, editor, build artifact, and build-system-specific patterns. Idempotent — only adds what's missing.",
        params(
            path = "Path to workspace directory",
            patterns = "Extra patterns to add beyond defaults (optional)",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            dry_run = "Preview without writing files (optional, default: false)"
        )
    )]
    pub async fn gitignore_sync(
        &self,
        path: String,
        patterns: Option<Vec<String>>,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        gitignore::gitignore_sync(path, patterns, include, exclude, dry_run)
    }

    /// Find large tracked files across workspace repos
    #[plexus_macros::hub_method(
        description = "Find large tracked files across all workspace repos. Scans git-tracked files only.",
        params(
            path = "Path to workspace directory",
            threshold_kb = "Size threshold in KB (optional, default: 100)",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)"
        )
    )]
    pub async fn large_files(
        &self,
        path: String,
        threshold_kb: Option<u64>,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        large_files::large_files(path, threshold_kb, include, exclude)
    }

    /// Show total tracked-file size for each workspace repo
    #[plexus_macros::hub_method(
        description = "Show total size of git-tracked files per repo, sorted by size descending",
        params(
            path = "Path to workspace directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)"
        )
    )]
    pub async fn repo_sizes(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        repo_size::repo_sizes(path, include, exclude)
    }

    /// Count lines of code per repo
    #[plexus_macros::hub_method(
        description = "Count lines of code per repo, sorted by total lines descending. Breaks down by file extension.",
        params(
            path = "Path to workspace directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)"
        )
    )]
    pub async fn loc(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        loc::loc(path, include, exclude)
    }

    /// Check which repos have uncommitted changes
    #[plexus_macros::hub_method(
        description = "Find repos with staged, unstaged, or untracked changes. Only reports dirty repos by default.",
        params(
            path = "Path to workspace directory",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            all_git = "Include all git repos, not just hyperforge-configured ones (optional, default: false)"
        )
    )]
    pub async fn dirty(
        &self,
        path: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        all_git: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        dirty::dirty(path, include, exclude, all_git)
    }

    /// Cross-compile, package, create forge releases, and upload assets
    #[plexus_macros::hub_method(
        description = "All-in-one release orchestrator: cross-compile binaries, package archives, create tagged releases on forges, and upload assets. Works for a single repo or entire workspace.",
        params(
            path = "Path to workspace or repo directory",
            tag = "Git tag for the release (e.g. v4.1.0)",
            targets = "Comma-separated target triples (optional, defaults to native host)",
            include = "Glob patterns — repo must match at least one (optional, repeatable)",
            exclude = "Glob patterns — repo matching any is excluded; exclude wins over include (optional, repeatable)",
            forge = "Target forges, comma-separated (optional, defaults to all configured)",
            title = "Release title (optional, defaults to tag)",
            body = "Release description/notes (optional)",
            draft = "Create as draft release (optional, default: false)",
            dry_run = "Preview everything without side effects (optional, default: false)"
        )
    )]
    pub async fn release(
        &self,
        path: String,
        tag: String,
        targets: Option<String>,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        forge: Option<String>,
        title: Option<String>,
        body: Option<String>,
        draft: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        release::release(path, tag, targets, include, exclude, forge, title, body, draft, dry_run)
    }
}

#[async_trait]
impl ChildRouter for BuildHub {
    fn router_namespace(&self) -> &str {
        "build"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None // Leaf plugin
    }
}
