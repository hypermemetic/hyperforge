//! Cargo/crates.io package publishing
//!
//! Publishes Rust packages to crates.io using `cargo publish`

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

use crate::types::VersionBump;
use super::PackageRegistry;

/// Cargo package publisher for crates.io
pub struct CargoPublisher {
    /// Optional registry token (if not set, uses ~/.cargo/credentials.toml)
    token: Option<String>,
    /// Custom registry name (defaults to crates.io)
    registry: Option<String>,
}

impl CargoPublisher {
    /// Create a new CargoPublisher using default credentials
    pub fn new() -> Self {
        Self {
            token: None,
            registry: None,
        }
    }

    /// Create a CargoPublisher with an explicit token
    pub fn with_token(token: String) -> Self {
        Self {
            token: Some(token),
            registry: None,
        }
    }

    /// Create a CargoPublisher for a custom registry
    pub fn with_registry(registry: String, token: Option<String>) -> Self {
        Self {
            token,
            registry: Some(registry),
        }
    }
}

impl Default for CargoPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PackageRegistry for CargoPublisher {
    async fn detect(&self, path: &Path) -> anyhow::Result<bool> {
        Ok(path.join("Cargo.toml").exists())
    }

    async fn bump_version(&self, path: &Path, bump: VersionBump) -> anyhow::Result<String> {
        let cargo_toml_path = path.join("Cargo.toml");
        let content = tokio::fs::read_to_string(&cargo_toml_path).await?;
        let mut doc = content.parse::<toml_edit::DocumentMut>()?;

        let current_version = doc["package"]["version"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No version found in Cargo.toml"))?;

        let new_version = bump_semver(current_version, &bump)?;
        doc["package"]["version"] = toml_edit::value(&new_version);

        tokio::fs::write(&cargo_toml_path, doc.to_string()).await?;
        Ok(new_version)
    }

    async fn publish(&self, path: &Path, dry_run: bool) -> anyhow::Result<()> {
        if dry_run {
            // For dry-run, use `cargo package` which only checks local packaging
            // without verifying dependencies exist on crates.io
            // This allows testing monorepo packages before any are published
            let mut cmd = Command::new("cargo");
            cmd.arg("package");
            cmd.arg("--allow-dirty");
            cmd.arg("--no-verify"); // Skip build verification
            cmd.current_dir(path);

            let output = cmd.output().await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                anyhow::bail!(
                    "cargo package failed:\nstdout: {}\nstderr: {}",
                    stdout,
                    stderr
                );
            }
        } else {
            // Actually publish
            let mut cmd = Command::new("cargo");
            cmd.arg("publish");
            cmd.current_dir(path);

            // Add token if provided
            if let Some(ref token) = self.token {
                cmd.arg("--token").arg(token);
            }

            // Add registry if not crates.io
            if let Some(ref registry) = self.registry {
                cmd.arg("--registry").arg(registry);
            }

            // Allow dirty working directory (we handle git state separately)
            cmd.arg("--allow-dirty");

            let output = cmd.output().await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                anyhow::bail!(
                    "cargo publish failed:\nstdout: {}\nstderr: {}",
                    stdout,
                    stderr
                );
            }
        }

        Ok(())
    }
}

/// Bump a semver version string (handles prerelease suffixes like -alpha, -worktree)
fn bump_semver(version: &str, bump: &VersionBump) -> anyhow::Result<String> {
    // Strip prerelease suffix if present (e.g., "0.2.1-worktree" -> "0.2.1")
    let base_version = version.split('-').next().unwrap_or(version);

    let parts: Vec<&str> = base_version.split('.').collect();
    if parts.len() != 3 {
        anyhow::bail!("Invalid semver version: {}", version);
    }

    let major: u64 = parts[0].parse()?;
    let minor: u64 = parts[1].parse()?;
    let patch: u64 = parts[2].parse()?;

    let (new_major, new_minor, new_patch) = match bump {
        VersionBump::Major => (major + 1, 0, 0),
        VersionBump::Minor => (major, minor + 1, 0),
        VersionBump::Patch => (major, minor, patch + 1),
    };

    // Bumping removes prerelease suffix (you're releasing a new version)
    Ok(format!("{}.{}.{}", new_major, new_minor, new_patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bump_semver_patch() {
        assert_eq!(bump_semver("1.2.3", &VersionBump::Patch).unwrap(), "1.2.4");
    }

    #[test]
    fn test_bump_semver_minor() {
        assert_eq!(bump_semver("1.2.3", &VersionBump::Minor).unwrap(), "1.3.0");
    }

    #[test]
    fn test_bump_semver_major() {
        assert_eq!(bump_semver("1.2.3", &VersionBump::Major).unwrap(), "2.0.0");
    }

    #[test]
    fn test_bump_semver_prerelease() {
        // Prerelease suffix should be stripped when bumping
        assert_eq!(bump_semver("0.2.1-worktree", &VersionBump::Patch).unwrap(), "0.2.2");
        assert_eq!(bump_semver("0.2.1-alpha", &VersionBump::Minor).unwrap(), "0.3.0");
        assert_eq!(bump_semver("1.0.0-beta.1", &VersionBump::Major).unwrap(), "2.0.0");
    }

    #[tokio::test]
    async fn test_detect_cargo() {
        let publisher = CargoPublisher::new();
        // Current directory should have Cargo.toml
        let result = publisher.detect(Path::new(".")).await.unwrap();
        assert!(result);
    }
}
