//! Node.js build system detection and manifest parsing

use std::path::Path;

use super::DepRef;

/// Check if a directory contains a package.json
pub fn is_node_project(path: &Path) -> bool {
    path.join("package.json").exists()
}

/// Parse the package name from package.json
pub fn node_package_name(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path.join("package.json")).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&content).ok()?;
    doc.get("name")?.as_str().map(|s| s.to_string())
}

/// Parse the package version from package.json
pub fn node_package_version(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path.join("package.json")).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&content).ok()?;
    doc.get("version")?.as_str().map(|s| s.to_string())
}

/// Parse dependencies from package.json
///
/// Reads `dependencies`, `devDependencies`, and `peerDependencies`.
pub fn parse_node_deps(path: &Path) -> Vec<DepRef> {
    let pkg_path = path.join("package.json");
    let content = match std::fs::read_to_string(&pkg_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let doc: serde_json::Value = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();

    for section in &["dependencies", "devDependencies", "peerDependencies"] {
        let is_dev = *section == "devDependencies";
        if let Some(obj) = doc.get(section).and_then(|v| v.as_object()) {
            for (name, value) in obj {
                let version_str = value.as_str().unwrap_or("*").to_string();
                let is_path = version_str.starts_with("file:")
                    || version_str.starts_with("link:");

                let path = if is_path {
                    Some(
                        version_str
                            .trim_start_matches("file:")
                            .trim_start_matches("link:")
                            .to_string(),
                    )
                } else {
                    None
                };

                deps.push(DepRef {
                    name: name.clone(),
                    version_req: Some(version_str),
                    is_path_dep: is_path,
                    path,
                    is_dev,
                });
            }
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
    fn test_is_node_project() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_node_project(tmp.path()));

        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "test", "version": "1.0.0"}"#,
        )
        .unwrap();
        assert!(is_node_project(tmp.path()));
    }

    #[test]
    fn test_node_package_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "@scope/my-package", "version": "1.0.0"}"#,
        )
        .unwrap();
        assert_eq!(
            node_package_name(tmp.path()),
            Some("@scope/my-package".to_string())
        );
    }

    #[test]
    fn test_parse_node_deps() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{
                "name": "test",
                "version": "1.0.0",
                "dependencies": {
                    "express": "^4.18.0",
                    "lodash": "~4.17.21"
                },
                "devDependencies": {
                    "jest": "^29.0.0",
                    "local-pkg": "file:../local-pkg"
                }
            }"#,
        )
        .unwrap();

        let deps = parse_node_deps(tmp.path());
        assert_eq!(deps.len(), 4);

        let express = deps.iter().find(|d| d.name == "express").unwrap();
        assert_eq!(express.version_req, Some("^4.18.0".to_string()));
        assert!(!express.is_path_dep);

        let local = deps.iter().find(|d| d.name == "local-pkg").unwrap();
        assert!(local.is_path_dep);
        assert_eq!(local.path, Some("../local-pkg".to_string()));
    }
}
