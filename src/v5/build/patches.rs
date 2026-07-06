//! Language-agnostic local-patch detection.
//!
//! Two categories of "local patching" are surfaced per repo:
//!
//! * `declared_override` — a manifest deliberately redirects a
//!   dependency at a local path / fork / pin: Rust `[patch]` /
//!   `[replace]` / `path =`, Go `replace => ./local`, npm
//!   `file:`/`link:`/`portal:` + `resolutions`/`overrides` +
//!   patch-package, Python editable installs / poetry `path`,
//!   Composer `path` repositories, Ruby `path:`/`git:` gems, Gradle
//!   `includeBuild`/`project(...)`, Maven system-scoped paths.
//!
//! * `vendored_edit` — a checked-in vendored dependency tree
//!   (`vendor/`, `third_party/`, …) has uncommitted local edits.
//!
//! Every parser is best-effort and defensive: an unreadable or
//! malformed manifest yields no findings (manifest validity is
//! `build.validate`'s job, not this method's).

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One local-patch signal found in a repo. Flattened into
/// `BuildEvent::PatchFinding` on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Finding {
    /// `cargo` | `go` | `npm` | `python` | `composer` | `ruby` |
    /// `gradle` | `maven` | `vendor`.
    pub ecosystem: String,
    /// `declared_override` | `vendored_edit`.
    pub category: String,
    /// Specific signal, e.g. `patch_table`, `path_dep`,
    /// `replace_directive`, `local_specifier`, `resolutions`,
    /// `patch_package`, `editable_install`, `path_repository`,
    /// `path_gem`, `included_build`, `dirty_vendor`.
    pub kind: String,
    /// Human-readable specifics (dependency name, target path, …).
    pub detail: String,
    /// Manifest / file the signal came from, relative to the repo root.
    pub file: String,
}

fn f(eco: &str, cat: &str, kind: &str, detail: impl Into<String>, file: &str) -> Finding {
    Finding {
        ecosystem: eco.to_string(),
        category: cat.to_string(),
        kind: kind.to_string(),
        detail: detail.into(),
        file: file.to_string(),
    }
}

/// Scan a single repo checkout for every local-patch signal.
#[must_use]
pub fn scan_repo(dir: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    scan_cargo(dir, &mut out);
    scan_go(dir, &mut out);
    scan_npm(dir, &mut out);
    scan_python(dir, &mut out);
    scan_composer(dir, &mut out);
    scan_ruby(dir, &mut out);
    scan_gradle(dir, &mut out);
    scan_maven(dir, &mut out);
    scan_vendored_edits(dir, &mut out);
    out
}

fn read(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

// --- Rust / Cargo -----------------------------------------------------

fn scan_cargo(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "Cargo.toml") else { return };
    let Ok(tbl) = text.parse::<toml::Table>() else { return };

    // `[patch.<registry>]` and `[workspace.patch...]` / `[replace]`.
    if let Some(patch) = tbl.get("patch").and_then(toml::Value::as_table) {
        for (registry, crates) in patch {
            if let Some(ct) = crates.as_table() {
                for name in ct.keys() {
                    out.push(f(
                        "cargo",
                        "declared_override",
                        "patch_table",
                        format!("{name} (via [patch.{registry}])"),
                        "Cargo.toml",
                    ));
                }
            }
        }
    }
    if let Some(repl) = tbl.get("replace").and_then(toml::Value::as_table) {
        for spec in repl.keys() {
            out.push(f(
                "cargo",
                "declared_override",
                "replace_table",
                format!("{spec} (via [replace])"),
                "Cargo.toml",
            ));
        }
    }

    // `path = ` dependencies in every dependency table, including
    // `[target.<cfg>.*-dependencies]` and `[workspace.dependencies]`.
    fn scan_dep_tables(node: &toml::Value, out: &mut Vec<Finding>) {
        let Some(t) = node.as_table() else { return };
        for kind in [
            "dependencies",
            "dev-dependencies",
            "build-dependencies",
        ] {
            if let Some(deps) = t.get(kind).and_then(toml::Value::as_table) {
                for (name, spec) in deps {
                    if let Some(p) = spec.as_table().and_then(|s| s.get("path")) {
                        let p = p.as_str().unwrap_or("?");
                        out.push(f(
                            "cargo",
                            "declared_override",
                            "path_dep",
                            format!("{name} -> {p} (in [{kind}])"),
                            "Cargo.toml",
                        ));
                    }
                }
            }
        }
    }
    scan_dep_tables(&toml::Value::Table(tbl.clone()), out);
    if let Some(ws) = tbl.get("workspace") {
        scan_dep_tables(ws, out);
    }
    if let Some(targets) = tbl.get("target").and_then(toml::Value::as_table) {
        for cfg in targets.values() {
            scan_dep_tables(cfg, out);
        }
    }
}

// --- Go ---------------------------------------------------------------

fn scan_go(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "go.mod") else { return };
    let mut in_block = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("replace (") {
            in_block = true;
            continue;
        }
        if in_block && line == ")" {
            in_block = false;
            continue;
        }
        let body = if in_block {
            Some(line)
        } else {
            line.strip_prefix("replace ").map(str::trim)
        };
        let Some(body) = body else { continue };
        let Some((_, rhs)) = body.split_once("=>") else { continue };
        let target = rhs.trim().split_whitespace().next().unwrap_or("");
        // Local replace: filesystem path rather than a versioned module.
        if target.starts_with("./")
            || target.starts_with("../")
            || target.starts_with('/')
            || target.starts_with(".\\")
        {
            out.push(f(
                "go",
                "declared_override",
                "replace_directive",
                format!("{} (replace => {target})", body.split("=>").next().unwrap_or("").trim()),
                "go.mod",
            ));
        }
    }
}

// --- npm / Node -------------------------------------------------------

fn scan_npm(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "package.json") else { return };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return };

    for field in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(obj) = json.get(field).and_then(|v| v.as_object()) {
            for (name, spec) in obj {
                let s = spec.as_str().unwrap_or("");
                if s.starts_with("file:") || s.starts_with("link:") || s.starts_with("portal:") {
                    out.push(f(
                        "npm",
                        "declared_override",
                        "local_specifier",
                        format!("{name} -> {s} (in {field})"),
                        "package.json",
                    ));
                }
            }
        }
    }
    for (field, kind) in [
        ("resolutions", "resolutions"),
        ("overrides", "overrides"),
    ] {
        if let Some(obj) = json.get(field).and_then(|v| v.as_object()) {
            for name in obj.keys() {
                out.push(f(
                    "npm",
                    "declared_override",
                    kind,
                    format!("{name} (in \"{field}\")"),
                    "package.json",
                ));
            }
        }
    }
    if let Some(pnpm) = json.get("pnpm") {
        if let Some(o) = pnpm.get("overrides").and_then(|v| v.as_object()) {
            for name in o.keys() {
                out.push(f("npm", "declared_override", "overrides",
                    format!("{name} (in pnpm.overrides)"), "package.json"));
            }
        }
        if let Some(o) = pnpm.get("patchedDependencies").and_then(|v| v.as_object()) {
            for name in o.keys() {
                out.push(f("npm", "declared_override", "patch_package",
                    format!("{name} (pnpm.patchedDependencies)"), "package.json"));
            }
        }
    }
    // patch-package: a patches/ dir of *.patch files.
    if let Ok(rd) = std::fs::read_dir(dir.join("patches")) {
        for e in rd.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            if n.ends_with(".patch") {
                out.push(f("npm", "declared_override", "patch_package",
                    format!("patches/{n}"), "patches/"));
            }
        }
    }
}

// --- Python -----------------------------------------------------------

fn scan_python(dir: &Path, out: &mut Vec<Finding>) {
    if let Some(text) = read(dir, "pyproject.toml") {
        if let Ok(tbl) = text.parse::<toml::Table>() {
            // Poetry: path = / develop = true dependency tables.
            let poetry = tbl
                .get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(toml::Value::as_table);
            if let Some(p) = poetry {
                for key in ["dependencies", "dev-dependencies"] {
                    if let Some(deps) = p.get(key).and_then(toml::Value::as_table) {
                        for (name, spec) in deps {
                            if let Some(st) = spec.as_table() {
                                if st.contains_key("path") || st.get("develop")
                                    .and_then(toml::Value::as_bool) == Some(true)
                                {
                                    out.push(f("python", "declared_override", "path_dep",
                                        format!("{name} (poetry {key})"), "pyproject.toml"));
                                }
                            }
                        }
                    }
                }
            }
            // uv workspace/path sources.
            if let Some(src) = tbl
                .get("tool")
                .and_then(|t| t.get("uv"))
                .and_then(|u| u.get("sources"))
                .and_then(toml::Value::as_table)
            {
                for (name, spec) in src {
                    if let Some(s) = spec.as_table() {
                        if s.contains_key("path") || s.contains_key("workspace") {
                            out.push(f("python", "declared_override", "path_dep",
                                format!("{name} (tool.uv.sources)"), "pyproject.toml"));
                        }
                    }
                }
            }
        }
    }
    for req in ["requirements.txt", "requirements-dev.txt", "dev-requirements.txt"] {
        let Some(text) = read(dir, req) else { continue };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("-e ")
                || line.starts_with("--editable")
                || line.contains("file://")
                || line.starts_with("./")
                || line.starts_with("../")
                || line.contains(" @ ./")
                || line.contains(" @ ../")
            {
                out.push(f("python", "declared_override", "editable_install",
                    line.chars().take(120).collect::<String>(), req));
            }
        }
    }
}

// --- Composer / PHP ---------------------------------------------------

fn scan_composer(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "composer.json") else { return };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let repos = json.get("repositories");
    let iter: Vec<&serde_json::Value> = match repos {
        Some(serde_json::Value::Array(a)) => a.iter().collect(),
        Some(serde_json::Value::Object(o)) => o.values().collect(),
        _ => return,
    };
    for r in iter {
        if r.get("type").and_then(|v| v.as_str()) == Some("path") {
            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            out.push(f("composer", "declared_override", "path_repository",
                format!("path repository -> {url}"), "composer.json"));
        }
    }
}

// --- Ruby / Bundler ---------------------------------------------------

fn scan_ruby(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "Gemfile") else { return };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || !line.starts_with("gem ") {
            if line.starts_with("gemspec") {
                out.push(f("ruby", "declared_override", "local_gemspec",
                    "gemspec".to_string(), "Gemfile"));
            }
            continue;
        }
        if line.contains("path:") || line.contains("path =>") || line.contains(":path") {
            out.push(f("ruby", "declared_override", "path_gem",
                line.chars().take(120).collect::<String>(), "Gemfile"));
        } else if line.contains("git:")
            || line.contains("github:")
            || line.contains("git =>")
        {
            out.push(f("ruby", "declared_override", "git_gem",
                line.chars().take(120).collect::<String>(), "Gemfile"));
        }
    }
}

// --- Gradle -----------------------------------------------------------

fn scan_gradle(dir: &Path, out: &mut Vec<Finding>) {
    for settings in ["settings.gradle", "settings.gradle.kts"] {
        if let Some(text) = read(dir, settings) {
            for raw in text.lines() {
                if raw.trim_start().starts_with("includeBuild") {
                    out.push(f("gradle", "declared_override", "included_build",
                        raw.trim().chars().take(120).collect::<String>(), settings));
                }
            }
        }
    }
    for build in ["build.gradle", "build.gradle.kts"] {
        if let Some(text) = read(dir, build) {
            for raw in text.lines() {
                let l = raw.trim();
                if l.contains("project(") && (l.contains("implementation") || l.contains("api") || l.contains("compile")) {
                    out.push(f("gradle", "declared_override", "project_dependency",
                        l.chars().take(120).collect::<String>(), build));
                } else if l.contains("mavenLocal()") {
                    out.push(f("gradle", "declared_override", "maven_local",
                        "mavenLocal()".to_string(), build));
                }
            }
        }
    }
}

// --- Maven ------------------------------------------------------------

fn scan_maven(dir: &Path, out: &mut Vec<Finding>) {
    let Some(text) = read(dir, "pom.xml") else { return };
    if text.contains("<systemPath>") {
        out.push(f("maven", "declared_override", "system_path",
            "<systemPath> dependency".to_string(), "pom.xml"));
    }
    if text.contains("<scope>system</scope>") {
        out.push(f("maven", "declared_override", "system_scope",
            "<scope>system</scope> dependency".to_string(), "pom.xml"));
    }
}

// --- Hand-edited vendored trees --------------------------------------

const VENDOR_PREFIXES: &[&str] = &[
    "vendor/",
    "third_party/",
    "third-party/",
    "Godeps/",
    "vendored/",
    ".yarn/patches/",
];

fn scan_vendored_edits(dir: &Path, out: &mut Vec<Finding>) {
    let Ok(changed) = crate::v5::ops::git::changed_paths(dir) else { return };
    for prefix in VENDOR_PREFIXES {
        let hits: Vec<&String> = changed
            .iter()
            .filter(|p| p.starts_with(prefix) || p.starts_with(&format!("\"{prefix}")))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let sample: Vec<String> = hits.iter().take(5).map(|s| (*s).clone()).collect();
        let more = hits.len().saturating_sub(sample.len());
        let detail = if more > 0 {
            format!("{} edited under {prefix}: {} (+{more} more)", hits.len(), sample.join(", "))
        } else {
            format!("{} edited under {prefix}: {}", hits.len(), sample.join(", "))
        };
        out.push(f("vendor", "vendored_edit", "dirty_vendor", detail, prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_patch_and_path_dep() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "x"
version = "0.1.0"

[dependencies]
plexus-core = { path = "../plexus-core" }
serde = "1"

[patch.crates-io]
serde = { git = "https://example/serde" }
"#,
        )
        .unwrap();
        let fs = scan_repo(tmp.path());
        assert!(fs.iter().any(|x| x.kind == "path_dep" && x.detail.contains("plexus-core")));
        assert!(fs.iter().any(|x| x.kind == "patch_table" && x.detail.contains("serde")));
    }

    #[test]
    fn npm_file_specifier_and_resolutions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{ "dependencies": { "a": "file:../a" },
                 "resolutions": { "left-pad": "1.3.0" } }"#,
        )
        .unwrap();
        let fs = scan_repo(tmp.path());
        assert!(fs.iter().any(|x| x.kind == "local_specifier" && x.detail.contains("file:../a")));
        assert!(fs.iter().any(|x| x.kind == "resolutions" && x.detail.contains("left-pad")));
    }

    #[test]
    fn go_local_replace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("go.mod"),
            "module m\n\nrequire x v1.0.0\nreplace x => ./local/x\n",
        )
        .unwrap();
        let fs = scan_repo(tmp.path());
        assert!(fs.iter().any(|x| x.ecosystem == "go" && x.kind == "replace_directive"));
    }

    #[test]
    fn clean_repo_has_no_findings() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n[dependencies]\nserde=\"1\"\n",
        )
        .unwrap();
        assert!(scan_repo(tmp.path()).is_empty());
    }
}
