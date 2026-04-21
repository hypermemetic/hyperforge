//! Cargo (Rust) build system detection and manifest parsing

use std::path::Path;
use std::process::Command;

use super::{BinaryTarget, BuildSystemKind, DepRef};

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
        .map(std::string::ToString::to_string)
}

/// Parse the package version from Cargo.toml
pub fn cargo_package_version(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;
    doc.get("package")?
        .get("version")?
        .as_str()
        .map(std::string::ToString::to_string)
}

/// Detect binary targets from Cargo.toml
///
/// Finds binaries by:
/// 1. Explicit `[[bin]]` sections with `name` fields
/// 2. Implicit binary: if `src/main.rs` exists and no `[[bin]]` sections, the package name is used
/// 3. Workspace members: recurses into workspace member directories for their binaries
pub fn cargo_binary_targets(path: &Path) -> Vec<BinaryTarget> {
    let cargo_path = path.join("Cargo.toml");
    let content = match std::fs::read_to_string(&cargo_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let doc: toml::Value = match toml::from_str(&content) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut targets = Vec::new();
    let repo_path = path.to_path_buf();

    // Check for explicit [[bin]] sections
    let explicit_bins: Vec<String> = doc
        .get("bin")
        .and_then(|v| v.as_array())
        .map(|bins| {
            bins.iter()
                .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(std::string::ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    if !explicit_bins.is_empty() {
        for name in explicit_bins {
            targets.push(BinaryTarget {
                name,
                build_system: BuildSystemKind::Cargo,
                repo_path: repo_path.clone(),
            });
        }
    } else if path.join("src/main.rs").exists() {
        // Implicit binary: package name is the binary name
        if let Some(pkg_name) = doc
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        {
            targets.push(BinaryTarget {
                name: pkg_name.to_string(),
                build_system: BuildSystemKind::Cargo,
                repo_path,
            });
        }
    }

    // Recurse into workspace members
    if let Some(members) = doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for member in members {
            if let Some(pattern) = member.as_str() {
                // Handle glob patterns like "crates/*" and direct paths like "my-crate"
                if pattern.contains('*') {
                    let base = path.join(pattern.trim_end_matches("/*").trim_end_matches("\\*"));
                    if let Ok(entries) = std::fs::read_dir(&base) {
                        for entry in entries.flatten() {
                            let member_path = entry.path();
                            if member_path.is_dir() && member_path.join("Cargo.toml").exists() {
                                targets.extend(cargo_binary_targets(&member_path));
                            }
                        }
                    }
                } else {
                    let member_path = path.join(pattern);
                    if member_path.is_dir() && member_path.join("Cargo.toml").exists() {
                        targets.extend(cargo_binary_targets(&member_path));
                    }
                }
            }
        }
    }

    targets
}

/// Parse dependencies from Cargo.toml
///
/// Reads `[dependencies]`, `[dev-dependencies]`, and `[build-dependencies]`.
/// Dev-dependencies are tagged with `is_dev = true`.
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
        deps.extend(parse_dep_table(dep_table, false));
    }

    // Parse [dev-dependencies]
    if let Some(dep_table) = doc.get("dev-dependencies").and_then(|v| v.as_table()) {
        deps.extend(parse_dep_table(dep_table, true));
    }

    // Parse [build-dependencies]
    if let Some(dep_table) = doc.get("build-dependencies").and_then(|v| v.as_table()) {
        deps.extend(parse_dep_table(dep_table, false));
    }

    deps
}

/// List files that would be included in a published crate.
///
/// Shells out to `cargo package --list` and parses the output.
/// Returns `None` if the command fails (e.g., not a publishable crate).
pub fn cargo_publishable_files(path: &Path) -> Option<Vec<String>> {
    let output = Command::new("cargo")
        .args(["package", "--list", "--allow-dirty"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if files.is_empty() { None } else { Some(files) }
}

fn parse_dep_table(table: &toml::map::Map<String, toml::Value>, is_dev: bool) -> Vec<DepRef> {
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
                    is_dev,
                });
            }
            // Table form: dep = { version = "1.0", path = "../dep", ... }
            toml::Value::Table(t) => {
                let version = t.get("version").and_then(|v| v.as_str()).map(std::string::ToString::to_string);
                let dep_path = t.get("path").and_then(|v| v.as_str()).map(std::string::ToString::to_string);
                let is_path = dep_path.is_some();

                deps.push(DepRef {
                    name: name.clone(),
                    version_req: version,
                    is_path_dep: is_path,
                    path: dep_path,
                    is_dev,
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
    fn test_cargo_binary_targets_explicit_bins() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "hyperforge"
version = "0.1.0"

[[bin]]
name = "hyperforge"
path = "src/main.rs"

[[bin]]
name = "hyperforge-auth"
path = "src/bin/auth.rs"

[[bin]]
name = "hyperforge-ssh"
path = "src/bin/ssh.rs"
"#,
        )
        .unwrap();

        let targets = cargo_binary_targets(tmp.path());
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().any(|t| t.name == "hyperforge"));
        assert!(targets.iter().any(|t| t.name == "hyperforge-auth"));
        assert!(targets.iter().any(|t| t.name == "hyperforge-ssh"));
        for t in &targets {
            assert_eq!(t.build_system, super::BuildSystemKind::Cargo);
            assert_eq!(t.repo_path, tmp.path().to_path_buf());
        }
    }

    #[test]
    fn test_cargo_binary_targets_implicit() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "my-tool"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let targets = cargo_binary_targets(tmp.path());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "my-tool");
    }

    #[test]
    fn test_cargo_binary_targets_lib_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "my-lib"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/lib.rs"), "pub fn hello() {}").unwrap();

        let targets = cargo_binary_targets(tmp.path());
        assert!(targets.is_empty());
    }

    #[test]
    fn test_cargo_binary_targets_workspace() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/*"]
"#,
        )
        .unwrap();

        // Create two member crates
        let crate_a = tmp.path().join("crates/app-a");
        fs::create_dir_all(crate_a.join("src")).unwrap();
        fs::write(
            crate_a.join("Cargo.toml"),
            "[package]\nname = \"app-a\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(crate_a.join("src/main.rs"), "fn main() {}").unwrap();

        let crate_b = tmp.path().join("crates/lib-b");
        fs::create_dir_all(crate_b.join("src")).unwrap();
        fs::write(
            crate_b.join("Cargo.toml"),
            "[package]\nname = \"lib-b\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(crate_b.join("src/lib.rs"), "").unwrap();

        let targets = cargo_binary_targets(tmp.path());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "app-a");
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
