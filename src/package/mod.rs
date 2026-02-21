//! Package registry clients for version querying and publishing.
//!
//! Provides a unified trait for interacting with package registries
//! (crates.io, Hackage, npm) and concrete implementations.

pub mod crates_io;
pub mod hackage;

use crate::build_system::BuildSystemKind;
use crate::hub::PackageRegistry;
use async_trait::async_trait;
use std::path::Path;

/// Version information from a registry
#[derive(Debug, Clone)]
pub struct PublishedVersion {
    pub name: String,
    pub version: String,
}

/// Result of a publish operation
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub package_name: String,
    pub version: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Unified interface for package registry operations
#[async_trait]
pub trait RegistryClient: Send + Sync {
    /// Which build system this registry serves
    fn build_system(&self) -> BuildSystemKind;

    /// Registry identifier for event reporting
    fn registry_kind(&self) -> PackageRegistry;

    /// Query the registry for the latest published version.
    /// Returns None if the package has never been published.
    async fn published_version(&self, name: &str) -> anyhow::Result<Option<PublishedVersion>>;

    /// Publish a package. If dry_run is true, validate without actually publishing.
    async fn publish(
        &self,
        path: &Path,
        name: &str,
        dry_run: bool,
    ) -> anyhow::Result<PublishResult>;
}

/// Get the appropriate registry client for a build system kind.
/// Returns None for Unknown or unsupported build systems.
pub fn registry_for(kind: &BuildSystemKind) -> Option<Box<dyn RegistryClient>> {
    match kind {
        BuildSystemKind::Cargo => Some(Box::new(crates_io::CratesIoClient::new())),
        BuildSystemKind::Cabal => Some(Box::new(hackage::HackageClient::new())),
        BuildSystemKind::Node | BuildSystemKind::Unknown => None,
    }
}
