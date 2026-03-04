//! BuildHub — Development tools: manifest generation, publishing, cross-repo execution.
//!
//! These methods never write LocalForge or call forge APIs. They operate purely
//! on the filesystem via workspace discovery.

pub mod execution;
pub mod gitignore;
pub mod manifest;
pub mod packaging;

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
            filter = "Glob pattern to filter packages by name (optional)"
        )
    )]
    pub async fn package_diff(
        &self,
        path: String,
        filter: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::package_diff(path, filter)
    }

    /// Publish packages with transitive dependency resolution
    #[plexus_macros::hub_method(
        description = "Publish workspace packages in dependency order, auto-publishing transitive deps first. Dry-run by default — pass --execute to actually publish.",
        params(
            path = "Path to workspace root directory",
            filter = "Glob pattern to filter target packages by name (optional, default: all)",
            execute = "Actually publish to registries (default: false, dry-run unless set)",
            no_tag = "Skip creating git tags after publish (optional, default: false)",
            no_commit = "Skip auto-commit after version bumps (optional, default: false)",
            bump = "Version bump kind for auto-bump: patch, minor, major (optional, default: patch)"
        )
    )]
    pub async fn publish(
        &self,
        path: String,
        filter: Option<String>,
        execute: Option<bool>,
        no_tag: Option<bool>,
        no_commit: Option<bool>,
        bump: Option<String>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::publish(path, filter, execute, no_tag, no_commit, bump)
    }

    /// Bump versions for workspace packages
    #[plexus_macros::hub_method(
        description = "Bump package versions across the workspace",
        params(
            path = "Path to workspace root directory",
            filter = "Glob pattern to filter packages by name (optional, default: all)",
            bump = "Version bump kind: patch, minor, major (default: patch)",
            commit = "Auto-commit after bumping (optional, default: false)",
            dry_run = "Preview without writing changes (optional, default: false)"
        )
    )]
    pub async fn bump(
        &self,
        path: String,
        filter: Option<String>,
        bump: Option<String>,
        commit: Option<bool>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        packaging::bump(path, filter, bump, commit, dry_run)
    }

    /// Run a command across all workspace repos
    #[plexus_macros::hub_method(
        description = "Execute an arbitrary shell command in every workspace repo directory. Runs in parallel by default.",
        params(
            path = "Path to workspace directory",
            command = "Shell command to execute in each repo",
            filter = "Glob pattern to filter repos by name (optional)",
            sequential = "Run sequentially instead of in parallel (optional, default: false)",
            dirty = "Only run on repos with uncommitted changes (optional, default: false)"
        )
    )]
    pub async fn exec(
        &self,
        path: String,
        command: String,
        filter: Option<String>,
        sequential: Option<bool>,
        dirty: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        execution::exec(path, command, filter, sequential, dirty)
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

    /// Ensure sane .gitignore patterns across all workspace repos
    #[plexus_macros::hub_method(
        description = "Add missing .gitignore patterns across all workspace repos. Includes OS, editor, build artifact, and build-system-specific patterns. Idempotent — only adds what's missing.",
        params(
            path = "Path to workspace directory",
            patterns = "Extra patterns to add beyond defaults (optional)",
            filter = "Glob pattern to filter repos by name (optional)",
            dry_run = "Preview without writing files (optional, default: false)"
        )
    )]
    pub async fn gitignore_sync(
        &self,
        path: String,
        patterns: Option<Vec<String>>,
        filter: Option<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        gitignore::gitignore_sync(path, patterns, filter, dry_run)
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
