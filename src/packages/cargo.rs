//! Cargo (Rust) package manifest handler

use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value};

use super::{Package, PackageError, PackageManifest, PackageManager, PackageResult};

/// Handler for Cargo.toml manifests
pub struct CargoManifest;

impl CargoManifest {
    fn read_manifest(path: &Path) -> PackageResult<(PathBuf, String)> {
        let manifest_path = path.join("Cargo.toml");
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| PackageError::ReadError {
            path: manifest_path.clone(),
            source: e,
        })?;
        Ok((manifest_path, content))
    }

    fn parse_document(path: &Path, content: &str) -> PackageResult<DocumentMut> {
        content.parse::<DocumentMut>().map_err(|e| PackageError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    fn write_manifest(path: &Path, doc: &DocumentMut) -> PackageResult<()> {
        let manifest_path = path.join("Cargo.toml");
        std::fs::write(&manifest_path, doc.to_string()).map_err(|e| PackageError::WriteError {
            path: manifest_path,
            source: e,
        })
    }

    fn extract_dep_names(deps: &Item) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(table) = deps.as_table_like() {
            for (name, _) in table.iter() {
                names.push(name.to_string());
            }
        }
        names
    }
}

impl PackageManifest for CargoManifest {
    fn manager(&self) -> PackageManager {
        PackageManager::Cargo
    }

    fn manifest_name(&self) -> &str {
        "Cargo.toml"
    }

    fn detect(&self, path: &Path) -> bool {
        path.join("Cargo.toml").exists()
    }

    fn find_manifest(&self, path: &Path) -> Option<PathBuf> {
        let manifest = path.join("Cargo.toml");
        if manifest.exists() {
            Some(manifest)
        } else {
            None
        }
    }

    fn parse(&self, path: &Path) -> PackageResult<Package> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let doc = Self::parse_document(&manifest_path, &content)?;

        // Get package name and version
        let package = doc.get("package").ok_or_else(|| PackageError::ParseError {
            path: manifest_path.clone(),
            message: "Missing [package] section".to_string(),
        })?;

        let name = package
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PackageError::ParseError {
                path: manifest_path.clone(),
                message: "Missing package.name".to_string(),
            })?
            .to_string();

        let version = package
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();

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
            manager: PackageManager::Cargo,
            workspace_deps: vec![], // Filled in by scan_workspace
        })
    }

    fn all_dependencies(&self, path: &Path) -> PackageResult<Vec<String>> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let doc = Self::parse_document(&manifest_path, &content)?;

        let mut deps = Vec::new();

        // Regular dependencies (required for publish order)
        if let Some(d) = doc.get("dependencies") {
            deps.extend(Self::extract_dep_names(d));
        }

        // Build dependencies (required for publish order - needed to compile)
        if let Some(d) = doc.get("build-dependencies") {
            deps.extend(Self::extract_dep_names(d));
        }

        // NOTE: dev-dependencies are NOT included - they don't affect publish order
        // since they're only used for testing, not compilation

        Ok(deps)
    }

    fn set_dependency_version(
        &self,
        path: &Path,
        dep_name: &str,
        new_version: &str,
    ) -> PackageResult<()> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let mut doc = Self::parse_document(&manifest_path, &content)?;

        let sections = ["dependencies", "dev-dependencies", "build-dependencies"];

        for section in sections {
            if let Some(deps) = doc.get_mut(section) {
                if let Some(table) = deps.as_table_like_mut() {
                    if let Some(dep) = table.get_mut(dep_name) {
                        match dep {
                            Item::Value(Value::String(_)) => {
                                // Simple version string: dep = "1.0"
                                *dep = Item::Value(new_version.into());
                            }
                            Item::Value(Value::InlineTable(t)) => {
                                // Inline table: dep = { version = "1.0", ... }
                                if t.contains_key("version") {
                                    t.insert("version", new_version.into());
                                }
                            }
                            Item::Table(t) => {
                                // Table: [dependencies.dep] version = "1.0"
                                t["version"] = toml_edit::value(new_version);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Self::write_manifest(path, &doc)
    }

    fn set_package_version(&self, path: &Path, new_version: &str) -> PackageResult<()> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let mut doc = Self::parse_document(&manifest_path, &content)?;

        if let Some(package) = doc.get_mut("package") {
            if let Some(table) = package.as_table_like_mut() {
                table.insert("version", toml_edit::value(new_version));
            }
        }

        Self::write_manifest(path, &doc)
    }

    fn update_path_dependencies(
        &self,
        path: &Path,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> PackageResult<Vec<PathBuf>> {
        let mut modified = Vec::new();

        // Update Cargo.toml
        if self.update_cargo_toml_paths(path, old_dir_name, new_dir_name)? {
            modified.push(path.join("Cargo.toml"));
        }

        // Update .cargo/config.toml if it exists
        if self.update_cargo_config_paths(path, old_dir_name, new_dir_name)? {
            modified.push(path.join(".cargo").join("config.toml"));
        }

        Ok(modified)
    }
}

impl CargoManifest {
    /// Update path dependencies in Cargo.toml
    fn update_cargo_toml_paths(
        &self,
        path: &Path,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> PackageResult<bool> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let mut doc = Self::parse_document(&manifest_path, &content)?;

        let sections = ["dependencies", "dev-dependencies", "build-dependencies"];
        let mut changed = false;

        for section in sections {
            if let Some(deps) = doc.get_mut(section) {
                if Self::update_paths_in_table(deps, old_dir_name, new_dir_name) {
                    changed = true;
                }
            }
        }

        if changed {
            Self::write_manifest(path, &doc)?;
        }

        Ok(changed)
    }

    /// Update path dependencies in .cargo/config.toml
    fn update_cargo_config_paths(
        &self,
        path: &Path,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> PackageResult<bool> {
        let config_path = path.join(".cargo").join("config.toml");
        if !config_path.exists() {
            return Ok(false);
        }

        let content = std::fs::read_to_string(&config_path).map_err(|e| PackageError::ReadError {
            path: config_path.clone(),
            source: e,
        })?;

        let mut doc = content.parse::<DocumentMut>().map_err(|e| PackageError::ParseError {
            path: config_path.clone(),
            message: e.to_string(),
        })?;

        let mut changed = false;

        // Check [patch.crates-io] section
        if let Some(patch) = doc.get_mut("patch") {
            if let Some(patch_table) = patch.as_table_like_mut() {
                for (_registry, registry_patches) in patch_table.iter_mut() {
                    if Self::update_paths_in_table(registry_patches, old_dir_name, new_dir_name) {
                        changed = true;
                    }
                }
            }
        }

        // Check [paths] section (cargo path overrides)
        if let Some(paths) = doc.get_mut("paths") {
            if Self::update_paths_in_table(paths, old_dir_name, new_dir_name) {
                changed = true;
            }
        }

        if changed {
            std::fs::write(&config_path, doc.to_string()).map_err(|e| PackageError::WriteError {
                path: config_path,
                source: e,
            })?;
        }

        Ok(changed)
    }

    /// Update path values in a TOML table (works for deps and patches)
    fn update_paths_in_table(table: &mut Item, old_dir_name: &str, new_dir_name: &str) -> bool {
        let mut changed = false;

        if let Some(table) = table.as_table_like_mut() {
            for (_name, item) in table.iter_mut() {
                let path_updated = match item {
                    Item::Value(Value::InlineTable(t)) => {
                        Self::update_path_in_inline_table(t, old_dir_name, new_dir_name)
                    }
                    Item::Table(t) => {
                        Self::update_path_in_table(t, old_dir_name, new_dir_name)
                    }
                    _ => false,
                };
                if path_updated {
                    changed = true;
                }
            }
        }

        changed
    }

    /// Update path value in an inline table like { path = "..." }
    fn update_path_in_inline_table(
        t: &mut toml_edit::InlineTable,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> bool {
        if let Some(path_val) = t.get_mut("path") {
            if let Some(path_str) = path_val.as_str() {
                if let Some(new_path) = Self::replace_dir_in_path(path_str, old_dir_name, new_dir_name) {
                    *path_val = new_path.into();
                    return true;
                }
            }
        }
        false
    }

    /// Update path value in a table like [dependencies.foo] path = "..."
    fn update_path_in_table(
        t: &mut toml_edit::Table,
        old_dir_name: &str,
        new_dir_name: &str,
    ) -> bool {
        if let Some(path_item) = t.get_mut("path") {
            if let Some(path_str) = path_item.as_str() {
                if let Some(new_path) = Self::replace_dir_in_path(path_str, old_dir_name, new_dir_name) {
                    *path_item = toml_edit::value(new_path);
                    return true;
                }
            }
        }
        false
    }

    /// Replace old directory name with new one in a path string
    /// Returns Some(new_path) if changed, None if no change needed
    fn replace_dir_in_path(path_str: &str, old_dir_name: &str, new_dir_name: &str) -> Option<String> {
        if !path_str.contains(old_dir_name) {
            return None;
        }

        let new_path = path_str
            .replace(&format!("/{}", old_dir_name), &format!("/{}", new_dir_name))
            .replace(&format!("../{}", old_dir_name), &format!("../{}", new_dir_name));

        if new_path != path_str {
            Some(new_path)
        } else {
            None
        }
    }

    /// Setup patches: convert path deps to version-only + add [patch.crates-io]
    ///
    /// This transforms:
    ///   plexus-core = { path = "../plexus-core", version = "0.2.1" }
    /// Into:
    ///   plexus-core = "0.2.1"
    ///   [patch.crates-io]
    ///   plexus-core = { path = "../plexus-core" }
    ///
    /// The package_map contains: name -> (version, absolute_path)
    pub fn setup_patches(
        path: &Path,
        package_map: &std::collections::HashMap<String, (String, PathBuf)>,
    ) -> PackageResult<super::SetupPatchesResult> {
        let (manifest_path, content) = Self::read_manifest(path)?;
        let mut doc = Self::parse_document(&manifest_path, &content)?;
        let mut patches_needed: Vec<(String, String)> = Vec::new(); // (name, rel_path)

        // Process each dependency section
        for section in ["dependencies", "build-dependencies", "dev-dependencies"] {
            if let Some(deps) = doc.get_mut(section) {
                if let Some(table) = deps.as_table_like_mut() {
                    let dep_names: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();

                    for name in dep_names {
                        if let Some(dep) = table.get_mut(&name) {
                            // Check if this is a path dep pointing to a known package
                            if let Some(dep_path_str) = Self::extract_path_from_dep(dep) {
                                // Check if this package is in our workspace
                                if let Some((version, pkg_abs_path)) = package_map.get(&name) {
                                    // Calculate relative path from this repo to the dep
                                    let rel_path = Self::calculate_relative_path(path, pkg_abs_path);

                                    // Convert dep to version-only
                                    Self::convert_dep_to_version(dep, version);

                                    // Remember we need a patch for this (avoid duplicates)
                                    if !patches_needed.iter().any(|(n, _)| n == &name) {
                                        patches_needed.push((name.clone(), rel_path));
                                    }
                                } else {
                                    // External path dep (e.g., cllient) - leave as-is
                                    // These need to stay as path deps until they're on crates.io
                                    let _ = dep_path_str;
                                }
                            }
                        }
                    }
                }
            }
        }

        let modified = !patches_needed.is_empty();

        // Add/update [patch.crates-io] section if we have patches
        if !patches_needed.is_empty() {
            Self::add_patch_crates_io_section(&mut doc, &patches_needed);
        }

        // Write back
        if modified {
            Self::write_manifest(path, &doc)?;
        }

        Ok(super::SetupPatchesResult {
            patches_added: patches_needed,
            modified,
        })
    }

    /// Extract path value from a dependency item
    fn extract_path_from_dep(dep: &Item) -> Option<String> {
        match dep {
            Item::Value(Value::InlineTable(t)) => {
                t.get("path").and_then(|v| v.as_str()).map(|s| s.to_string())
            }
            Item::Table(t) => t.get("path").and_then(|v| v.as_str()).map(|s| s.to_string()),
            _ => None,
        }
    }

    /// Convert a path+version dep to version-only
    /// plexus-core = { path = "../plexus-core", version = "0.2.1" }
    /// becomes:
    /// plexus-core = "0.2.1"
    fn convert_dep_to_version(dep: &mut Item, version: &str) {
        match dep {
            Item::Value(Value::InlineTable(t)) => {
                // Remove path
                t.remove("path");
                // Get existing version or use provided
                let ver = t
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| version.to_string());
                // If only version remains (or was added), simplify to string
                if t.len() == 1 && t.contains_key("version") {
                    *dep = Item::Value(Value::String(toml_edit::Formatted::new(ver)));
                } else if !t.contains_key("version") {
                    t.insert("version", ver.into());
                }
            }
            Item::Table(t) => {
                t.remove("path");
                if !t.contains_key("version") {
                    t["version"] = toml_edit::value(version);
                }
            }
            _ => {}
        }
    }

    /// Add or update [patch.crates-io] section
    fn add_patch_crates_io_section(doc: &mut DocumentMut, patches: &[(String, String)]) {
        // Get or create [patch] table
        if doc.get("patch").is_none() {
            doc["patch"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        let patch = doc
            .get_mut("patch")
            .and_then(|p| p.as_table_mut())
            .expect("patch table should exist");

        // Get or create [patch.crates-io] table
        if patch.get("crates-io").is_none() {
            patch["crates-io"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        let crates_io = patch
            .get_mut("crates-io")
            .and_then(|c| c.as_table_mut())
            .expect("crates-io table should exist");

        // Add each patch (only if not already present)
        for (name, rel_path) in patches {
            if crates_io.get(name).is_none() {
                let mut inline = toml_edit::InlineTable::new();
                inline.insert("path", rel_path.clone().into());
                crates_io.insert(name, toml_edit::value(inline));
            }
        }
    }

    /// Calculate relative path from one directory to another
    fn calculate_relative_path(from_dir: &Path, to_dir: &Path) -> String {
        // Both paths should be absolute; we want the relative path from from_dir to to_dir
        // Simple case: both are siblings in the same parent
        if let (Some(from_parent), Some(to_name)) =
            (from_dir.parent(), to_dir.file_name().and_then(|n| n.to_str()))
        {
            if let Some(to_parent) = to_dir.parent() {
                if from_parent == to_parent {
                    // Siblings - simple ../name
                    return format!("../{}", to_name);
                }
            }
        }

        // More complex: try to use pathdiff if available, otherwise fall back to ../name
        // For workspace layouts, packages are typically siblings
        if let Some(to_name) = to_dir.file_name().and_then(|n| n.to_str()) {
            format!("../{}", to_name)
        } else {
            // Fallback to absolute path (shouldn't happen in practice)
            to_dir.to_string_lossy().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_cargo() {
        let tmp = TempDir::new().unwrap();
        let manifest = CargoManifest;

        // No Cargo.toml
        assert!(!manifest.detect(tmp.path()));

        // With Cargo.toml
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"\nversion = \"1.0.0\"").unwrap();
        assert!(manifest.detect(tmp.path()));
    }

    #[test]
    fn test_parse_cargo() {
        let tmp = TempDir::new().unwrap();
        let content = r#"
[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1.0", features = ["full"] }
"#;
        std::fs::write(tmp.path().join("Cargo.toml"), content).unwrap();

        let manifest = CargoManifest;
        let pkg = manifest.parse(tmp.path()).unwrap();

        assert_eq!(pkg.name, "my-crate");
        assert_eq!(pkg.version, "0.1.0");
        assert_eq!(pkg.manager, PackageManager::Cargo);
    }

    #[test]
    fn test_all_dependencies() {
        let tmp = TempDir::new().unwrap();
        let content = r#"
[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1.0" }

[build-dependencies]
cc = "1.0"

[dev-dependencies]
tempfile = "3.0"
"#;
        std::fs::write(tmp.path().join("Cargo.toml"), content).unwrap();

        let manifest = CargoManifest;
        let deps = manifest.all_dependencies(tmp.path()).unwrap();

        // Regular dependencies included
        assert!(deps.contains(&"serde".to_string()));
        assert!(deps.contains(&"tokio".to_string()));
        // Build dependencies included (needed to compile)
        assert!(deps.contains(&"cc".to_string()));
        // Dev dependencies NOT included (don't affect publish order)
        assert!(!deps.contains(&"tempfile".to_string()));
    }

    #[test]
    fn test_set_package_version() {
        let tmp = TempDir::new().unwrap();
        let content = r#"
[package]
name = "my-crate"
version = "0.1.0"
"#;
        std::fs::write(tmp.path().join("Cargo.toml"), content).unwrap();

        let manifest = CargoManifest;
        manifest.set_package_version(tmp.path(), "2.0.0").unwrap();

        let pkg = manifest.parse(tmp.path()).unwrap();
        assert_eq!(pkg.version, "2.0.0");
    }

    #[test]
    fn test_set_dependency_version() {
        let tmp = TempDir::new().unwrap();
        let content = r#"
[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
other-crate = "1.0.0"
"#;
        std::fs::write(tmp.path().join("Cargo.toml"), content).unwrap();

        let manifest = CargoManifest;
        manifest.set_dependency_version(tmp.path(), "other-crate", "2.0.0").unwrap();

        let new_content = std::fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(new_content.contains("\"2.0.0\""));
    }

    #[test]
    fn test_setup_patches() {
        let tmp = TempDir::new().unwrap();

        // Create package A with path dep to package B
        let pkg_a = tmp.path().join("pkg-a");
        let pkg_b = tmp.path().join("pkg-b");
        std::fs::create_dir_all(&pkg_a).unwrap();
        std::fs::create_dir_all(&pkg_b).unwrap();

        // Package B
        std::fs::write(
            pkg_b.join("Cargo.toml"),
            r#"
[package]
name = "pkg-b"
version = "1.0.0"
"#,
        )
        .unwrap();

        // Package A with path+version dep to B
        std::fs::write(
            pkg_a.join("Cargo.toml"),
            r#"
[package]
name = "pkg-a"
version = "1.0.0"

[dependencies]
pkg-b = { path = "../pkg-b", version = "1.0.0" }
"#,
        )
        .unwrap();

        // Build package map
        let mut package_map = std::collections::HashMap::new();
        package_map.insert("pkg-b".to_string(), ("1.0.0".to_string(), pkg_b.clone()));

        // Run setup_patches on pkg-a
        let result = CargoManifest::setup_patches(&pkg_a, &package_map).unwrap();

        assert!(result.modified);
        assert_eq!(result.patches_added.len(), 1);
        assert_eq!(result.patches_added[0].0, "pkg-b");

        // Verify the output
        let new_content = std::fs::read_to_string(pkg_a.join("Cargo.toml")).unwrap();

        // Should have version-only dep
        assert!(new_content.contains("pkg-b = \"1.0.0\""));

        // Should have patch section
        assert!(new_content.contains("[patch.crates-io]"));
        assert!(new_content.contains("pkg-b = { path ="));
    }

    #[test]
    fn test_setup_patches_with_features() {
        let tmp = TempDir::new().unwrap();

        // Create packages
        let pkg_a = tmp.path().join("pkg-a");
        let pkg_b = tmp.path().join("pkg-b");
        std::fs::create_dir_all(&pkg_a).unwrap();
        std::fs::create_dir_all(&pkg_b).unwrap();

        std::fs::write(
            pkg_b.join("Cargo.toml"),
            r#"
[package]
name = "pkg-b"
version = "2.0.0"
"#,
        )
        .unwrap();

        // Package A with path+version dep including features
        std::fs::write(
            pkg_a.join("Cargo.toml"),
            r#"
[package]
name = "pkg-a"
version = "1.0.0"

[dependencies]
pkg-b = { path = "../pkg-b", version = "2.0.0", features = ["async"] }
"#,
        )
        .unwrap();

        let mut package_map = std::collections::HashMap::new();
        package_map.insert("pkg-b".to_string(), ("2.0.0".to_string(), pkg_b.clone()));

        let result = CargoManifest::setup_patches(&pkg_a, &package_map).unwrap();

        assert!(result.modified);
        assert_eq!(result.patches_added.len(), 1);

        let new_content = std::fs::read_to_string(pkg_a.join("Cargo.toml")).unwrap();

        // Should keep features but remove path
        assert!(new_content.contains("features"));
        assert!(new_content.contains("async"));
        // Should have patch section
        assert!(new_content.contains("[patch.crates-io]"));
    }

    #[test]
    fn test_setup_patches_external_dep_unchanged() {
        let tmp = TempDir::new().unwrap();

        let pkg_a = tmp.path().join("pkg-a");
        std::fs::create_dir_all(&pkg_a).unwrap();

        // Package A with external path dep (not in package_map)
        std::fs::write(
            pkg_a.join("Cargo.toml"),
            r#"
[package]
name = "pkg-a"
version = "1.0.0"

[dependencies]
external-crate = { path = "../../external/crate" }
"#,
        )
        .unwrap();

        // Empty package map - no workspace packages
        let package_map = std::collections::HashMap::new();

        let result = CargoManifest::setup_patches(&pkg_a, &package_map).unwrap();

        // No modifications since external-crate isn't in workspace
        assert!(!result.modified);
        assert!(result.patches_added.is_empty());

        let new_content = std::fs::read_to_string(pkg_a.join("Cargo.toml")).unwrap();

        // External dep should remain unchanged
        assert!(new_content.contains("path = \"../../external/crate\""));
        // No patch section should be added
        assert!(!new_content.contains("[patch.crates-io]"));
    }
}
