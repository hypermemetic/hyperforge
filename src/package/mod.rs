//! Package management and publishing
//!
//! Unified interface for publishing to different package registries
//! (Cargo, npm, PyPI, Hex, Hackage)

use crate::types::VersionBump;
use async_trait::async_trait;
use std::path::Path;

/// Package registry trait
#[async_trait]
pub trait PackageRegistry: Send + Sync {
    /// Detect if this registry type exists at path
    async fn detect(&self, path: &Path) -> anyhow::Result<bool>;

    /// Bump version
    async fn bump_version(&self, path: &Path, bump: VersionBump) -> anyhow::Result<String>;

    /// Publish package
    async fn publish(&self, path: &Path, dry_run: bool) -> anyhow::Result<()>;
}
