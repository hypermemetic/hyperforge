//! Unified manifest parsing for Cargo.toml, package.json, pyproject.toml.
//!
//! Each parser yields a `PackageManifest` with the fields BuildHub
//! surfaces. Unknown / missing optional fields fall back to empty.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ManifestError {
    #[error("manifest file not found: {0}")]
    NotFound(String),
    #[error("manifest parse error in {file}: {message}")]
    ParseError { file: String, message: String },
    #[error("io error: {0}")]
    Io(String),
}

impl ManifestError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "manifest_not_found",
            Self::ParseError { .. } => "manifest_parse_error",
            Self::Io(_) => "io",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Dep {
    pub name: String,
    pub version: String,
    /// `cargo` | `npm` | `pypi` — which manifest kind this came from.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct PackageManifest {
    /// `cargo` | `npm` | `pypi`.
    pub kind: String,
    pub name: String,
    pub version: String,
    pub deps: Vec<Dep>,
}

/// Detect + parse the first manifest found in `dir`. Order of
/// preference: Cargo.toml, package.json, pyproject.toml.
pub fn detect_and_parse(dir: &Path) -> Result<PackageManifest, ManifestError> {
    let cargo = dir.join("Cargo.toml");
    if cargo.is_file() {
        return parse_cargo(&cargo);
    }
    // Cabal BEFORE npm — mirrors registry::detect_build_system. A
    // Haskell repo carrying an auxiliary package.json (e.g. a codegen
    // client config) must not be misread as an npm package, or the
    // publish tiering silently drops it (HF-PUBLISH-SAFETY finding:
    // synapse-cc's `@plexus/client` package.json shadowed its .cabal).
    if let Some(cabal) = find_cabal_file(dir) {
        return parse_cabal(&cabal);
    }
    let npm = dir.join("package.json");
    if npm.is_file() {
        return parse_npm(&npm);
    }
    let py = dir.join("pyproject.toml");
    if py.is_file() {
        return parse_pyproject(&py);
    }
    Err(ManifestError::NotFound(dir.display().to_string()))
}

fn find_cabal_file(dir: &Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.extension().and_then(|x| x.to_str()) == Some("cabal") && p.is_file())
            .then_some(p)
    })
}

/// Minimal `.cabal` parse: `name:` + `version:` top-level fields.
/// Dependencies are NOT extracted (cabal `build-depends` grammar is
/// section-scoped); Hackage packages therefore tier independently.
pub fn parse_cabal(path: &Path) -> Result<PackageManifest, ManifestError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ManifestError::Io(format!("{}: {e}", path.display())))?;
    let field = |key: &str| -> String {
        raw.lines()
            .find_map(|line| {
                let t = line.trim();
                let ok = t.len() > key.len()
                    && t[..key.len()].eq_ignore_ascii_case(key)
                    && t.as_bytes()[key.len()] == b':';
                if ok {
                    let v = t[key.len() + 1..].trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
                None
            })
            .unwrap_or_default()
    };
    let name = field("name");
    let version = field("version");
    if name.is_empty() || version.is_empty() {
        return Err(ManifestError::ParseError {
            file: path.display().to_string(),
            message: "missing name: or version: field".into(),
        });
    }
    Ok(PackageManifest { kind: "cabal".into(), name, version, deps: Vec::new() })
}

pub fn parse_cargo(path: &Path) -> Result<PackageManifest, ManifestError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ManifestError::Io(format!("{}: {e}", path.display())))?;
    parse_cargo_str(&raw, path)
}

pub fn parse_cargo_str(raw: &str, path: &Path) -> Result<PackageManifest, ManifestError> {
    let v: toml::Value = toml::from_str(raw)
        .map_err(|e| ManifestError::ParseError { file: path.display().to_string(), message: e.to_string() })?;
    let pkg = v.get("package").and_then(|t| t.as_table())
        .ok_or_else(|| ManifestError::ParseError {
            file: path.display().to_string(),
            message: "missing [package]".into(),
        })?;
    let name = pkg.get("name").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let version = pkg.get("version").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let mut deps: Vec<Dep> = Vec::new();
    if let Some(table) = v.get("dependencies").and_then(|t| t.as_table()) {
        for (k, val) in table {
            deps.push(Dep {
                name: k.clone(),
                version: dep_version(val),
                source: "cargo".into(),
            });
        }
    }
    Ok(PackageManifest { kind: "cargo".into(), name, version, deps })
}

fn dep_version(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Table(t) => t.get("version").and_then(|s| s.as_str()).unwrap_or("*").to_string(),
        _ => "*".into(),
    }
}

pub fn parse_npm(path: &Path) -> Result<PackageManifest, ManifestError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ManifestError::Io(format!("{}: {e}", path.display())))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ManifestError::ParseError { file: path.display().to_string(), message: e.to_string() })?;
    let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let version = v.get("version").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let mut deps: Vec<Dep> = Vec::new();
    for field in ["dependencies", "devDependencies"] {
        if let Some(obj) = v.get(field).and_then(|o| o.as_object()) {
            for (k, val) in obj {
                deps.push(Dep {
                    name: k.clone(),
                    version: val.as_str().unwrap_or("*").to_string(),
                    source: "npm".into(),
                });
            }
        }
    }
    Ok(PackageManifest { kind: "npm".into(), name, version, deps })
}

pub fn parse_pyproject(path: &Path) -> Result<PackageManifest, ManifestError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ManifestError::Io(format!("{}: {e}", path.display())))?;
    let v: toml::Value = toml::from_str(&raw)
        .map_err(|e| ManifestError::ParseError { file: path.display().to_string(), message: e.to_string() })?;
    // PEP 621: [project] block; fallback to [tool.poetry].
    let (name, version, deps_list) =
        if let Some(project) = v.get("project").and_then(|t| t.as_table()) {
            let n = project.get("name").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let ver = project.get("version").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let deps = project.get("dependencies")
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<_>>())
                .unwrap_or_default();
            (n, ver, deps)
        } else if let Some(p) = v.get("tool").and_then(|t| t.get("poetry")).and_then(|t| t.as_table()) {
            let n = p.get("name").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let ver = p.get("version").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let deps_tbl = p.get("dependencies").and_then(|t| t.as_table());
            let deps: Vec<String> = if let Some(d) = deps_tbl {
                d.iter().map(|(k, val)| format!("{}=={}", k, val.as_str().unwrap_or("*"))).collect()
            } else { Vec::new() };
            (n, ver, deps)
        } else {
            return Err(ManifestError::ParseError {
                file: path.display().to_string(),
                message: "missing [project] or [tool.poetry]".into(),
            });
        };
    let deps: Vec<Dep> = deps_list.into_iter().map(|spec| {
        // Split "name==ver" or "name>=ver" best-effort; default to spec as name.
        let (n, v) = match spec.split_once("==")
            .or_else(|| spec.split_once(">="))
            .or_else(|| spec.split_once("~="))
            .or_else(|| spec.split_once('>')) {
            Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
            None => (spec.trim().to_string(), "*".to_string()),
        };
        Dep { name: n, version: v, source: "pypi".into() }
    }).collect();
    Ok(PackageManifest { kind: "pypi".into(), name, version, deps })
}

/// Analyze a set of manifests for cross-repo anomalies. Returns a list
/// of findings; callers decide how to emit them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// Two manifests declare the same package name with different versions.
    DuplicateName { name: String, versions: Vec<String> },
    /// Two manifests declare a shared dependency at different versions.
    VersionMismatch { dep: String, versions: Vec<String> },
}

pub fn analyze(manifests: &[PackageManifest]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    // Duplicate names.
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for m in manifests {
        if m.name.is_empty() { continue; }
        by_name.entry(&m.name).or_default().push(&m.version);
    }
    for (name, versions) in by_name {
        let unique: std::collections::BTreeSet<&&str> = versions.iter().collect();
        if versions.len() > 1 && unique.len() > 1 {
            findings.push(Finding::DuplicateName {
                name: name.to_string(),
                versions: versions.into_iter().map(String::from).collect(),
            });
        }
    }
    // Cross-manifest dep version mismatches.
    let mut deps: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for m in manifests {
        for d in &m.deps {
            deps.entry(&d.name).or_default().push(&d.version);
        }
    }
    for (dep, versions) in deps {
        let unique: std::collections::BTreeSet<&&str> = versions.iter().collect();
        if unique.len() > 1 {
            findings.push(Finding::VersionMismatch {
                dep: dep.to_string(),
                versions: unique.into_iter().map(|s| (*s).to_string()).collect(),
            });
        }
    }
    findings
}

/// Topological sort of workspace packages into publish tiers.
///
/// Returns tiers of package names. Tier 0 has no in-workspace deps,
/// Tier 1 depends only on Tier 0, etc. Each tier can publish in parallel.
/// Returns an error if a dependency cycle is detected.
pub fn build_publish_order(manifests: &[PackageManifest]) -> Result<Vec<Vec<String>>, String> {
    // Collect workspace package names for filtering.
    let workspace_names: BTreeSet<&str> = manifests.iter().map(|m| m.name.as_str()).collect();

    // Build adjacency (deps) and in-degree maps.
    // Edge: dependency -> dependent (dep must publish before dependent).
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for m in manifests {
        in_degree.entry(&m.name).or_insert(0);
        for d in &m.deps {
            if workspace_names.contains(d.name.as_str()) && d.name != m.name {
                *in_degree.entry(&m.name).or_insert(0) += 1;
                dependents.entry(d.name.as_str()).or_default().push(&m.name);
            }
        }
    }

    // Kahn's algorithm with tier grouping.
    let mut tiers: Vec<Vec<String>> = Vec::new();
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    let mut processed = 0usize;
    let total = manifests.len();

    while !queue.is_empty() {
        // Drain current queue into one tier.
        let mut tier: Vec<String> = Vec::new();
        let tier_size = queue.len();
        for _ in 0..tier_size {
            let node = queue.pop_front().unwrap();
            tier.push(node.to_string());
            processed += 1;

            if let Some(deps) = dependents.get(node) {
                for &dep in deps {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
        tier.sort();
        tiers.push(tier);
    }

    if processed < total {
        // Cycle detected — report the remaining nodes.
        let stuck: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg > 0)
            .map(|(&name, _)| name.to_string())
            .collect();
        return Err(format!("dependency cycle detected among: {}", stuck.join(", ")));
    }

    Ok(tiers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_cargo_ok() {
        let raw = r#"
            [package]
            name = "foo"
            version = "0.1.2"
            [dependencies]
            serde = "1.0"
            tokio = { version = "1.30" }
        "#;
        let m = parse_cargo_str(raw, &PathBuf::from("<mem>")).unwrap();
        assert_eq!(m.name, "foo");
        assert_eq!(m.version, "0.1.2");
        assert_eq!(m.deps.len(), 2);
    }

    #[test]
    fn analyze_spots_version_mismatch() {
        let a = PackageManifest {
            kind: "cargo".into(), name: "a".into(), version: "0.1.0".into(),
            deps: vec![Dep { name: "serde".into(), version: "1.0.200".into(), source: "cargo".into() }],
        };
        let b = PackageManifest {
            kind: "cargo".into(), name: "b".into(), version: "0.1.0".into(),
            deps: vec![Dep { name: "serde".into(), version: "1.0.150".into(), source: "cargo".into() }],
        };
        let findings = analyze(&[a, b]);
        assert!(findings.iter().any(|f| matches!(f, Finding::VersionMismatch { dep, .. } if dep == "serde")));
    }

    fn pkg(name: &str, deps: &[&str]) -> PackageManifest {
        PackageManifest {
            kind: "cargo".into(),
            name: name.into(),
            version: "0.1.0".into(),
            deps: deps
                .iter()
                .map(|d| Dep {
                    name: (*d).into(),
                    version: "*".into(),
                    source: "cargo".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn test_topo_sort_linear() {
        // A depends on B depends on C
        let manifests = vec![pkg("A", &["B"]), pkg("B", &["C"]), pkg("C", &[])];
        let tiers = build_publish_order(&manifests).unwrap();
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0], vec!["C"]);
        assert_eq!(tiers[1], vec!["B"]);
        assert_eq!(tiers[2], vec!["A"]);
    }

    #[test]
    fn test_topo_sort_diamond() {
        // A→B, A→C, B→D, C→D
        let manifests = vec![
            pkg("A", &["B", "C"]),
            pkg("B", &["D"]),
            pkg("C", &["D"]),
            pkg("D", &[]),
        ];
        let tiers = build_publish_order(&manifests).unwrap();
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0], vec!["D"]);
        assert_eq!(tiers[1], vec!["B", "C"]);
        assert_eq!(tiers[2], vec!["A"]);
    }

    #[test]
    fn test_topo_sort_independent() {
        let manifests = vec![pkg("A", &[]), pkg("B", &[]), pkg("C", &[])];
        let tiers = build_publish_order(&manifests).unwrap();
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0], vec!["A", "B", "C"]);
    }

    #[test]
    fn test_topo_sort_cycle() {
        // A→B→A
        let manifests = vec![pkg("A", &["B"]), pkg("B", &["A"])];
        let result = build_publish_order(&manifests);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("A"), "error should mention A: {err}");
        assert!(err.contains("B"), "error should mention B: {err}");
    }

    #[test]
    fn test_topo_sort_mixed_workspace_external() {
        // A depends on B (workspace) and reqwest (external). Only B in graph.
        let manifests = vec![pkg("A", &["B", "reqwest"]), pkg("B", &[])];
        let tiers = build_publish_order(&manifests).unwrap();
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0], vec!["B"]);
        assert_eq!(tiers[1], vec!["A"]);
    }
}

#[cfg(test)]
mod cabal_tests {
    use super::*;

    #[test]
    fn parse_cabal_name_and_version() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("synapse-cc.cabal");
        std::fs::write(&p, "cabal-version:      3.0\nname:               synapse-cc\nversion:            0.3.4\n").unwrap();
        let m = parse_cabal(&p).unwrap();
        assert_eq!(m.kind, "cabal");
        assert_eq!(m.name, "synapse-cc");
        assert_eq!(m.version, "0.3.4");
        assert!(m.deps.is_empty());
    }

    #[test]
    fn parse_cabal_missing_version_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.cabal");
        std::fs::write(&p, "name: x\n").unwrap();
        assert!(parse_cabal(&p).is_err());
    }

    #[test]
    fn detect_prefers_cabal_over_stray_package_json() {
        // HF-PUBLISH-SAFETY: a Haskell repo with an auxiliary
        // package.json must resolve as the cabal package, mirroring
        // registry::detect_build_system — otherwise the publish
        // tiering operates under the wrong name and silently drops
        // the package (observed on synapse-cc / @plexus/client).
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("thing.cabal"),
            "name: thing\nversion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            "{\"name\":\"@scope/other\",\"version\":\"0.0.1\"}",
        )
        .unwrap();
        let m = detect_and_parse(tmp.path()).unwrap();
        assert_eq!(m.kind, "cabal");
        assert_eq!(m.name, "thing");
    }

    #[test]
    fn detect_still_prefers_cargo_over_cabal() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"r\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("x.cabal"), "name: h\nversion: 1.0.0\n").unwrap();
        let m = detect_and_parse(tmp.path()).unwrap();
        assert_eq!(m.kind, "cargo");
    }
}
