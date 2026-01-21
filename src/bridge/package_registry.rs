//! Package registry trait and implementations.
//!
//! Provides a unified interface for publishing packages to various registries
//! (crates.io, npm, PyPI, etc.). Uses SecretStore for token management.

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

use crate::bridge::SecretStore;
use crate::types::{PackageConfig, PackageSummary, PackageType, PublishResult, VersionBump};

/// Trait for package registry operations.
#[async_trait]
pub trait PackageRegistry: Send + Sync {
    /// Get the registry name
    fn name(&self) -> &str;

    /// Get the package type this registry handles
    fn package_type(&self) -> PackageType;

    /// Read package info from the local manifest
    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String>;

    /// Get the latest published version from the registry
    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String>;

    /// Bump the version in the manifest file according to semver.
    /// Returns the new version string.
    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String>;

    /// Publish a package to the registry.
    /// If `bump` is provided, the version will be incremented before publishing.
    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String>;

    /// Check if a specific version is already published
    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String>;
}

/// Create a package registry for the given package type.
///
/// Uses the provided SecretStore to retrieve authentication tokens.
pub fn create_registry(
    package_type: &PackageType,
    secret_store: Box<dyn SecretStore>,
) -> Box<dyn PackageRegistry> {
    match package_type {
        PackageType::Crate => Box::new(CratesRegistry::new(secret_store)),
        PackageType::Npm => Box::new(NpmRegistry::new(secret_store)),
        PackageType::Hex => Box::new(HexRegistry::new(secret_store)),
        PackageType::Hackage => Box::new(HackageRegistry::new(secret_store)),
        PackageType::PyPi => Box::new(PyPiRegistry::new(secret_store)),
    }
}

// ============================================================================
// CratesRegistry - crates.io
// ============================================================================

pub struct CratesRegistry {
    secret_store: Box<dyn SecretStore>,
}

impl CratesRegistry {
    pub fn new(secret_store: Box<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    async fn get_token(&self) -> Result<String, String> {
        self.secret_store
            .get("crates-token")
            .await?
            .ok_or_else(|| "crates-token not set. Use `secrets set --key crates-token`".to_string())
    }
}

#[async_trait]
impl PackageRegistry for CratesRegistry {
    fn name(&self) -> &str {
        "crates.io"
    }

    fn package_type(&self) -> PackageType {
        PackageType::Crate
    }

    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String> {
        let manifest_path = repo_path.join(&package.path).join("Cargo.toml");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

        // Parse TOML to get version
        let toml: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

        let local_version = toml
            .get("package")
            .and_then(|p: &toml_edit::Item| p.get("version"))
            .and_then(|v: &toml_edit::Item| v.as_str())
            .map(|s: &str| s.to_string());

        let published_version = self.get_published_version(&package.name).await.ok().flatten();

        let needs_publish = match (&local_version, &published_version) {
            (Some(local), Some(published)) => local != published,
            (Some(_), None) => true,
            _ => false,
        };

        Ok(PackageSummary {
            name: package.name.clone(),
            package_type: PackageType::Crate,
            path: package.path.clone(),
            registry: package.registry().to_string(),
            local_version,
            published_version,
            needs_publish,
        })
    }

    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String> {
        let url = format!("https://crates.io/api/v1/crates/{}", package_name);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("User-Agent", "hyperforge")
            .send()
            .await
            .map_err(|e| format!("Failed to query crates.io: {}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!("crates.io API error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let version = json
            .get("crate")
            .and_then(|c| c.get("max_version"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(version)
    }

    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String> {
        let manifest_path = repo_path.join(&package.path).join("Cargo.toml");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

        let mut toml: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

        let current_version = toml
            .get("package")
            .and_then(|p: &toml_edit::Item| p.get("version"))
            .and_then(|v: &toml_edit::Item| v.as_str())
            .ok_or("No version found in Cargo.toml")?
            .to_string();

        let new_version = bump.apply(&current_version)?;

        // Update the version in the document
        toml["package"]["version"] = toml_edit::value(&new_version);

        tokio::fs::write(&manifest_path, toml.to_string())
            .await
            .map_err(|e| format!("Failed to write Cargo.toml: {}", e))?;

        Ok(new_version)
    }

    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String> {
        // Bump version if requested
        let version = if let Some(bump_type) = bump {
            self.bump_version(repo_path, package, bump_type).await?
        } else {
            let summary = self.read_manifest(repo_path, package).await?;
            summary.local_version.unwrap_or_else(|| "unknown".to_string())
        };

        let token = self.get_token().await?;
        let package_path = repo_path.join(&package.path);

        let mut cmd = Command::new("cargo");
        cmd.current_dir(&package_path)
            .args(["publish", "--token", &token]);

        if dry_run {
            cmd.arg("--dry-run");
        }

        let output = cmd.output().await.map_err(|e| format!("Failed to run cargo: {}", e))?;

        let success = output.status.success();
        let message = if success {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        };

        Ok(PublishResult {
            name: package.name.clone(),
            package_type: PackageType::Crate,
            registry: self.name().to_string(),
            version,
            success,
            message,
        })
    }

    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String> {
        let url = format!("https://crates.io/api/v1/crates/{}/{}", package_name, version);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("User-Agent", "hyperforge")
            .send()
            .await
            .map_err(|e| format!("Failed to query crates.io: {}", e))?;

        Ok(response.status().is_success())
    }
}

// ============================================================================
// NpmRegistry - npmjs.com
// ============================================================================

pub struct NpmRegistry {
    secret_store: Box<dyn SecretStore>,
}

impl NpmRegistry {
    pub fn new(secret_store: Box<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    async fn get_token(&self) -> Result<String, String> {
        self.secret_store
            .get("npm-token")
            .await?
            .ok_or_else(|| "npm-token not set. Use `secrets set --key npm-token`".to_string())
    }
}

#[async_trait]
impl PackageRegistry for NpmRegistry {
    fn name(&self) -> &str {
        "npmjs.com"
    }

    fn package_type(&self) -> PackageType {
        PackageType::Npm
    }

    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String> {
        let manifest_path = repo_path.join(&package.path).join("package.json");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read package.json: {}", e))?;

        let json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse package.json: {}", e))?;

        let local_version = json
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let published_version = self.get_published_version(&package.name).await.ok().flatten();

        let needs_publish = match (&local_version, &published_version) {
            (Some(local), Some(published)) => local != published,
            (Some(_), None) => true,
            _ => false,
        };

        Ok(PackageSummary {
            name: package.name.clone(),
            package_type: PackageType::Npm,
            path: package.path.clone(),
            registry: package.registry().to_string(),
            local_version,
            published_version,
            needs_publish,
        })
    }

    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String> {
        let url = format!("https://registry.npmjs.org/{}/latest", package_name);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query npm: {}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!("npm API error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let version = json
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(version)
    }

    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String> {
        let manifest_path = repo_path.join(&package.path).join("package.json");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read package.json: {}", e))?;

        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse package.json: {}", e))?;

        let current_version = json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or("No version found in package.json")?
            .to_string();

        let new_version = bump.apply(&current_version)?;

        json["version"] = serde_json::Value::String(new_version.clone());

        let new_content = serde_json::to_string_pretty(&json)
            .map_err(|e| format!("Failed to serialize package.json: {}", e))?;

        tokio::fs::write(&manifest_path, new_content)
            .await
            .map_err(|e| format!("Failed to write package.json: {}", e))?;

        Ok(new_version)
    }

    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String> {
        // Bump version if requested
        let version = if let Some(bump_type) = bump {
            self.bump_version(repo_path, package, bump_type).await?
        } else {
            let summary = self.read_manifest(repo_path, package).await?;
            summary.local_version.unwrap_or_else(|| "unknown".to_string())
        };

        let token = self.get_token().await?;
        let package_path = repo_path.join(&package.path);

        // Write .npmrc with token
        let npmrc_path = package_path.join(".npmrc");
        let npmrc_content = format!("//registry.npmjs.org/:_authToken={}\n", token);
        tokio::fs::write(&npmrc_path, &npmrc_content)
            .await
            .map_err(|e| format!("Failed to write .npmrc: {}", e))?;

        let mut cmd = Command::new("npm");
        cmd.current_dir(&package_path).arg("publish");

        if dry_run {
            cmd.arg("--dry-run");
        }

        let output = cmd.output().await.map_err(|e| format!("Failed to run npm: {}", e))?;

        // Clean up .npmrc
        let _ = tokio::fs::remove_file(&npmrc_path).await;

        let success = output.status.success();
        let message = if success {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        };

        Ok(PublishResult {
            name: package.name.clone(),
            package_type: PackageType::Npm,
            registry: self.name().to_string(),
            version,
            success,
            message,
        })
    }

    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String> {
        let url = format!("https://registry.npmjs.org/{}/{}", package_name, version);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query npm: {}", e))?;

        Ok(response.status().is_success())
    }
}

// ============================================================================
// HexRegistry - hex.pm (Elixir/Erlang)
// ============================================================================

pub struct HexRegistry {
    secret_store: Box<dyn SecretStore>,
}

impl HexRegistry {
    pub fn new(secret_store: Box<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    async fn get_token(&self) -> Result<String, String> {
        self.secret_store
            .get("hex-token")
            .await?
            .ok_or_else(|| "hex-token not set. Use `secrets set --key hex-token`".to_string())
    }
}

#[async_trait]
impl PackageRegistry for HexRegistry {
    fn name(&self) -> &str {
        "hex.pm"
    }

    fn package_type(&self) -> PackageType {
        PackageType::Hex
    }

    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String> {
        let manifest_path = repo_path.join(&package.path).join("mix.exs");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read mix.exs: {}", e))?;

        // Simple regex to extract version from mix.exs
        let version_re = regex::Regex::new(r#"version:\s*"([^"]+)""#)
            .map_err(|e| format!("Regex error: {}", e))?;

        let local_version = version_re
            .captures(&content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        let published_version = self.get_published_version(&package.name).await.ok().flatten();

        let needs_publish = match (&local_version, &published_version) {
            (Some(local), Some(published)) => local != published,
            (Some(_), None) => true,
            _ => false,
        };

        Ok(PackageSummary {
            name: package.name.clone(),
            package_type: PackageType::Hex,
            path: package.path.clone(),
            registry: package.registry().to_string(),
            local_version,
            published_version,
            needs_publish,
        })
    }

    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String> {
        let url = format!("https://hex.pm/api/packages/{}", package_name);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query hex.pm: {}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!("hex.pm API error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Get latest release version
        let version = json
            .get("releases")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("version"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(version)
    }

    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String> {
        let manifest_path = repo_path.join(&package.path).join("mix.exs");

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("Failed to read mix.exs: {}", e))?;

        let version_re = regex::Regex::new(r#"version:\s*"([^"]+)""#)
            .map_err(|e| format!("Regex error: {}", e))?;

        let current_version = version_re
            .captures(&content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or("No version found in mix.exs")?;

        let new_version = bump.apply(&current_version)?;

        // Replace version in mix.exs
        let new_content = version_re.replace(&content, format!(r#"version: "{}""#, new_version));

        tokio::fs::write(&manifest_path, new_content.as_ref())
            .await
            .map_err(|e| format!("Failed to write mix.exs: {}", e))?;

        Ok(new_version)
    }

    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String> {
        // Bump version if requested
        let version = if let Some(bump_type) = bump {
            self.bump_version(repo_path, package, bump_type).await?
        } else {
            let summary = self.read_manifest(repo_path, package).await?;
            summary.local_version.unwrap_or_else(|| "unknown".to_string())
        };

        let token = self.get_token().await?;
        let package_path = repo_path.join(&package.path);

        let mut cmd = Command::new("mix");
        cmd.current_dir(&package_path)
            .env("HEX_API_KEY", &token)
            .args(["hex.publish", "--yes"]);

        if dry_run {
            // Hex doesn't have a dry-run flag, so we just return early
            return Ok(PublishResult {
                name: package.name.clone(),
                package_type: PackageType::Hex,
                registry: self.name().to_string(),
                version,
                success: true,
                message: Some("Dry run - would publish".to_string()),
            });
        }

        let output = cmd.output().await.map_err(|e| format!("Failed to run mix: {}", e))?;

        let success = output.status.success();
        let message = if success {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        };

        Ok(PublishResult {
            name: package.name.clone(),
            package_type: PackageType::Hex,
            registry: self.name().to_string(),
            version,
            success,
            message,
        })
    }

    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String> {
        let url = format!("https://hex.pm/api/packages/{}/releases/{}", package_name, version);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query hex.pm: {}", e))?;

        Ok(response.status().is_success())
    }
}

// ============================================================================
// HackageRegistry - hackage.haskell.org
// ============================================================================

pub struct HackageRegistry {
    secret_store: Box<dyn SecretStore>,
}

impl HackageRegistry {
    pub fn new(secret_store: Box<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    async fn get_credentials(&self) -> Result<(String, String), String> {
        let username = self.secret_store
            .get("hackage-username")
            .await?
            .ok_or_else(|| "hackage-username not set".to_string())?;
        let password = self.secret_store
            .get("hackage-password")
            .await?
            .ok_or_else(|| "hackage-password not set".to_string())?;
        Ok((username, password))
    }
}

#[async_trait]
impl PackageRegistry for HackageRegistry {
    fn name(&self) -> &str {
        "hackage.haskell.org"
    }

    fn package_type(&self) -> PackageType {
        PackageType::Hackage
    }

    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String> {
        let package_path = repo_path.join(&package.path);

        // Find .cabal file
        let mut cabal_file = None;
        let mut entries = tokio::fs::read_dir(&package_path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();
            if path.extension().map(|e| e == "cabal").unwrap_or(false) {
                cabal_file = Some(path);
                break;
            }
        }

        let cabal_path = cabal_file.ok_or("No .cabal file found")?;
        let content = tokio::fs::read_to_string(&cabal_path)
            .await
            .map_err(|e| format!("Failed to read .cabal file: {}", e))?;

        // Simple regex to extract version
        let version_re = regex::Regex::new(r"(?i)^version:\s*(.+)$")
            .map_err(|e| format!("Regex error: {}", e))?;

        let local_version = content
            .lines()
            .find_map(|line| {
                version_re.captures(line.trim()).and_then(|c| c.get(1)).map(|m| m.as_str().trim().to_string())
            });

        let published_version = self.get_published_version(&package.name).await.ok().flatten();

        let needs_publish = match (&local_version, &published_version) {
            (Some(local), Some(published)) => local != published,
            (Some(_), None) => true,
            _ => false,
        };

        Ok(PackageSummary {
            name: package.name.clone(),
            package_type: PackageType::Hackage,
            path: package.path.clone(),
            registry: package.registry().to_string(),
            local_version,
            published_version,
            needs_publish,
        })
    }

    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String> {
        let url = format!("https://hackage.haskell.org/package/{}/preferred.json", package_name);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query Hackage: {}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!("Hackage API error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let version = json
            .get("normal-version")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(version)
    }

    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String> {
        let package_path = repo_path.join(&package.path);

        // Find .cabal file
        let mut cabal_file = None;
        let mut entries = tokio::fs::read_dir(&package_path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();
            if path.extension().map(|e| e == "cabal").unwrap_or(false) {
                cabal_file = Some(path);
                break;
            }
        }

        let cabal_path = cabal_file.ok_or("No .cabal file found")?;
        let content = tokio::fs::read_to_string(&cabal_path)
            .await
            .map_err(|e| format!("Failed to read .cabal file: {}", e))?;

        let version_re = regex::Regex::new(r"(?im)^(version:\s*)(.+)$")
            .map_err(|e| format!("Regex error: {}", e))?;

        let current_version = content
            .lines()
            .find_map(|line| {
                let lower = line.to_lowercase();
                if lower.trim().starts_with("version:") {
                    Some(line.split(':').nth(1)?.trim().to_string())
                } else {
                    None
                }
            })
            .ok_or("No version found in .cabal file")?;

        let new_version = bump.apply(&current_version)?;

        // Replace version in .cabal file
        let new_content = version_re.replace(&content, format!("${{1}}{}", new_version));

        tokio::fs::write(&cabal_path, new_content.as_ref())
            .await
            .map_err(|e| format!("Failed to write .cabal file: {}", e))?;

        Ok(new_version)
    }

    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String> {
        // Bump version if requested
        let version = if let Some(bump_type) = bump {
            self.bump_version(repo_path, package, bump_type).await?
        } else {
            let summary = self.read_manifest(repo_path, package).await?;
            summary.local_version.unwrap_or_else(|| "unknown".to_string())
        };

        let (username, password) = self.get_credentials().await?;
        let package_path = repo_path.join(&package.path);

        let mut cmd = Command::new("cabal");
        cmd.current_dir(&package_path)
            .args(["upload", "--username", &username, "--password", &password]);

        if dry_run {
            // Just check if we can create the sdist
            let output = Command::new("cabal")
                .current_dir(&package_path)
                .args(["sdist"])
                .output()
                .await
                .map_err(|e| format!("Failed to run cabal: {}", e))?;

            return Ok(PublishResult {
                name: package.name.clone(),
                package_type: PackageType::Hackage,
                registry: self.name().to_string(),
                version,
                success: output.status.success(),
                message: Some("Dry run - sdist created".to_string()),
            });
        }

        let output = cmd.output().await.map_err(|e| format!("Failed to run cabal: {}", e))?;

        let success = output.status.success();
        let message = if success {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        };

        Ok(PublishResult {
            name: package.name.clone(),
            package_type: PackageType::Hackage,
            registry: self.name().to_string(),
            version,
            success,
            message,
        })
    }

    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String> {
        let url = format!("https://hackage.haskell.org/package/{}-{}", package_name, version);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query Hackage: {}", e))?;

        Ok(response.status().is_success())
    }
}

// ============================================================================
// PyPiRegistry - pypi.org
// ============================================================================

pub struct PyPiRegistry {
    secret_store: Box<dyn SecretStore>,
}

impl PyPiRegistry {
    pub fn new(secret_store: Box<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    async fn get_token(&self) -> Result<String, String> {
        self.secret_store
            .get("pypi-token")
            .await?
            .ok_or_else(|| "pypi-token not set. Use `secrets set --key pypi-token`".to_string())
    }
}

#[async_trait]
impl PackageRegistry for PyPiRegistry {
    fn name(&self) -> &str {
        "pypi.org"
    }

    fn package_type(&self) -> PackageType {
        PackageType::PyPi
    }

    async fn read_manifest(&self, repo_path: &Path, package: &PackageConfig) -> Result<PackageSummary, String> {
        let package_path = repo_path.join(&package.path);

        // Try pyproject.toml first
        let pyproject_path = package_path.join("pyproject.toml");
        let local_version = if pyproject_path.exists() {
            let content = tokio::fs::read_to_string(&pyproject_path)
                .await
                .map_err(|e| format!("Failed to read pyproject.toml: {}", e))?;

            let toml: toml_edit::DocumentMut = content
                .parse()
                .map_err(|e| format!("Failed to parse pyproject.toml: {}", e))?;

            toml.get("project")
                .or_else(|| toml.get("tool").and_then(|t: &toml_edit::Item| t.get("poetry")))
                .and_then(|p: &toml_edit::Item| p.get("version"))
                .and_then(|v: &toml_edit::Item| v.as_str())
                .map(|s: &str| s.to_string())
        } else {
            // Try setup.py
            let setup_path = package_path.join("setup.py");
            if setup_path.exists() {
                let content = tokio::fs::read_to_string(&setup_path)
                    .await
                    .map_err(|e| format!("Failed to read setup.py: {}", e))?;

                let version_re = regex::Regex::new(r#"version\s*=\s*["']([^"']+)["']"#)
                    .map_err(|e| format!("Regex error: {}", e))?;

                version_re
                    .captures(&content)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
            } else {
                None
            }
        };

        let published_version = self.get_published_version(&package.name).await.ok().flatten();

        let needs_publish = match (&local_version, &published_version) {
            (Some(local), Some(published)) => local != published,
            (Some(_), None) => true,
            _ => false,
        };

        Ok(PackageSummary {
            name: package.name.clone(),
            package_type: PackageType::PyPi,
            path: package.path.clone(),
            registry: package.registry().to_string(),
            local_version,
            published_version,
            needs_publish,
        })
    }

    async fn get_published_version(&self, package_name: &str) -> Result<Option<String>, String> {
        let url = format!("https://pypi.org/pypi/{}/json", package_name);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query PyPI: {}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!("PyPI API error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let version = json
            .get("info")
            .and_then(|i| i.get("version"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(version)
    }

    async fn bump_version(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: VersionBump,
    ) -> Result<String, String> {
        let package_path = repo_path.join(&package.path);

        // Try pyproject.toml first
        let pyproject_path = package_path.join("pyproject.toml");
        if pyproject_path.exists() {
            let content = tokio::fs::read_to_string(&pyproject_path)
                .await
                .map_err(|e| format!("Failed to read pyproject.toml: {}", e))?;

            let mut toml: toml_edit::DocumentMut = content
                .parse()
                .map_err(|e| format!("Failed to parse pyproject.toml: {}", e))?;

            // Try [project] first, then [tool.poetry]
            let current_version = toml
                .get("project")
                .and_then(|p: &toml_edit::Item| p.get("version"))
                .and_then(|v: &toml_edit::Item| v.as_str())
                .or_else(|| {
                    toml.get("tool")
                        .and_then(|t: &toml_edit::Item| t.get("poetry"))
                        .and_then(|p: &toml_edit::Item| p.get("version"))
                        .and_then(|v: &toml_edit::Item| v.as_str())
                })
                .ok_or("No version found in pyproject.toml")?
                .to_string();

            let new_version = bump.apply(&current_version)?;

            // Update in the appropriate section
            if toml.get("project").and_then(|p: &toml_edit::Item| p.get("version")).is_some() {
                toml["project"]["version"] = toml_edit::value(&new_version);
            } else if toml.get("tool")
                .and_then(|t: &toml_edit::Item| t.get("poetry"))
                .and_then(|p: &toml_edit::Item| p.get("version"))
                .is_some()
            {
                toml["tool"]["poetry"]["version"] = toml_edit::value(&new_version);
            }

            tokio::fs::write(&pyproject_path, toml.to_string())
                .await
                .map_err(|e| format!("Failed to write pyproject.toml: {}", e))?;

            return Ok(new_version);
        }

        // Fall back to setup.py
        let setup_path = package_path.join("setup.py");
        if setup_path.exists() {
            let content = tokio::fs::read_to_string(&setup_path)
                .await
                .map_err(|e| format!("Failed to read setup.py: {}", e))?;

            let version_re = regex::Regex::new(r#"(version\s*=\s*["'])([^"']+)(["'])"#)
                .map_err(|e| format!("Regex error: {}", e))?;

            let current_version = version_re
                .captures(&content)
                .and_then(|c| c.get(2))
                .map(|m| m.as_str().to_string())
                .ok_or("No version found in setup.py")?;

            let new_version = bump.apply(&current_version)?;

            let new_content = version_re.replace(&content, format!("${{1}}{}${{3}}", new_version));

            tokio::fs::write(&setup_path, new_content.as_ref())
                .await
                .map_err(|e| format!("Failed to write setup.py: {}", e))?;

            return Ok(new_version);
        }

        Err("No pyproject.toml or setup.py found".to_string())
    }

    async fn publish(
        &self,
        repo_path: &Path,
        package: &PackageConfig,
        bump: Option<VersionBump>,
        dry_run: bool,
    ) -> Result<PublishResult, String> {
        // Bump version if requested
        let version = if let Some(bump_type) = bump {
            self.bump_version(repo_path, package, bump_type).await?
        } else {
            let summary = self.read_manifest(repo_path, package).await?;
            summary.local_version.unwrap_or_else(|| "unknown".to_string())
        };

        let token = self.get_token().await?;
        let package_path = repo_path.join(&package.path);

        // Build the package first
        let build_output = Command::new("python")
            .current_dir(&package_path)
            .args(["-m", "build"])
            .output()
            .await
            .map_err(|e| format!("Failed to build package: {}", e))?;

        if !build_output.status.success() {
            return Ok(PublishResult {
                name: package.name.clone(),
                package_type: PackageType::PyPi,
                registry: self.name().to_string(),
                version,
                success: false,
                message: Some(format!("Build failed: {}", String::from_utf8_lossy(&build_output.stderr))),
            });
        }

        if dry_run {
            return Ok(PublishResult {
                name: package.name.clone(),
                package_type: PackageType::PyPi,
                registry: self.name().to_string(),
                version,
                success: true,
                message: Some("Dry run - package built successfully".to_string()),
            });
        }

        // Upload with twine
        let output = Command::new("twine")
            .current_dir(&package_path)
            .args(["upload", "dist/*", "--username", "__token__", "--password", &token])
            .output()
            .await
            .map_err(|e| format!("Failed to run twine: {}", e))?;

        let success = output.status.success();
        let message = if success {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        };

        Ok(PublishResult {
            name: package.name.clone(),
            package_type: PackageType::PyPi,
            registry: self.name().to_string(),
            version,
            success,
            message,
        })
    }

    async fn version_exists(&self, package_name: &str, version: &str) -> Result<bool, String> {
        let url = format!("https://pypi.org/pypi/{}/{}/json", package_name, version);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to query PyPI: {}", e))?;

        Ok(response.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests would require actual registry access
    // Unit tests can verify the trait is object-safe

    #[test]
    fn test_trait_object_safe() {
        fn _accepts_registry(_: &dyn PackageRegistry) {}
    }
}
