//! Cargo (Rust) build system detection and manifest parsing

use std::path::Path;

use super::DepRef;

/// Check if a directory contains a Cargo.toml
pub fn is_cargo_project(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
}

/// Parse the package name from Cargo.toml
pub fn cargo_package_name(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;
    doc.get("package")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

/// Parse the package version from Cargo.toml
pub fn cargo_package_version(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;
    doc.get("package")?
        .get("version")?
        .as_str()
        .map(|s| s.to_string())
}

/// Parse dependencies from Cargo.toml
///
/// Reads both `[dependencies]` and `[dev-dependencies]` sections.
/// Handles both simple version strings and table-form deps.
pub fn parse_cargo_deps(path: &Path) -> Vec<DepRef> {
    let cargo_path = path.join("Cargo.toml");
    let content = match std::fs::read_to_string(&cargo_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let doc: toml::Value = match toml::from_str(&content) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();

    // Parse [dependencies]
    if let Some(dep_table) = doc.get("dependencies").and_then(|v| v.as_table()) {
        deps.extend(parse_dep_table(dep_table));
    }

    // Parse [dev-dependencies]
    if let Some(dep_table) = doc.get("dev-dependencies").and_then(|v| v.as_table()) {
        deps.extend(parse_dep_table(dep_table));
    }

    // Parse [build-dependencies]
    if let Some(dep_table) = doc.get("build-dependencies").and_then(|v| v.as_table()) {
        deps.extend(parse_dep_table(dep_table));
    }

    deps
}

fn parse_dep_table(table: &toml::map::Map<String, toml::Value>) -> Vec<DepRef> {
    let mut deps = Vec::new();

    for (name, value) in table {
        match value {
            // Simple form: dep = "1.0"
            toml::Value::String(version) => {
                deps.push(DepRef {
                    name: name.clone(),
                    version_req: Some(version.clone()),
                    is_path_dep: false,
                    path: None,
                });
            }
            // Table form: dep = { version = "1.0", path = "../dep", ... }
            toml::Value::Table(t) => {
                let version = t.get("version").and_then(|v| v.as_str()).map(|s| s.to_string());
                let dep_path = t.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
                let is_path = dep_path.is_some();

                deps.push(DepRef {
                    name: name.clone(),
                    version_req: version,
                    is_path_dep: is_path,
                    path: dep_path,
                });
            }
            _ => {}
        }
    }

    deps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_is_cargo_project() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_cargo_project(tmp.path()));

        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
        assert!(is_cargo_project(tmp.path()));
    }

    #[test]
    fn test_cargo_package_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert_eq!(cargo_package_name(tmp.path()), Some("my-crate".to_string()));
    }

    #[test]
    fn test_parse_simple_deps() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = "1"
"#,
        )
        .unwrap();

        let deps = parse_cargo_deps(tmp.path());
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.name == "serde" && d.version_req == Some("1.0".to_string())));
        assert!(deps.iter().any(|d| d.name == "tokio" && !d.is_path_dep));
    }

    #[test]
    fn test_parse_table_deps() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
local-crate = { path = "../local-crate", version = "0.1.0" }
path-only = { path = "../path-only" }
"#,
        )
        .unwrap();

        let deps = parse_cargo_deps(tmp.path());
        assert_eq!(deps.len(), 3);

        let serde_dep = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde_dep.is_path_dep);
        assert_eq!(serde_dep.version_req, Some("1.0".to_string()));

        let local = deps.iter().find(|d| d.name == "local-crate").unwrap();
        assert!(local.is_path_dep);
        assert_eq!(local.path, Some("../local-crate".to_string()));
        assert_eq!(local.version_req, Some("0.1.0".to_string()));

        let path_only = deps.iter().find(|d| d.name == "path-only").unwrap();
        assert!(path_only.is_path_dep);
        assert_eq!(path_only.version_req, None);
    }

    #[test]
    fn test_parse_dev_deps() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"

[dev-dependencies]
tempfile = "3"
"#,
        )
        .unwrap();

        let deps = parse_cargo_deps(tmp.path());
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.name == "tempfile"));
    }
}
