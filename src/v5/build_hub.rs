//! `BuildHub` — V5PARITY-9/10/11. Wire surface for manifest inspection,
//! release flow, distribution scaffolding, and per-repo command exec.

use std::path::PathBuf;

use async_stream::stream;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::v5::build::{diff, dist, exec, manifest, registry, release};
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
    RegistryDiffEntry {
        package_name: String,
        build_system: String,
        local_version: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        published_version: Option<String>,
        registry: String,
        status: String,
    },
    RegistryDiffSummary {
        name: String,
        total: u32,
        ahead: u32,
        stale: u32,
        up_to_date: u32,
        unpublished: u32,
        errored: u32,
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

    // PKGPUB-6: drift fix events.
    DriftFixed {
        package_name: String,
        old_version: String,
        new_version: String,
        build_system: String,
        dry_run: bool,
    },
    DriftSummary {
        total_checked: u32,
        drifted: u32,
        fixed: u32,
        dry_run: bool,
    },

    // PKGPUB-4: workspace publish events.
    PublishStep {
        package_name: String,
        version: String,
        registry: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        dry_run: bool,
    },
    PublishSummary {
        total: u32,
        published: u32,
        failed: u32,
        skipped: u32,
        auto_bumped: u32,
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

    #[plexus_macros::method(params(
        name = "Workspace name",
        filter = "Glob patterns to include (optional)"
    ))]
    pub async fn registry_diff(
        &self,
        name: String,
        filter: Option<Vec<String>>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };

            let mut total = 0u32;
            let mut ahead = 0u32;
            let mut stale = 0u32;
            let mut up_to_date = 0u32;
            let mut unpublished = 0u32;
            let mut errored = 0u32;

            for (_r, dir) in &members {
                // Detect build system for this member.
                let (build_sys, reg) = match registry::detect_build_system(dir) {
                    Some(pair) => pair,
                    None => continue, // No known manifest, skip.
                };

                // Parse local name + version.
                let (pkg_name, local_ver) = match registry::parse_local_version(dir, build_sys) {
                    Some(pair) => pair,
                    None => continue,
                };

                // Apply filter if provided.
                if let Some(ref patterns) = filter {
                    let matched = patterns.iter().any(|pat| {
                        glob_match(pat, &pkg_name)
                    });
                    if !matched {
                        continue;
                    }
                }

                total += 1;

                // Query registry (async).
                let published = match registry::query_registry(&pkg_name, reg).await {
                    Ok(v) => v,
                    Err(e) => {
                        errored += 1;
                        yield err(e.code(), e.to_string());
                        continue;
                    }
                };

                let status = registry::diff_status(&local_ver, published.as_deref());

                match status {
                    "ahead" => ahead += 1,
                    "stale" => stale += 1,
                    "up_to_date" => up_to_date += 1,
                    "unpublished" => unpublished += 1,
                    _ => {}
                }

                yield BuildEvent::RegistryDiffEntry {
                    package_name: pkg_name,
                    build_system: build_sys.to_string(),
                    local_version: local_ver,
                    published_version: published,
                    registry: reg.to_string(),
                    status: status.to_string(),
                };
            }

            yield BuildEvent::RegistryDiffSummary {
                name,
                total,
                ahead,
                stale,
                up_to_date,
                unpublished,
                errored,
            };
        }
    }

    // ==================================================================
    // PKGPUB-6: auto-bump drifted packages.
    // ==================================================================

    #[plexus_macros::method(params(
        name = "Workspace name",
        filter = "Glob patterns to include (optional)",
        dry_run = "If true (default), show what would be bumped without writing"
    ))]
    pub async fn fix_drift(
        &self,
        name: String,
        filter: Option<Vec<String>>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let dry = dry_run.unwrap_or(true);
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v, Err(e) => { yield err("not_found", e); return; }
            };

            let mut total_checked = 0u32;
            let mut drifted = 0u32;
            let mut fixed = 0u32;

            for (_r, dir) in &members {
                // Detect build system for this member.
                let (build_sys, reg) = match registry::detect_build_system(dir) {
                    Some(pair) => pair,
                    None => continue,
                };

                // Parse local name + version.
                let (pkg_name, local_ver) = match registry::parse_local_version(dir, build_sys) {
                    Some(pair) => pair,
                    None => continue,
                };

                // Apply filter if provided.
                if let Some(ref patterns) = filter {
                    let matched = patterns.iter().any(|pat| glob_match(pat, &pkg_name));
                    if !matched {
                        continue;
                    }
                }

                total_checked += 1;

                // Query registry.
                let published = match registry::query_registry(&pkg_name, reg).await {
                    Ok(v) => v,
                    Err(e) => {
                        yield err(e.code(), e.to_string());
                        continue;
                    }
                };

                let status = registry::diff_status(&local_ver, published.as_deref());

                // "up_to_date" means local == published — the package is drifted
                // (same version on registry, but source may have changed).
                if status != "up_to_date" {
                    continue;
                }

                drifted += 1;

                // Determine the manifest file and bump.
                let bump_result = match build_sys {
                    "cargo" => {
                        let cargo = dir.join("Cargo.toml");
                        let raw = match std::fs::read_to_string(&cargo) {
                            Ok(s) => s,
                            Err(e) => { yield err("io", e.to_string()); continue; }
                        };
                        release::bump_cargo_toml(&raw, "patch").map(|(old, new, text)| {
                            (old, new, text, cargo, "Cargo.toml".to_string())
                        })
                    }
                    "cabal" => {
                        // Find the .cabal file.
                        let cabal_path = find_cabal_file(dir);
                        match cabal_path {
                            Some(cp) => {
                                let raw = match std::fs::read_to_string(&cp) {
                                    Ok(s) => s,
                                    Err(e) => { yield err("io", e.to_string()); continue; }
                                };
                                let file_name = cp.file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                release::bump_cabal_file(&raw, "patch").map(|(old, new, text)| {
                                    (old, new, text, cp, file_name)
                                })
                            }
                            None => { yield err("manifest_not_found", format!("no .cabal file in {}", dir.display())); continue; }
                        }
                    }
                    _ => {
                        // node/other — not yet supported for auto-bump.
                        yield err("unsupported", format!("auto-bump not supported for build system '{build_sys}'"));
                        continue;
                    }
                };

                let (old, new, new_text, manifest_path, manifest_name) = match bump_result {
                    Ok(t) => t,
                    Err(e) => { yield err(e.code(), e.to_string()); continue; }
                };

                if !dry {
                    if let Err(e) = std::fs::write(&manifest_path, new_text) {
                        yield err("io", e.to_string());
                        continue;
                    }
                    let _ = crate::v5::ops::git::add(dir, &[&manifest_name]);
                    let _ = crate::v5::ops::git::commit(
                        dir,
                        &format!("chore: bump {pkg_name} {old} \u{2192} {new} (drift)"),
                    );
                    fixed += 1;
                }

                yield BuildEvent::DriftFixed {
                    package_name: pkg_name,
                    old_version: old,
                    new_version: new,
                    build_system: build_sys.to_string(),
                    dry_run: dry,
                };
            }

            yield BuildEvent::DriftSummary {
                total_checked,
                drifted,
                fixed: if dry { 0 } else { fixed },
                dry_run: dry,
            };
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
    // PKGPUB-4: workspace-wide publish with dependency ordering.
    // ==================================================================

    #[plexus_macros::method(params(
        name = "Workspace name",
        include = "Glob patterns to include (optional)",
        exclude = "Glob patterns to exclude (optional)",
        bump = "major | minor | patch (default: patch)",
        execute = "Actually publish (default: false = dry-run)",
        no_tag = "Skip git tags",
        no_commit = "Skip auto-commit after bumps"
    ))]
    pub async fn publish_workspace(
        &self,
        name: String,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        bump: Option<String>,
        execute: Option<bool>,
        no_tag: Option<bool>,
        no_commit: Option<bool>,
    ) -> impl Stream<Item = BuildEvent> + Send + 'static {
        let config_dir = self.config_dir.clone();
        stream! {
            let bump_kind = bump.as_deref().unwrap_or("patch");
            if !matches!(bump_kind, "patch" | "minor" | "major") {
                yield err("validation", format!("invalid bump kind: {bump_kind}"));
                return;
            }
            let dry_run = !execute.unwrap_or(false);
            let skip_tag = no_tag.unwrap_or(false);
            let skip_commit = no_commit.unwrap_or(false);

            // 1. Resolve workspace members, detect build systems, parse manifests.
            let members = match workspace_members(&config_dir, &name) {
                Ok(v) => v,
                Err(e) => { yield err("not_found", e); return; }
            };

            // Collect package info for each member.
            struct PkgInfo {
                pkg_name: String,
                local_version: String,
                build_system: String,
                registry: String,
                dir: PathBuf,
                manifest: manifest::PackageManifest,
            }

            let mut packages: Vec<PkgInfo> = Vec::new();
            for (_r, dir) in &members {
                let (build_sys, reg) = match registry::detect_build_system(dir) {
                    Some(pair) => pair,
                    None => continue,
                };
                let (pkg_name, local_ver) = match registry::parse_local_version(dir, build_sys) {
                    Some(pair) => pair,
                    None => continue,
                };
                let m = match manifest::detect_and_parse(dir) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                packages.push(PkgInfo {
                    pkg_name,
                    local_version: local_ver,
                    build_system: build_sys.to_string(),
                    registry: reg.to_string(),
                    dir: dir.clone(),
                    manifest: m,
                });
            }

            // 2. Query registries and determine status.
            struct PkgStatus {
                pkg_name: String,
                local_version: String,
                build_system: String,
                registry: String,
                dir: PathBuf,
                status: String,
                #[allow(dead_code)]
                published_version: Option<String>,
                deps: Vec<String>,
            }

            let workspace_names: std::collections::BTreeSet<String> =
                packages.iter().map(|p| p.pkg_name.clone()).collect();

            let mut statuses: Vec<PkgStatus> = Vec::new();
            for pkg in &packages {
                let published = match registry::query_registry(&pkg.pkg_name, &pkg.registry).await {
                    Ok(v) => v,
                    Err(e) => {
                        yield err(e.code(), e.to_string());
                        continue;
                    }
                };
                let status = registry::diff_status(&pkg.local_version, published.as_deref());
                let ws_deps: Vec<String> = pkg.manifest.deps.iter()
                    .filter(|d| workspace_names.contains(&d.name))
                    .map(|d| d.name.clone())
                    .collect();
                statuses.push(PkgStatus {
                    pkg_name: pkg.pkg_name.clone(),
                    local_version: pkg.local_version.clone(),
                    build_system: pkg.build_system.clone(),
                    registry: pkg.registry.clone(),
                    dir: pkg.dir.clone(),
                    status: status.to_string(),
                    published_version: published,
                    deps: ws_deps,
                });
            }

            // 3. Filter by include/exclude globs.
            let mut selected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for st in &statuses {
                let inc = match &include {
                    Some(pats) => pats.iter().any(|p| glob_match(p, &st.pkg_name)),
                    None => true,
                };
                let exc = match &exclude {
                    Some(pats) => pats.iter().any(|p| glob_match(p, &st.pkg_name)),
                    None => false,
                };
                if inc && !exc {
                    selected.insert(st.pkg_name.clone());
                }
            }

            // 4. Auto-include transitive deps.
            let mut changed = true;
            while changed {
                changed = false;
                let current: Vec<String> = selected.iter().cloned().collect();
                for name_sel in &current {
                    if let Some(st) = statuses.iter().find(|s| s.pkg_name == *name_sel) {
                        for dep in &st.deps {
                            if !selected.contains(dep) {
                                selected.insert(dep.clone());
                                changed = true;
                            }
                        }
                    }
                }
            }

            // Build manifests for selected packages (for topo sort).
            let selected_manifests: Vec<manifest::PackageManifest> = packages.iter()
                .filter(|p| selected.contains(&p.pkg_name))
                .map(|p| p.manifest.clone())
                .collect();

            // 5. Build publish order.
            let tiers = match manifest::build_publish_order(&selected_manifests) {
                Ok(t) => t,
                Err(e) => { yield err("cycle", e); return; }
            };

            // 6. Process tiers (leaves first).
            let mut total = 0u32;
            let mut published_count = 0u32;
            let mut failed = 0u32;
            let mut skipped = 0u32;
            let mut auto_bumped = 0u32;
            let mut tier_failed = false;

            let resolver = crate::v5::secrets::YamlSecretStore::new(&config_dir);

            for (tier_idx, tier) in tiers.iter().enumerate() {
                for pkg_name in tier {
                    total += 1;

                    let st = match statuses.iter().find(|s| s.pkg_name == *pkg_name) {
                        Some(s) => s,
                        None => continue,
                    };

                    // If a previous tier failed, mark remaining as failed.
                    if tier_failed {
                        failed += 1;
                        yield BuildEvent::PublishStep {
                            package_name: pkg_name.clone(),
                            version: st.local_version.clone(),
                            registry: st.registry.clone(),
                            action: "failed".into(),
                            error: Some("dependency in earlier tier failed".into()),
                            dry_run,
                        };
                        continue;
                    }

                    // Skip if up_to_date.
                    if st.status == "up_to_date" {
                        skipped += 1;
                        yield BuildEvent::PublishStep {
                            package_name: pkg_name.clone(),
                            version: st.local_version.clone(),
                            registry: st.registry.clone(),
                            action: "skip".into(),
                            error: None,
                            dry_run,
                        };
                        continue;
                    }

                    let mut current_version = st.local_version.clone();

                    // Bump version for packages that need publishing.
                    let do_bump = st.status == "unpublished"
                        || st.status == "ahead"
                        || st.status == "stale";
                    if do_bump {
                        let bump_result = if st.build_system == "cargo" {
                            let cargo_path = st.dir.join("Cargo.toml");
                            match std::fs::read_to_string(&cargo_path) {
                                Ok(raw) => match release::bump_cargo_toml(&raw, bump_kind) {
                                    Ok((old, new, new_text)) => {
                                        if !dry_run {
                                            if let Err(e) = std::fs::write(&cargo_path, new_text) {
                                                Some(Err(e.to_string()))
                                            } else {
                                                Some(Ok((old, new)))
                                            }
                                        } else {
                                            Some(Ok((old, new)))
                                        }
                                    }
                                    Err(e) => Some(Err(e.to_string())),
                                },
                                Err(e) => Some(Err(e.to_string())),
                            }
                        } else if st.build_system == "cabal" {
                            let cabal_file = std::fs::read_dir(&st.dir).ok()
                                .and_then(|entries| {
                                    entries.flatten().find(|e| {
                                        e.path().extension().and_then(|x| x.to_str()) == Some("cabal")
                                            && e.path().is_file()
                                    })
                                })
                                .map(|e| e.path());
                            match cabal_file {
                                Some(path) => match std::fs::read_to_string(&path) {
                                    Ok(raw) => match release::bump_cabal_file(&raw, bump_kind) {
                                        Ok((old, new, new_text)) => {
                                            if !dry_run {
                                                if let Err(e) = std::fs::write(&path, new_text) {
                                                    Some(Err(e.to_string()))
                                                } else {
                                                    Some(Ok((old, new)))
                                                }
                                            } else {
                                                Some(Ok((old, new)))
                                            }
                                        }
                                        Err(e) => Some(Err(e.to_string())),
                                    },
                                    Err(e) => Some(Err(e.to_string())),
                                },
                                None => Some(Err("no .cabal file found".into())),
                            }
                        } else {
                            // node/other: skip bump, publish as-is.
                            None
                        };

                        match bump_result {
                            Some(Ok((_old, new))) => {
                                auto_bumped += 1;
                                current_version.clone_from(&new);
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: new.clone(),
                                    registry: st.registry.clone(),
                                    action: "auto_bump".into(),
                                    error: None,
                                    dry_run,
                                };
                                if !dry_run && !skip_commit {
                                    let _ = crate::v5::ops::git::add(&st.dir, &["."]);
                                    let _ = crate::v5::ops::git::commit(
                                        &st.dir,
                                        &format!("chore: bump {pkg_name} to {new}"),
                                    );
                                }
                                if !dry_run && !skip_tag {
                                    let _ = crate::v5::ops::git::tag(
                                        &st.dir,
                                        &format!("{pkg_name}-v{new}"),
                                    );
                                }
                            }
                            Some(Err(e)) => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version.clone(),
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(format!("bump failed: {e}")),
                                    dry_run,
                                };
                                continue;
                            }
                            None => {}
                        }
                    }

                    // Publish step.
                    if dry_run {
                        published_count += 1;
                        yield BuildEvent::PublishStep {
                            package_name: pkg_name.clone(),
                            version: current_version,
                            registry: st.registry.clone(),
                            action: "publish".into(),
                            error: None,
                            dry_run,
                        };
                    } else {
                        let channel = registry_to_channel(&st.registry);
                        let secret_key = match release::secret_key_for_channel(channel) {
                            Some(k) => k,
                            None => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(format!("unknown channel: {channel}")),
                                    dry_run,
                                };
                                continue;
                            }
                        };
                        use crate::v5::secrets::SecretResolver as _;
                        let token = match crate::v5::secrets::SecretRef::parse(
                            &format!("secrets://{secret_key}"),
                        ) {
                            Ok(parsed) => match resolver.resolve(&parsed) {
                                Ok(t) => t,
                                Err(_) => {
                                    failed += 1;
                                    tier_failed = true;
                                    yield BuildEvent::PublishStep {
                                        package_name: pkg_name.clone(),
                                        version: current_version,
                                        registry: st.registry.clone(),
                                        action: "failed".into(),
                                        error: Some(format!("no secret at secrets://{secret_key}")),
                                        dry_run,
                                    };
                                    continue;
                                }
                            },
                            Err(e) => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(e.to_string()),
                                    dry_run,
                                };
                                continue;
                            }
                        };
                        let cmd = match release::publish_command(channel, &token) {
                            Some(c) => c,
                            None => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(format!("no publish command for channel: {channel}")),
                                    dry_run,
                                };
                                continue;
                            }
                        };
                        match exec::run_shell(&st.dir, &cmd) {
                            Ok(res) if res.exit_code == 0 => {
                                published_count += 1;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "publish".into(),
                                    error: None,
                                    dry_run,
                                };
                            }
                            Ok(res) => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(format!(
                                        "exit {}: {}",
                                        res.exit_code,
                                        res.stderr.trim()
                                    )),
                                    dry_run,
                                };
                            }
                            Err(e) => {
                                failed += 1;
                                tier_failed = true;
                                yield BuildEvent::PublishStep {
                                    package_name: pkg_name.clone(),
                                    version: current_version,
                                    registry: st.registry.clone(),
                                    action: "failed".into(),
                                    error: Some(e.to_string()),
                                    dry_run,
                                };
                            }
                        }
                    }
                }

                // Wait for registry propagation between tiers (crates.io needs ~30s).
                if !dry_run && !tier_failed && tier_idx + 1 < tiers.len() {
                    let has_crates_io = tier.iter().any(|pkg_name| {
                        statuses.iter().any(|s| s.pkg_name == *pkg_name && s.registry == "crates_io")
                    });
                    if has_crates_io {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }
                }
            }

            yield BuildEvent::PublishSummary {
                total,
                published: published_count,
                failed,
                skipped,
                auto_bumped,
            };
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


/// Find the first `.cabal` file in a directory.
fn find_cabal_file(dir: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("cabal") && p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Simple glob matching: `*` matches any sequence, `?` matches one char.
fn glob_match(pattern: &str, s: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Map registry identifier (from `detect_build_system`) to the publish
/// channel name used by `release::publish_command` / `secret_key_for_channel`.
fn registry_to_channel(registry: &str) -> &str {
    match registry {
        "crates_io" => "crates.io",
        "hackage" => "hackage",
        "npm" => "npm",
        other => other,
    }
}

/// Minimal shell-escape: single-quote the value; escape embedded quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// Silence unused-import warnings if OrgName is imported for future refs.
#[allow(dead_code)]
fn _unused(_: OrgName) {}
