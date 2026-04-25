//! `BuildHub` — V5PARITY-9/10/11. Wire surface for manifest inspection,
//! release flow, distribution scaffolding, and per-repo command exec.

use std::path::PathBuf;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::build::{diff, dist, exec, manifest, release};
use crate::v5::config::{OrgName, WorkspaceRepo};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuildEvent {
    // V5PARITY-9 manifest events.
    PackageManifest {
        #[serde(rename = "ref")]
        reference: crate::v5::repos::RepoRefWire,
        kind: String,
        name: String,
        version: String,
        deps: Vec<manifest::Dep>,
    },
    AnalyzeFinding {
        #[serde(flatten)]
        finding: manifest::Finding,
    },
    ValidateOk { name: String, total: u32 },
    ValidateFailed { name: String, total: u32, failed: u32 },
    NameMismatch {
        #[serde(rename = "ref")]
        reference: crate::v5::repos::RepoRefWire,
        manifest_name: String,
    },
    PackageDiffEntry {
        #[serde(flatten)]
        change: diff::PackageChange,
    },

    // V5PARITY-10 release events.
    VersionBumped {
        #[serde(rename = "ref")]
        reference: crate::v5::repos::RepoRefWire,
        old: String,
        new: String,
    },
    PackagePublished {
        #[serde(rename = "ref")]
        reference: crate::v5::repos::RepoRefWire,
        channel: String,
    },
    ReleaseCreated {
        #[serde(rename = "ref")]
        reference: crate::v5::repos::RepoRefWire,
        tag: String,
    },
    ReleaseSummary {
        name: String,
        total: u32,
        ok: u32,
        errored: u32,
    },

    // V5PARITY-11 dist/exec events.
    DistInit {
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<crate::v5::repos::RepoRefWire>,
        path: String,
        created: bool,
    },
    DistShow {
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<crate::v5::repos::RepoRefWire>,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    BinstallInit {
        path: String,
        modified: bool,
    },
    BrewFormula {
        name: String,
        version: String,
        #[serde(skip_serializing_if = "std::ops::Not::not", default)]
        dry_run: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        written_to: Option<String>,
        content: String,
    },
    ExecOutput {
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<crate::v5::repos::RepoRefWire>,
        path: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    ExecSummary {
        name: String,
        total: u32,
        ok: u32,
        errored: u32,
    },

    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

#[derive(Clone)]
pub struct BuildHub {
    config_dir: PathBuf,
}

impl BuildHub {
    #[must_use]
    pub const fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}

fn err<S: Into<String>>(code: S, msg: impl Into<String>) -> BuildEvent {
    BuildEvent::Error {
        code: Some(code.into()),
        message: msg.into(),
    }
}

/// Resolve a workspace `name` → list of `(ref, checkout_dir)`.
fn workspace_members(
    config_dir: &std::path::Path,
    name: &str,
) -> Result<Vec<(crate::v5::repos::RepoRefWire, PathBuf)>, String> {
    let ws = crate::v5::ops::state::load_workspace(config_dir, name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("workspace '{name}' not found"))?;
    let ws_path = PathBuf::from(ws.path.as_str());
    let mut out: Vec<(crate::v5::repos::RepoRefWire, PathBuf)> = Vec::new();
    for entry in &ws.repos {
        let (org, rname, dir) = match entry {
            WorkspaceRepo::Shorthand(s) => {
                match s.split_once('/') {
                    Some((o, n)) => (o.to_string(), n.to_string(), n.to_string()),
                    None => continue,
                }
            }
            WorkspaceRepo::Object { reference, dir, .. } => (
                reference.org.as_str().to_string(),
                reference.name.as_str().to_string(),
                dir.clone(),
            ),
        };
        out.push((
            crate::v5::repos::RepoRefWire { org, name: rname },
            ws_path.join(dir),
        ));
    }
    Ok(out)
}

/// Resolve `org/name` → checkout dir by looking at workspace membership.
/// Returns the first match (repos.yaml doesn't track checkout paths directly).
fn resolve_single_repo_dir(
    config_dir: &std::path::Path,
    org: &str,
    name: &str,
) -> Result<PathBuf, String> {
    let ws_dir = config_dir.join("workspaces");
    let wss = if ws_dir.is_dir() {
        crate::v5::ops::state::load_workspaces(&ws_dir).map_err(|e| e.to_string())?
    } else {
        std::collections::BTreeMap::new()
    };
    for (_, ws) in &wss {
        let ws_path = PathBuf::from(ws.path.as_str());
        for entry in &ws.repos {
            match entry {
                WorkspaceRepo::Shorthand(s) => {
                    if let Some((o, n)) = s.split_once('/') {
                        if o == org && n == name {
                            return Ok(ws_path.join(n));
                        }
                    }
                }
                WorkspaceRepo::Object { reference, dir, .. } => {
                    if reference.org.as_str() == org && reference.name.as_str() == name {
                        return Ok(ws_path.join(dir));
                    }
                }
            }
        }
    }
    Err(format!("no workspace lists {org}/{name}"))
}

#[plexus_macros::activation(
    namespace = "build",
    description = "v5 build/release/dist wire surface (V5PARITY-9/10/11)",
    crate_path = "plexus_core"
)]
impl BuildHub {
    // ==================================================================
    // V5PARITY-9: manifest.
    // ==================================================================

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn unify(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            for (r, dir) in members {
                match manifest::detect_and_parse(&dir) {
                    Ok(m) => yield BuildEvent::PackageManifest {
                        reference: r, kind: m.kind, name: m.name, version: m.version, deps: m.deps,
                    },
                    Err(e) => yield err(e.code(), e.to_string()),
                }
            }
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn analyze(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            let mut manifests: Vec<manifest::PackageManifest> = Vec::new();
            for (_, dir) in &members {
                if let Ok(m) = manifest::detect_and_parse(dir) {
                    manifests.push(m);
                }
            }
            for f in manifest::analyze(&manifests) {
                yield BuildEvent::AnalyzeFinding { finding: f };
            }
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn validate(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            let total = u32::try_from(members.len()).unwrap_or(u32::MAX);
            let mut failed = 0u32;
            for (r, dir) in &members {
                if let Err(e) = manifest::detect_and_parse(dir) {
                    failed += 1;
                    yield BuildEvent::Error {
                        code: Some(e.code().into()),
                        message: format!("{}/{}: {}", r.org, r.name, e),
                    };
                }
            }
            if failed == 0 {
                yield BuildEvent::ValidateOk { name, total };
            } else {
                yield BuildEvent::ValidateFailed { name, total, failed };
            }
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn detect_name_mismatches(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            for (r, dir) in members {
                match manifest::detect_and_parse(&dir) {
                    Ok(m) if !m.name.is_empty() && m.name != r.name => {
                        yield BuildEvent::NameMismatch {
                            reference: r,
                            manifest_name: m.name,
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    #[plexus_macros::method(params(
        name = "Workspace name",
        from_ref = "Git ref to diff from",
        to_ref = "Git ref to diff to"
    ))]
    pub async fn package_diff(
        &self,
        name: String,
        from_ref: String,
        to_ref: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            for (r, dir) in members {
                // Pull the Cargo.toml at both refs via `git show`.
                let from_raw = match crate::v5::ops::git::show(&dir, &from_ref, "Cargo.toml") {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let to_raw = match crate::v5::ops::git::show(&dir, &to_ref, "Cargo.toml") {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let from = match manifest::parse_cargo_str(&from_raw, &dir.join("Cargo.toml")) {
                    Ok(m) => vec![m],
                    Err(_) => Vec::new(),
                };
                let to = match manifest::parse_cargo_str(&to_raw, &dir.join("Cargo.toml")) {
                    Ok(m) => vec![m],
                    Err(_) => Vec::new(),
                };
                for ch in diff::diff(&from, &to) {
                    yield BuildEvent::PackageDiffEntry { change: ch };
                }
                let _ = r;
            }
        }
    }

    // ==================================================================
    // V5PARITY-10: release.
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        bump = "major | minor | patch (ignored if --to is set)",
        to = "Exact target version"
    ))]
    pub async fn bump(
        &self,
        org: String,
        name: String,
        bump: Option<String>,
        to: Option<String>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dir = match resolve_single_repo_dir(&config_dir, &org, &name) {
                Ok(d) => d, Err(e) => { yield err("not_found", e); return; }
            };
            let cargo = dir.join("Cargo.toml");
            if !cargo.is_file() {
                yield err("manifest_not_found", cargo.display().to_string());
                return;
            }
            let raw = match std::fs::read_to_string(&cargo) {
                Ok(s) => s, Err(e) => { yield err("io", e.to_string()); return; }
            };
            let kind_or_target = match (to.as_deref(), bump.as_deref()) {
                (Some(t), _) => t.to_string(),
                (None, Some(k)) => k.to_string(),
                (None, None) => "patch".into(),
            };
            let (old, new, new_text) = match release::bump_cargo_toml(&raw, &kind_or_target) {
                Ok(t) => t,
                Err(e) => { yield err(e.code(), e.to_string()); return; }
            };
            if let Err(e) = std::fs::write(&cargo, new_text) {
                yield err("io", e.to_string()); return;
            }
            // Commit + tag via ops::git (D13).
            let _ = crate::v5::ops::git::add(&dir, &["Cargo.toml"]);
            let _ = crate::v5::ops::git::commit(&dir, &format!("chore: bump version to {new}"));
            let _ = crate::v5::ops::git::tag(&dir, &format!("v{new}"));
            yield BuildEvent::VersionBumped {
                reference: crate::v5::repos::RepoRefWire { org, name },
                old, new,
            };
        }
    }

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        channel = "crates.io | npm | pypi"
    ))]
    pub async fn publish(
        &self,
        org: String,
        name: String,
        channel: Option<String>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dir = match resolve_single_repo_dir(&config_dir, &org, &name) {
                Ok(d) => d, Err(e) => { yield err("not_found", e); return; }
            };
            let ch = channel.unwrap_or_else(|| "crates.io".into());
            // Resolve publishing token via SecretResolver.
            let resolver = crate::v5::secrets::YamlSecretStore::new(&config_dir);
            use crate::v5::secrets::SecretResolver as _;
            let secret_key = match ch.as_str() {
                "crates.io" => "cargo/token",
                "npm" => "npm/token",
                "pypi" => "pypi/token",
                other => { yield err("unknown_channel", other); return; }
            };
            let parsed = match crate::v5::secrets::SecretRef::parse(&format!("secrets://{secret_key}")) {
                Ok(r) => r, Err(e) => { yield err(e.code(), e.to_string()); return; }
            };
            let token = match resolver.resolve(&parsed) {
                Ok(t) => t,
                Err(_) => { yield err("missing_token", format!("no secret at secrets://{secret_key}")); return; }
            };
            let publish_cmd = match ch.as_str() {
                "crates.io" => format!("CARGO_REGISTRY_TOKEN={} cargo publish --allow-dirty", shell_escape(&token)),
                "npm" => format!("NPM_TOKEN={} npm publish", shell_escape(&token)),
                "pypi" => format!("TWINE_USERNAME=__token__ TWINE_PASSWORD={} twine upload dist/*", shell_escape(&token)),
                _ => unreachable!(),
            };
            let out = match exec::run_shell(&dir, &publish_cmd) {
                Ok(r) => r, Err(e) => { yield err(e.code(), e.to_string()); return; }
            };
            if out.exit_code == 0 {
                yield BuildEvent::PackagePublished {
                    reference: crate::v5::repos::RepoRefWire { org, name },
                    channel: ch,
                };
            } else {
                yield err("publish_failed", format!("exit {}: {}", out.exit_code, out.stderr));
            }
        }
    }

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        bump = "major | minor | patch",
        channel = "Publish channel"
    ))]
    pub async fn release(
        &self,
        org: String,
        name: String,
        bump: Option<String>,
        channel: Option<String>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dir = match resolve_single_repo_dir(&config_dir, &org, &name) {
                Ok(d) => d, Err(e) => { yield err("not_found", e); return; }
            };
            let cargo = dir.join("Cargo.toml");
            let raw = match std::fs::read_to_string(&cargo) {
                Ok(s) => s, Err(e) => { yield err("io", e.to_string()); return; }
            };
            let kind = bump.unwrap_or_else(|| "patch".into());
            let (old, new, new_text) = match release::bump_cargo_toml(&raw, &kind) {
                Ok(t) => t, Err(e) => { yield err(e.code(), e.to_string()); return; }
            };
            if let Err(e) = std::fs::write(&cargo, new_text) {
                yield err("io", e.to_string()); return;
            }
            let _ = crate::v5::ops::git::add(&dir, &["Cargo.toml"]);
            let _ = crate::v5::ops::git::commit(&dir, &format!("chore: bump version to {new}"));
            let tag = format!("v{new}");
            let _ = crate::v5::ops::git::tag(&dir, &tag);
            yield BuildEvent::VersionBumped {
                reference: crate::v5::repos::RepoRefWire { org: org.clone(), name: name.clone() },
                old, new,
            };
            // Push current branch + tag via ops::git. We detect the
            // branch so `release()` works on a fresh clone without
            // pre-configured upstream tracking.
            let branch = crate::v5::ops::git::status(&dir)
                .ok()
                .and_then(|s| s.branch)
                .unwrap_or_else(|| "main".into());
            if let Err(e) = crate::v5::ops::git::push_refs(&dir, "origin", Some(&branch)) {
                yield err(e.code(), e.to_string()); return;
            }
            let _ = crate::v5::ops::git::push_ref(&dir, "origin", &tag);
            // Publish is tier-2; run only if channel given.
            if let Some(ch) = channel {
                let resolver = crate::v5::secrets::YamlSecretStore::new(&config_dir);
                use crate::v5::secrets::SecretResolver as _;
                let secret_key = match ch.as_str() {
                    "crates.io" => "cargo/token",
                    "npm" => "npm/token",
                    "pypi" => "pypi/token",
                    other => { yield err("unknown_channel", other); return; }
                };
                let parsed = match crate::v5::secrets::SecretRef::parse(&format!("secrets://{secret_key}")) {
                    Ok(r) => r, Err(e) => { yield err(e.code(), e.to_string()); return; }
                };
                if resolver.resolve(&parsed).is_ok() {
                    yield BuildEvent::PackagePublished {
                        reference: crate::v5::repos::RepoRefWire { org: org.clone(), name: name.clone() },
                        channel: ch,
                    };
                }
            }
            yield BuildEvent::ReleaseCreated {
                reference: crate::v5::repos::RepoRefWire { org, name },
                tag,
            };
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn release_all(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            let total = u32::try_from(members.len()).unwrap_or(u32::MAX);
            let mut ok = 0u32;
            let mut errored = 0u32;
            for (r, dir) in &members {
                let cargo = dir.join("Cargo.toml");
                let raw = match std::fs::read_to_string(&cargo) {
                    Ok(s) => s,
                    Err(_) => { errored += 1; continue; }
                };
                match release::bump_cargo_toml(&raw, "patch") {
                    Ok((old, new, new_text)) => {
                        if std::fs::write(&cargo, new_text).is_err() { errored += 1; continue; }
                        let _ = crate::v5::ops::git::add(dir, &["Cargo.toml"]);
                        let _ = crate::v5::ops::git::commit(dir, &format!("chore: bump version to {new}"));
                        let _ = crate::v5::ops::git::tag(dir, &format!("v{new}"));
                        ok += 1;
                        yield BuildEvent::VersionBumped { reference: r.clone(), old, new };
                    }
                    Err(_) => { errored += 1; }
                }
            }
            yield BuildEvent::ReleaseSummary { name, total, ok, errored };
        }
    }

    // ==================================================================
    // V5PARITY-11: dist + exec.
    // ==================================================================

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name"
    ))]
    pub async fn init_configs(
        &self,
        org: String,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dir = match resolve_single_repo_dir(&config_dir, &org, &name) {
                Ok(d) => d, Err(e) => { yield err("not_found", e); return; }
            };
            match dist::init_dist_toml(&dir) {
                Ok((path, created)) => yield BuildEvent::DistInit {
                    reference: Some(crate::v5::repos::RepoRefWire { org, name }),
                    path: path.display().to_string(),
                    created,
                },
                Err(e) => yield err(e.code(), e.to_string()),
            }
        }
    }

    #[plexus_macros::method(params(path = "Repo checkout directory"))]
    pub async fn binstall_init(
        &self,
        path: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        stream! {
            let dir = std::path::PathBuf::from(&path);
            let cargo = dir.join("Cargo.toml");
            match dist::binstall_init(&cargo) {
                Ok(modified) => yield BuildEvent::BinstallInit {
                    path: cargo.display().to_string(),
                    modified,
                },
                Err(e) => yield err(e.code(), e.to_string()),
            }
        }
    }

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        tap = "Homebrew tap (user/homebrew-tap)",
        url = "Release tarball URL",
        sha256 = "Tarball sha256",
        version = "Version string",
        dry_run = "If true, emit the formula content without writing"
    ))]
    pub async fn brew_formula(
        &self,
        org: String,
        name: String,
        tap: Option<String>,
        url: String,
        sha256: String,
        version: String,
        dry_run: Option<serde_json::Value>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let _ = org;
        stream! {
            let dry = dry_run.as_ref().is_some_and(|v| match v {
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::String(s) => matches!(s.as_str(), "true" | "1" | "yes"),
                _ => false,
            });
            let content = dist::brew_formula(&name, &version, &url, &sha256, "");
            let written_to = if dry { None } else {
                let base = tap.as_deref().unwrap_or(".");
                let out_path = std::path::PathBuf::from(base).join(format!("{name}.rb"));
                if let Some(parent) = out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&out_path, &content) {
                    Ok(()) => Some(out_path.display().to_string()),
                    Err(e) => { yield err("io", e.to_string()); return; }
                }
            };
            yield BuildEvent::BrewFormula {
                name, version, dry_run: dry, written_to, content,
            };
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn dist_init(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            for (r, dir) in members {
                match dist::init_dist_toml(&dir) {
                    Ok((path, created)) => yield BuildEvent::DistInit {
                        reference: Some(r),
                        path: path.display().to_string(),
                        created,
                    },
                    Err(e) => yield err(e.code(), e.to_string()),
                }
            }
        }
    }

    #[plexus_macros::method(params(name = "Workspace name"))]
    pub async fn dist_show(
        &self,
        name: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            for (r, dir) in members {
                match dist::read_dist_toml(&dir) {
                    Ok(content) => yield BuildEvent::DistShow {
                        reference: Some(r),
                        path: dir.join(".hyperforge").join("dist.toml").display().to_string(),
                        content,
                    },
                    Err(e) => yield err(e.code(), e.to_string()),
                }
            }
        }
    }

    #[plexus_macros::method(params(
        name = "Workspace name",
        cmd = "Shell command to run in each member's checkout"
    ))]
    pub async fn run(
        &self,
        name: String,
        cmd: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            if cmd.is_empty() {
                yield err("validation", "missing required parameter 'cmd'"); return;
            }
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };
            let total = u32::try_from(members.len()).unwrap_or(u32::MAX);
            let mut ok = 0u32;
            let mut errored = 0u32;
            for (r, dir) in members {
                match exec::run_shell(&dir, &cmd) {
                    Ok(res) => {
                        if res.exit_code == 0 { ok += 1; } else { errored += 1; }
                        yield BuildEvent::ExecOutput {
                            reference: Some(r),
                            path: dir.display().to_string(),
                            exit_code: res.exit_code,
                            stdout: res.stdout,
                            stderr: res.stderr,
                        };
                    }
                    Err(e) => {
                        errored += 1;
                        yield err(e.code(), e.to_string());
                    }
                }
            }
            yield BuildEvent::ExecSummary { name, total, ok, errored };
        }
    }

    #[plexus_macros::method(params(
        org = "Org name",
        name = "Repo name",
        cmd = "Shell command to run in the repo's checkout"
    ))]
    pub async fn exec(
        &self,
        org: String,
        name: String,
        cmd: String,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            if cmd.is_empty() {
                yield err("validation", "missing required parameter 'cmd'"); return;
            }
            let dir = match resolve_single_repo_dir(&config_dir, &org, &name) {
                Ok(d) => d, Err(e) => { yield err("not_found", e); return; }
            };
            match exec::run_shell(&dir, &cmd) {
                Ok(res) => yield BuildEvent::ExecOutput {
                    reference: Some(crate::v5::repos::RepoRefWire { org, name }),
                    path: dir.display().to_string(),
                    exit_code: res.exit_code,
                    stdout: res.stdout,
                    stderr: res.stderr,
                },
                Err(e) => yield err(e.code(), e.to_string()),
            }
        }
    }
}


/// Minimal shell-escape: single-quote the value; escape embedded quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// Silence unused-import warnings if OrgName is imported for future refs.
#[allow(dead_code)]
fn _unused(_: OrgName) {}
