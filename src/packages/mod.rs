//! Package management abstractions for hyperforge
//!
//! This module provides a unified interface for working with different
//! package ecosystems (Cargo, Cabal, etc.) to enable:
//! - Determining publish order via topological sort of dependencies
//! - Updating package versions across a workspace

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub mod cargo;
pub mod cabal;

/// Errors that can occur during package operations
#[derive(Debug, Error)]
pub enum PackageError {
    #[error("Failed to read manifest at {path}: {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to parse manifest at {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Failed to write manifest at {path}: {source}")]
    WriteError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Cycle detected in dependency graph: {packages:?}")]
    CycleDetected { packages: Vec<String> },

    #[error("Package not found: {name}")]
    PackageNotFound { name: String },
}

pub type PackageResult<T> = Result<T, PackageError>;

/// Supported package managers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageManager {
    Cargo,
    Cabal,
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManager::Cargo => write!(f, "cargo"),
            PackageManager::Cabal => write!(f, "cabal"),
        }
    }
}

/// A package in the workspace
#[derive(Debug, Clone)]
pub struct Package {
    /// Package name (from manifest)
    pub name: String,
    /// Directory name (may differ from package name if renamed)
    pub dir_name: String,
    /// Current version
    pub version: String,
    /// Path to the package root (directory containing manifest)
    pub path: PathBuf,
    /// Package manager type
    pub manager: PackageManager,
    /// Names of dependencies that are also in this workspace
    pub workspace_deps: Vec<String>,
}

/// Result of setting up patch dependencies
#[derive(Debug, Clone)]
pub struct SetupPatchesResult {
    /// List of patches added: (package_name, relative_path)
    pub patches_added: Vec<(String, String)>,
    /// Whether the file was modified
    pub modified: bool,
}

/// A detected package rename (directory name differs from package name)
#[derive(Debug, Clone)]
pub struct PackageRename {
    /// Old name (directory name, likely the old package name)
    pub old_name: String,
    /// New name (current package name in manifest)
    pub new_name: String,
    /// Package version
    pub version: String,
    /// Path to the package
    pub path: PathBuf,
    /// Package manager type
    pub manager: PackageManager,
}

impl Package {
    /// Check if this package has been renamed (dir name differs from package name)
    pub fn is_renamed(&self) -> bool {
        self.dir_name != self.name
    }

    /// Get rename info if package was renamed
    pub fn as_rename(&self) -> Option<PackageRename> {
        if self.is_renamed() {
            Some(PackageRename {
                old_name: self.dir_name.clone(),
                new_name: self.name.clone(),
                version: self.version.clone(),
                path: self.path.clone(),
                manager: self.manager,
            })
        } else {
            None
        }
    }
}

/// Trait for package manifest parsers/writers
pub trait PackageManifest: Send + Sync {
    /// Get the package manager type
    fn manager(&self) -> PackageManager;

    /// Get the manifest filename (e.g., "Cargo.toml", "*.cabal")
    fn manifest_name(&self) -> &str;

    /// Detect if this manifest type exists at path
    fn detect(&self, path: &Path) -> bool;

    /// Find the manifest file in a directory
    fn find_manifest(&self, path: &Path) -> Option<PathBuf>;

    /// Parse package info from manifest
    fn parse(&self, path: &Path) -> PackageResult<Package>;

    /// Get all dependency names from the manifest
    fn all_dependencies(&self, path: &Path) -> PackageResult<Vec<String>>;

    /// Update a dependency version in the manifest
    fn set_dependency_version(
        &self,
        path: &Path,
        dep_name: &str,
        new_version: &str,
    ) -> PackageResult<()>;

    /// Update the package's own version
    fn set_package_version(&self, path: &Path, new_version: &str) -> PackageResult<()>;

    /// Update path dependencies that reference old_dir_name to use new_dir_name
    /// Returns list of modified file paths
    fn update_path_dependencies(
        &self,
        path: &Path,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> PackageResult<Vec<PathBuf>> {
        // Default implementation: no-op (for package managers without path deps)
        let _ = (path, old_dir_name, new_dir_name);
        Ok(vec![])
    }
}

/// Registry of all package manifest handlers
pub struct PackageRegistry {
    handlers: Vec<Box<dyn PackageManifest>>,
}

impl Default for PackageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageRegistry {
    pub fn new() -> Self {
        Self {
            handlers: vec![
                Box::new(cargo::CargoManifest),
                Box::new(cabal::CabalManifest),
            ],
        }
    }

    /// Detect which package manager is used in a directory
    pub fn detect(&self, path: &Path) -> Option<&dyn PackageManifest> {
        self.handlers.iter().find_map(|h| {
            if h.detect(path) {
                Some(h.as_ref())
            } else {
                None
            }
        })
    }

    /// Detect and parse a package in a directory
    pub fn detect_package(&self, path: &Path) -> Option<Package> {
        self.detect(path).and_then(|h| h.parse(path).ok())
    }

    /// Scan a workspace and collect all packages
    pub fn scan_workspace(&self, workspace_path: &Path) -> PackageResult<Vec<Package>> {
        let mut packages = Vec::new();

        // Read directory entries
        let entries = std::fs::read_dir(workspace_path).map_err(|e| PackageError::ReadError {
            path: workspace_path.to_path_buf(),
            source: e,
        })?;

        // First pass: collect all package names
        let mut package_names = HashSet::new();
        let mut detected_packages = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Skip hidden directories and common non-package dirs
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
            }

            if let Some(handler) = self.detect(&path) {
                if let Ok(pkg) = handler.parse(&path) {
                    package_names.insert(pkg.name.clone());
                    detected_packages.push((path, handler));
                }
            }
        }

        // Second pass: parse with workspace context
        for (path, handler) in detected_packages {
            let mut pkg = handler.parse(&path)?;

            // Filter dependencies to only include workspace-local ones
            let all_deps = handler.all_dependencies(&path)?;
            pkg.workspace_deps = all_deps
                .into_iter()
                .filter(|dep| package_names.contains(dep))
                .collect();

            packages.push(pkg);
        }

        Ok(packages)
    }

    /// Detect packages that have been renamed (directory name differs from package name)
    pub fn detect_renames(&self, workspace_path: &Path) -> PackageResult<Vec<PackageRename>> {
        let packages = self.scan_workspace(workspace_path)?;
        Ok(packages.into_iter().filter_map(|p| p.as_rename()).collect())
    }

    /// Update path dependencies across the workspace after a directory rename
    /// Returns list of modified manifest paths
    pub fn update_workspace_path_deps(
        &self,
        workspace_path: &Path,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> PackageResult<Vec<PathBuf>> {
        let mut modified = Vec::new();

        let entries = std::fs::read_dir(workspace_path).map_err(|e| PackageError::ReadError {
            path: workspace_path.to_path_buf(),
            source: e,
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Skip hidden directories and common non-package dirs
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
            }

            if let Some(handler) = self.detect(&path) {
                let updated_files = handler.update_path_dependencies(&path, old_dir_name, new_dir_name)?;
                modified.extend(updated_files);
            }
        }

        Ok(modified)
    }

    /// Get topological publish order (dependencies first)
    pub fn publish_order(&self, packages: &[Package]) -> PackageResult<Vec<String>> {
        // Build adjacency list: package -> packages that depend on it
        let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();

        // Initialize
        for pkg in packages {
            graph.entry(pkg.name.as_str()).or_default();
            in_degree.entry(pkg.name.as_str()).or_insert(0);
        }

        // Build edges: if A depends on B, then B -> A (B must be published before A)
        for pkg in packages {
            for dep in &pkg.workspace_deps {
                if let Some(dep_pkg) = packages.iter().find(|p| &p.name == dep) {
                    graph.entry(dep_pkg.name.as_str()).or_default().push(&pkg.name);
                    *in_degree.entry(pkg.name.as_str()).or_insert(0) += 1;
                }
            }
        }

        // Kahn's algorithm for topological sort
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&name, _)| name)
            .collect();

        // Sort for deterministic output
        queue.sort();

        let mut result = Vec::new();

        while let Some(node) = queue.pop() {
            result.push(node.to_string());

            if let Some(neighbors) = graph.get(node) {
                let mut next_nodes = Vec::new();
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            next_nodes.push(neighbor);
                        }
                    }
                }
                // Sort for deterministic output
                next_nodes.sort();
                // Reverse because we pop from end
                next_nodes.reverse();
                queue.extend(next_nodes);
            }
        }

        // Check for cycles
        if result.len() != packages.len() {
            let remaining: Vec<String> = packages
                .iter()
                .filter(|p| !result.contains(&p.name))
                .map(|p| p.name.clone())
                .collect();
            return Err(PackageError::CycleDetected { packages: remaining });
        }

        Ok(result)
    }

    /// Update a package version across the workspace
    pub fn update_package_version(
        &self,
        workspace_path: &Path,
        package_name: &str,
        new_version: &str,
    ) -> PackageResult<Vec<PathBuf>> {
        let packages = self.scan_workspace(workspace_path)?;
        let mut updated_paths = Vec::new();

        // Find the package itself and update its version
        let target_pkg = packages
            .iter()
            .find(|p| p.name == package_name)
            .ok_or_else(|| PackageError::PackageNotFound {
                name: package_name.to_string(),
            })?;

        let handler = self
            .detect(&target_pkg.path)
            .ok_or_else(|| PackageError::PackageNotFound {
                name: package_name.to_string(),
            })?;

        handler.set_package_version(&target_pkg.path, new_version)?;
        updated_paths.push(target_pkg.path.clone());

        // Update all packages that depend on this one
        for pkg in &packages {
            if pkg.workspace_deps.contains(&package_name.to_string()) {
                if let Some(h) = self.detect(&pkg.path) {
                    h.set_dependency_version(&pkg.path, package_name, new_version)?;
                    updated_paths.push(pkg.path.clone());
                }
            }
        }

        Ok(updated_paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publish_order_simple() {
        let packages = vec![
            Package {
                name: "leaf".to_string(),
                dir_name: "leaf".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/leaf"),
                manager: PackageManager::Cargo,
                workspace_deps: vec![],
            },
            Package {
                name: "middle".to_string(),
                dir_name: "middle".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/middle"),
                manager: PackageManager::Cargo,
                workspace_deps: vec!["leaf".to_string()],
            },
            Package {
                name: "root".to_string(),
                dir_name: "root".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/root"),
                manager: PackageManager::Cargo,
                workspace_deps: vec!["middle".to_string()],
            },
        ];

        let registry = PackageRegistry::new();
        let order = registry.publish_order(&packages).unwrap();

        assert_eq!(order, vec!["leaf", "middle", "root"]);
    }

    #[test]
    fn test_publish_order_diamond() {
        // Diamond dependency: root -> {a, b} -> leaf
        let packages = vec![
            Package {
                name: "leaf".to_string(),
                dir_name: "leaf".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/leaf"),
                manager: PackageManager::Cargo,
                workspace_deps: vec![],
            },
            Package {
                name: "a".to_string(),
                dir_name: "a".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/a"),
                manager: PackageManager::Cargo,
                workspace_deps: vec!["leaf".to_string()],
            },
            Package {
                name: "b".to_string(),
                dir_name: "b".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/b"),
                manager: PackageManager::Cargo,
                workspace_deps: vec!["leaf".to_string()],
            },
            Package {
                name: "root".to_string(),
                dir_name: "root".to_string(),
                version: "1.0.0".to_string(),
                path: PathBuf::from("/root"),
                manager: PackageManager::Cargo,
                workspace_deps: vec!["a".to_string(), "b".to_string()],
            },
        ];

        let registry = PackageRegistry::new();
        let order = registry.publish_order(&packages).unwrap();

        // leaf must come first, root must come last
        assert_eq!(order[0], "leaf");
        assert_eq!(order[3], "root");
        // a and b can be in either order, but both before root
        assert!(order[1] == "a" || order[1] == "b");
        assert!(order[2] == "a" || order[2] == "b");
    }
}
