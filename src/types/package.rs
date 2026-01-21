//! Package types for repos that contain publishable packages.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Semantic version bump type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VersionBump {
    /// Bump patch version (0.0.x) - bug fixes, no API changes
    Patch,
    /// Bump minor version (0.x.0) - new features, backwards compatible
    Minor,
    /// Bump major version (x.0.0) - breaking changes
    Major,
}

impl VersionBump {
    /// Apply this bump to a semver version string.
    /// Returns the new version with sub-components zeroed as appropriate.
    ///
    /// Examples:
    /// - `1.2.3` + Patch → `1.2.4`
    /// - `1.2.3` + Minor → `1.3.0`
    /// - `1.2.3` + Major → `2.0.0`
    pub fn apply(&self, version: &str) -> Result<String, String> {
        // Parse version, handling optional pre-release suffix
        let base_version = version.split('-').next().unwrap_or(version);
        let parts: Vec<&str> = base_version.split('.').collect();

        if parts.len() < 3 {
            return Err(format!("Invalid semver version: {}", version));
        }

        let major: u64 = parts[0].parse()
            .map_err(|_| format!("Invalid major version: {}", parts[0]))?;
        let minor: u64 = parts[1].parse()
            .map_err(|_| format!("Invalid minor version: {}", parts[1]))?;
        let patch: u64 = parts[2].parse()
            .map_err(|_| format!("Invalid patch version: {}", parts[2]))?;

        let (new_major, new_minor, new_patch) = match self {
            VersionBump::Patch => (major, minor, patch + 1),
            VersionBump::Minor => (major, minor + 1, 0),
            VersionBump::Major => (major + 1, 0, 0),
        };

        Ok(format!("{}.{}.{}", new_major, new_minor, new_patch))
    }
}

impl std::fmt::Display for VersionBump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionBump::Patch => write!(f, "patch"),
            VersionBump::Minor => write!(f, "minor"),
            VersionBump::Major => write!(f, "major"),
        }
    }
}

impl std::str::FromStr for VersionBump {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "patch" => Ok(VersionBump::Patch),
            "minor" => Ok(VersionBump::Minor),
            "major" => Ok(VersionBump::Major),
            _ => Err(format!("Invalid version bump: {}. Use 'patch', 'minor', or 'major'", s)),
        }
    }
}

/// Type of package/registry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    /// Rust crate (crates.io)
    Crate,
    /// npm package (npmjs.com)
    Npm,
    /// Hex package (hex.pm) - Elixir/Erlang
    Hex,
    /// Hackage package (hackage.haskell.org)
    Hackage,
    /// PyPI package (pypi.org)
    PyPi,
}

impl PackageType {
    /// Get the default registry for this package type
    pub fn default_registry(&self) -> &'static str {
        match self {
            PackageType::Crate => "crates.io",
            PackageType::Npm => "npmjs.com",
            PackageType::Hex => "hex.pm",
            PackageType::Hackage => "hackage.haskell.org",
            PackageType::PyPi => "pypi.org",
        }
    }

    /// Get the manifest filename for this package type
    pub fn manifest_file(&self) -> &'static str {
        match self {
            PackageType::Crate => "Cargo.toml",
            PackageType::Npm => "package.json",
            PackageType::Hex => "mix.exs",
            PackageType::Hackage => "*.cabal",
            PackageType::PyPi => "pyproject.toml",
        }
    }

    /// Get the secret key name for this package type's token
    pub fn token_key(&self) -> &'static str {
        match self {
            PackageType::Crate => "crates-token",
            PackageType::Npm => "npm-token",
            PackageType::Hex => "hex-token",
            PackageType::Hackage => "hackage-token",
            PackageType::PyPi => "pypi-token",
        }
    }

    /// Get the publish command for this package type
    pub fn publish_command(&self) -> &'static str {
        match self {
            PackageType::Crate => "cargo publish",
            PackageType::Npm => "npm publish",
            PackageType::Hex => "mix hex.publish",
            PackageType::Hackage => "cabal upload",
            PackageType::PyPi => "twine upload dist/*",
        }
    }
}

impl std::fmt::Display for PackageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageType::Crate => write!(f, "crate"),
            PackageType::Npm => write!(f, "npm"),
            PackageType::Hex => write!(f, "hex"),
            PackageType::Hackage => write!(f, "hackage"),
            PackageType::PyPi => write!(f, "pypi"),
        }
    }
}

impl std::str::FromStr for PackageType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "crate" | "cargo" | "rust" => Ok(PackageType::Crate),
            "npm" | "node" | "js" => Ok(PackageType::Npm),
            "hex" | "elixir" | "erlang" => Ok(PackageType::Hex),
            "hackage" | "haskell" | "cabal" => Ok(PackageType::Hackage),
            "pypi" | "python" | "pip" => Ok(PackageType::PyPi),
            _ => Err(format!("Unknown package type: {}", s)),
        }
    }
}

/// Configuration for a package within a repository
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PackageConfig {
    /// Package name (as published to registry)
    pub name: String,

    /// Type of package
    #[serde(rename = "type")]
    pub package_type: PackageType,

    /// Path to package root relative to repo root (default: ".")
    #[serde(default = "default_path")]
    pub path: PathBuf,

    /// Registry to publish to (default: type's default registry)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

    /// Whether to publish this package (default: true)
    #[serde(default = "default_true")]
    pub publish: bool,

    /// Custom publish command (overrides default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_command: Option<String>,
}

fn default_path() -> PathBuf {
    PathBuf::from(".")
}

fn default_true() -> bool {
    true
}

impl PackageConfig {
    /// Get the registry URL for this package
    pub fn registry(&self) -> &str {
        self.registry
            .as_deref()
            .unwrap_or_else(|| self.package_type.default_registry())
    }

    /// Get the publish command for this package
    pub fn publish_command(&self) -> &str {
        self.publish_command
            .as_deref()
            .unwrap_or_else(|| self.package_type.publish_command())
    }

    /// Get the token key for this package's registry
    pub fn token_key(&self) -> &str {
        self.package_type.token_key()
    }
}

/// Build configuration for a repository
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BuildConfig {
    /// Build command to run
    pub command: String,

    /// Working directory (relative to repo root)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,

    /// Environment variables to set
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,

    /// Artifacts produced by the build
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactConfig>,
}

/// Environment variable configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// Artifact configuration for build outputs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactConfig {
    /// Path to artifact (relative to repo root, supports globs)
    pub path: String,

    /// Where to upload the artifact
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_to: Option<ArtifactDestination>,
}

/// Destination for build artifacts
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactDestination {
    /// Upload to GitHub Releases
    GithubReleases,
    /// Upload to Codeberg Releases
    CodebergReleases,
    /// Upload to a custom URL
    Custom { url: String },
}

/// Summary of a package for display
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PackageSummary {
    pub name: String,
    pub package_type: PackageType,
    pub path: PathBuf,
    pub registry: String,
    pub local_version: Option<String>,
    pub published_version: Option<String>,
    pub needs_publish: bool,
}

/// Result of a publish operation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PublishResult {
    pub name: String,
    pub package_type: PackageType,
    pub registry: String,
    pub version: String,
    pub success: bool,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_type_from_str() {
        assert_eq!("crate".parse::<PackageType>().unwrap(), PackageType::Crate);
        assert_eq!("npm".parse::<PackageType>().unwrap(), PackageType::Npm);
        assert_eq!("python".parse::<PackageType>().unwrap(), PackageType::PyPi);
    }

    #[test]
    fn test_package_type_defaults() {
        assert_eq!(PackageType::Crate.default_registry(), "crates.io");
        assert_eq!(PackageType::Crate.manifest_file(), "Cargo.toml");
        assert_eq!(PackageType::Crate.token_key(), "crates-token");
    }

    #[test]
    fn test_package_config_defaults() {
        let config: PackageConfig = serde_yaml::from_str(r#"
            name: my-crate
            type: crate
        "#).unwrap();

        assert_eq!(config.path, PathBuf::from("."));
        assert_eq!(config.registry(), "crates.io");
        assert!(config.publish);
    }

    #[test]
    fn test_version_bump_patch() {
        assert_eq!(VersionBump::Patch.apply("1.2.3").unwrap(), "1.2.4");
        assert_eq!(VersionBump::Patch.apply("0.0.0").unwrap(), "0.0.1");
        assert_eq!(VersionBump::Patch.apply("1.2.99").unwrap(), "1.2.100");
    }

    #[test]
    fn test_version_bump_minor() {
        assert_eq!(VersionBump::Minor.apply("1.2.3").unwrap(), "1.3.0");
        assert_eq!(VersionBump::Minor.apply("0.0.5").unwrap(), "0.1.0");
        assert_eq!(VersionBump::Minor.apply("1.99.3").unwrap(), "1.100.0");
    }

    #[test]
    fn test_version_bump_major() {
        assert_eq!(VersionBump::Major.apply("1.2.3").unwrap(), "2.0.0");
        assert_eq!(VersionBump::Major.apply("0.5.10").unwrap(), "1.0.0");
        assert_eq!(VersionBump::Major.apply("99.1.1").unwrap(), "100.0.0");
    }

    #[test]
    fn test_version_bump_with_prerelease() {
        // Should strip prerelease suffix and bump
        assert_eq!(VersionBump::Patch.apply("1.2.3-alpha").unwrap(), "1.2.4");
        assert_eq!(VersionBump::Minor.apply("1.2.3-beta.1").unwrap(), "1.3.0");
    }

    #[test]
    fn test_version_bump_from_str() {
        assert_eq!("patch".parse::<VersionBump>().unwrap(), VersionBump::Patch);
        assert_eq!("MINOR".parse::<VersionBump>().unwrap(), VersionBump::Minor);
        assert_eq!("Major".parse::<VersionBump>().unwrap(), VersionBump::Major);
        assert!("invalid".parse::<VersionBump>().is_err());
    }
}
