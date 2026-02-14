//! Build system detection and workspace management
//!
//! Detects build systems (Cargo, Cabal, Node) from filesystem markers,
//! parses manifests for dependency information, and generates native
//! workspace files for unified builds.

pub mod cabal;
pub mod cabal_project;
pub mod cargo;
pub mod cargo_config;
pub mod dep_graph;
pub mod node;
pub mod validate;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Known build system types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BuildSystemKind {
    Cargo,
    Cabal,
    Node,
    Unknown,
}

impl std::fmt::Display for BuildSystemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cargo => write!(f, "cargo"),
            Self::Cabal => write!(f, "cabal"),
            Self::Node => write!(f, "node"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// A dependency reference parsed from a manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DepRef {
    /// Dependency name (crate name, package name, npm package name)
    pub name: String,
    /// Version requirement string (e.g., "0.2.1", "^1.0", ">=2.0")
    pub version_req: Option<String>,
    /// Whether this is a path dependency (local reference)
    pub is_path_dep: bool,
    /// Path to the dependency (if path dep)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Detect the build system for a directory by checking filesystem markers.
///
/// Checks for Cargo.toml, *.cabal files, and package.json in order.
/// Returns the first match, or `Unknown` if none found.
pub fn detect_build_system(path: &Path) -> BuildSystemKind {
    if cargo::is_cargo_project(path) {
        BuildSystemKind::Cargo
    } else if cabal::is_cabal_project(path) {
        BuildSystemKind::Cabal
    } else if node::is_node_project(path) {
        BuildSystemKind::Node
    } else {
        BuildSystemKind::Unknown
    }
}

/// Detect all build systems present in a directory.
///
/// A directory can have multiple build systems (e.g., a Rust project
/// with a Node frontend).
pub fn detect_all_build_systems(path: &Path) -> Vec<BuildSystemKind> {
    let mut systems = Vec::new();
    if cargo::is_cargo_project(path) {
        systems.push(BuildSystemKind::Cargo);
    }
    if cabal::is_cabal_project(path) {
        systems.push(BuildSystemKind::Cabal);
    }
    if node::is_node_project(path) {
        systems.push(BuildSystemKind::Node);
    }
    systems
}

/// Parse dependencies from a project manifest.
///
/// Delegates to the appropriate parser based on `kind`.
pub fn parse_dependencies(path: &Path, kind: &BuildSystemKind) -> Vec<DepRef> {
    match kind {
        BuildSystemKind::Cargo => cargo::parse_cargo_deps(path),
        BuildSystemKind::Cabal => cabal::parse_cabal_deps(path),
        BuildSystemKind::Node => node::parse_node_deps(path),
        BuildSystemKind::Unknown => Vec::new(),
    }
}

/// Get the package name from a project manifest.
pub fn package_name(path: &Path, kind: &BuildSystemKind) -> Option<String> {
    match kind {
        BuildSystemKind::Cargo => cargo::cargo_package_name(path),
        BuildSystemKind::Cabal => cabal::cabal_package_name(path),
        BuildSystemKind::Node => node::node_package_name(path),
        BuildSystemKind::Unknown => None,
    }
}

/// Get the package version from a project manifest.
pub fn package_version(path: &Path, kind: &BuildSystemKind) -> Option<String> {
    match kind {
        BuildSystemKind::Cargo => cargo::cargo_package_version(path),
        BuildSystemKind::Cabal => cabal::cabal_package_version(path),
        BuildSystemKind::Node => node::node_package_version(path),
        BuildSystemKind::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_cargo() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(detect_build_system(tmp.path()), BuildSystemKind::Cargo);
    }

    #[test]
    fn test_detect_cabal() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.cabal"), "name: test\nversion: 0.1.0\n").unwrap();
        assert_eq!(detect_build_system(tmp.path()), BuildSystemKind::Cabal);
    }

    #[test]
    fn test_detect_node() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "test", "version": "0.1.0"}"#,
        )
        .unwrap();
        assert_eq!(detect_build_system(tmp.path()), BuildSystemKind::Node);
    }

    #[test]
    fn test_detect_unknown() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(detect_build_system(tmp.path()), BuildSystemKind::Unknown);
    }

    #[test]
    fn test_detect_all_multiple() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "test", "version": "0.1.0"}"#,
        )
        .unwrap();
        let systems = detect_all_build_systems(tmp.path());
        assert_eq!(systems.len(), 2);
        assert!(systems.contains(&BuildSystemKind::Cargo));
        assert!(systems.contains(&BuildSystemKind::Node));
    }
}
