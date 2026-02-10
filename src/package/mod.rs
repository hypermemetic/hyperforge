//! Package management and publishing
//!
//! Unified interface for publishing to different package registries
//! (Cargo/crates.io, Cabal/Hackage)

pub mod cargo;
pub mod cabal;

use crate::types::VersionBump;
use async_trait::async_trait;
use std::path::Path;

pub use cargo::CargoPublisher;
pub use cabal::CabalPublisher;

/// Package registry trait for publishing packages
#[async_trait]
pub trait PackageRegistry: Send + Sync {
    /// Detect if this registry type exists at path
    async fn detect(&self, path: &Path) -> anyhow::Result<bool>;

    /// Bump version according to semver/PVP rules
    async fn bump_version(&self, path: &Path, bump: VersionBump) -> anyhow::Result<String>;

    /// Publish package to registry
    /// If dry_run is true, validates but doesn't actually publish
    async fn publish(&self, path: &Path, dry_run: bool) -> anyhow::Result<()>;
}

/// Registry that auto-detects package type and delegates to appropriate publisher
pub struct AutoPublisher {
    cargo: CargoPublisher,
    cabal: CabalPublisher,
}

impl AutoPublisher {
    pub fn new() -> Self {
        Self {
            cargo: CargoPublisher::new(),
            cabal: CabalPublisher::new(),
        }
    }

    /// Create with custom Cargo token
    pub fn with_cargo_token(mut self, token: String) -> Self {
        self.cargo = CargoPublisher::with_token(token);
        self
    }

    /// Create with Hackage credentials
    pub fn with_hackage_credentials(mut self, username: String, password: String) -> Self {
        self.cabal = CabalPublisher::with_credentials(username, password);
        self
    }

    /// Get the appropriate publisher for a path
    pub async fn publisher_for(&self, path: &Path) -> anyhow::Result<&dyn PackageRegistry> {
        if self.cargo.detect(path).await? {
            Ok(&self.cargo)
        } else if self.cabal.detect(path).await? {
            Ok(&self.cabal)
        } else {
            anyhow::bail!("No supported package manifest found at {:?}", path)
        }
    }
}

impl Default for AutoPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PackageRegistry for AutoPublisher {
    async fn detect(&self, path: &Path) -> anyhow::Result<bool> {
        Ok(self.cargo.detect(path).await? || self.cabal.detect(path).await?)
    }

    async fn bump_version(&self, path: &Path, bump: VersionBump) -> anyhow::Result<String> {
        self.publisher_for(path).await?.bump_version(path, bump).await
    }

    async fn publish(&self, path: &Path, dry_run: bool) -> anyhow::Result<()> {
        self.publisher_for(path).await?.publish(path, dry_run).await
    }
}
