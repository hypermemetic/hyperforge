//! Cabal (Haskell) package manifest handler
//!
//! Cabal files use a custom format with key: value pairs and indented sections.
//! Example:
//! ```cabal
//! name:          my-package
//! version:       0.1.0.0
//! build-depends: base >=4.7 && <5, text, aeson
//! ```

use std::path::{Path, PathBuf};
use regex::Regex;

use super::{Package, PackageError, PackageManifest, PackageManager, PackageResult};

/// Handler for *.cabal manifests
pub struct CabalManifest;

impl CabalManifest {
    /// Find the .cabal file in a directory
    fn find_cabal_file(path: &Path) -> Option<PathBuf> {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map(|e| e == "cabal").unwrap_or(false) {
                    return Some(p);
                }
            }
        }
        None
    }

    fn read_manifest(path: &Path) -> PackageResult<(PathBuf, String)> {
        let cabal_path = Self::find_cabal_file(path).ok_or_else(|| PackageError::ReadError {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "No .cabal file found"),
        })?;
        let content = std::fs::read_to_string(&cabal_path).map_err(|e| PackageError::ReadError {
            path: cabal_path.clone(),
            source: e,
        })?;
        Ok((cabal_path, content))
    }

    /// Parse a field value from cabal content (handles multi-line values)
    fn get_field(content: &str, field: &str) -> Option<String> {
        let field_lower = field.to_lowercase();
        for line in content.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.starts_with(&field_lower) {
                if let Some(idx) = line.find(':') {
                    let value = line[idx + 1..].trim();
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Parse build-depends field and extract package names
    fn parse_build_depends(content: &str) -> Vec<String> {
        let mut deps = Vec::new();
        let mut in_build_depends = false;
        let mut depends_content = String::new();

        for line in content.lines() {
            let trimmed = line.trim_start();
            let line_lower = trimmed.to_lowercase();

            if line_lower.starts_with("build-depends:") {
                in_build_depends = true;
                if let Some(idx) = trimmed.find(':') {
                    depends_content.push_str(&trimmed[idx + 1..]);
                }
            } else if in_build_depends {
                // Check if this is a continuation (indented) or a new field
                if line.starts_with(' ') || line.starts_with('\t') {
                    depends_content.push_str(" ");
                    depends_content.push_str(trimmed);
                } else if !trimmed.is_empty() {
                    // New field, stop collecting
                    break;
                }
            }
        }

        // Parse the collected dependencies
        // Format: "base >=4.7 && <5, text, aeson >=1.0"
        let pkg_name_re = Regex::new(r"([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();

        for part in depends_content.split(',') {
            let part = part.trim();
            if let Some(cap) = pkg_name_re.captures(part) {
                if let Some(name) = cap.get(1) {
                    deps.push(name.as_str().to_string());
                }
            }
        }

        deps
    }

    /// Update a field value in cabal content
    fn set_field(content: &str, field: &str, new_value: &str) -> String {
        let field_lower = field.to_lowercase();
        let mut result = Vec::new();
        let mut found = false;

        for line in content.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.starts_with(&field_lower) && line.contains(':') {
                // Find the position of the colon and preserve formatting
                if let Some(idx) = line.find(':') {
                    let prefix = &line[..=idx];
                    // Try to preserve spacing after colon
                    let after_colon = &line[idx + 1..];
                    let spaces = after_colon.len() - after_colon.trim_start().len();
                    let padding = if spaces > 0 {
                        " ".repeat(spaces)
                    } else {
                        " ".to_string()
                    };
                    result.push(format!("{}{}{}", prefix, padding, new_value));
                    found = true;
                } else {
                    result.push(line.to_string());
                }
            } else {
                result.push(line.to_string());
            }
        }

        if !found {
            // Add the field if it doesn't exist
            result.push(format!("{}: {}", field, new_value));
        }

        result.join("\n")
    }

    /// Update a dependency version in build-depends
    fn update_dependency_in_content(content: &str, dep_name: &str, new_version: &str) -> String {
        // This is tricky because cabal version constraints are complex
        // We'll use a regex to find and replace the package with its version constraint
        let pattern = format!(r"(?i)\b{}\s*(>=?|<=?|==|\^>=)?\s*[\d.]*(\s*&&\s*(>=?|<=?|==|\^>=)\s*[\d.]*)?", regex::escape(dep_name));
        let re = Regex::new(&pattern).unwrap();

        let replacement = format!("{} =={}", dep_name, new_version);
        re.replace_all(content, replacement.as_str()).to_string()
    }
}

impl PackageManifest for CabalManifest {
    fn manager(&self) -> PackageManager {
        PackageManager::Cabal
    }

    fn manifest_name(&self) -> &str {
        "*.cabal"
    }

    fn detect(&self, path: &Path) -> bool {
        Self::find_cabal_file(path).is_some()
    }

    fn find_manifest(&self, path: &Path) -> Option<PathBuf> {
        Self::find_cabal_file(path)
    }

    fn parse(&self, path: &Path) -> PackageResult<Package> {
        let (cabal_path, content) = Self::read_manifest(path)?;

        let name = Self::get_field(&content, "name").ok_or_else(|| PackageError::ParseError {
            path: cabal_path.clone(),
            message: "Missing 'name' field".to_string(),
        })?;

        let version = Self::get_field(&content, "version").unwrap_or_else(|| "0.0.0.0".to_string());

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(Package {
            name,
            dir_name,
            version,
            path: path.to_path_buf(),
            manager: PackageManager::Cabal,
            workspace_deps: vec![], // Filled in by scan_workspace
        })
    }

    fn all_dependencies(&self, path: &Path) -> PackageResult<Vec<String>> {
        let (_, content) = Self::read_manifest(path)?;
        Ok(Self::parse_build_depends(&content))
    }

    fn set_dependency_version(
        &self,
        path: &Path,
        dep_name: &str,
        new_version: &str,
    ) -> PackageResult<()> {
        let (cabal_path, content) = Self::read_manifest(path)?;
        let new_content = Self::update_dependency_in_content(&content, dep_name, new_version);

        std::fs::write(&cabal_path, new_content).map_err(|e| PackageError::WriteError {
            path: cabal_path,
            source: e,
        })
    }

    fn set_package_version(&self, path: &Path, new_version: &str) -> PackageResult<()> {
        let (cabal_path, content) = Self::read_manifest(path)?;
        let new_content = Self::set_field(&content, "version", new_version);

        std::fs::write(&cabal_path, new_content).map_err(|e| PackageError::WriteError {
            path: cabal_path,
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE_CABAL: &str = r#"cabal-version:      3.0
name:               my-package
version:            0.1.0.0
license:            MIT
author:             Test

library
    exposed-modules:  MyLib
    build-depends:    base >=4.7 && <5,
                      text,
                      aeson >=1.0
    default-language: Haskell2010
"#;

    #[test]
    fn test_detect_cabal() {
        let tmp = TempDir::new().unwrap();
        let manifest = CabalManifest;

        // No .cabal file
        assert!(!manifest.detect(tmp.path()));

        // With .cabal file
        std::fs::write(tmp.path().join("test.cabal"), SAMPLE_CABAL).unwrap();
        assert!(manifest.detect(tmp.path()));
    }

    #[test]
    fn test_parse_cabal() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("my-package.cabal"), SAMPLE_CABAL).unwrap();

        let manifest = CabalManifest;
        let pkg = manifest.parse(tmp.path()).unwrap();

        assert_eq!(pkg.name, "my-package");
        assert_eq!(pkg.version, "0.1.0.0");
        assert_eq!(pkg.manager, PackageManager::Cabal);
    }

    #[test]
    fn test_all_dependencies() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("my-package.cabal"), SAMPLE_CABAL).unwrap();

        let manifest = CabalManifest;
        let deps = manifest.all_dependencies(tmp.path()).unwrap();

        assert!(deps.contains(&"base".to_string()));
        assert!(deps.contains(&"text".to_string()));
        assert!(deps.contains(&"aeson".to_string()));
    }

    #[test]
    fn test_set_package_version() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("my-package.cabal"), SAMPLE_CABAL).unwrap();

        let manifest = CabalManifest;
        manifest.set_package_version(tmp.path(), "2.0.0.0").unwrap();

        let pkg = manifest.parse(tmp.path()).unwrap();
        assert_eq!(pkg.version, "2.0.0.0");
    }

    #[test]
    fn test_get_field() {
        assert_eq!(CabalManifest::get_field(SAMPLE_CABAL, "name"), Some("my-package".to_string()));
        assert_eq!(CabalManifest::get_field(SAMPLE_CABAL, "version"), Some("0.1.0.0".to_string()));
        assert_eq!(CabalManifest::get_field(SAMPLE_CABAL, "nonexistent"), None);
    }

    #[test]
    fn test_parse_build_depends() {
        let deps = CabalManifest::parse_build_depends(SAMPLE_CABAL);
        assert!(deps.contains(&"base".to_string()));
        assert!(deps.contains(&"text".to_string()));
        assert!(deps.contains(&"aeson".to_string()));
    }
}
