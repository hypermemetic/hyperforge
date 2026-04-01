//! Build release orchestrator: cross-compile, package, create forge releases, upload assets.

use async_stream::stream;
use futures::Stream;
use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::releases::codeberg::CodebergReleaseAdapter;
use crate::adapters::releases::github::GitHubReleaseAdapter;
use crate::adapters::releases::ReleasePort;
use crate::auth::YamlAuthProvider;
use crate::build_system::cross_compile::{compile_and_package, host_triple, TargetTriple};
use crate::build_system::{self, BinaryTarget};
use crate::commands::runner::discover_or_bail;
use crate::commands::workspace::DiscoveredRepo;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::utils::RepoFilter;

fn make_auth() -> Result<Arc<YamlAuthProvider>, String> {
    YamlAuthProvider::new()
        .map(Arc::new)
        .map_err(|e| format!("Failed to create auth provider: {}", e))
}

fn make_release_adapter(
    forge: &str,
    auth: Arc<YamlAuthProvider>,
    org: &str,
) -> Result<Box<dyn ReleasePort>, String> {
    match forge {
        "github" => GitHubReleaseAdapter::new(auth, org)
            .map(|a| Box::new(a) as Box<dyn ReleasePort>)
            .map_err(|e| format!("github: {}", e)),
        "codeberg" => CodebergReleaseAdapter::new(auth, org)
            .map(|a| Box::new(a) as Box<dyn ReleasePort>)
            .map_err(|e| format!("codeberg: {}", e)),
        other => Err(format!("Releases not supported for forge: {}", other)),
    }
}

/// Guess content type from filename extension
fn guess_content_type(filename: &str) -> &'static str {
    if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        "application/gzip"
    } else if filename.ends_with(".zip") {
        "application/zip"
    } else {
        "application/octet-stream"
    }
}

/// Collect binary targets and version for a discovered repo.
/// Returns None if the repo has no binary targets.
fn repo_binary_info(repo: &DiscoveredRepo) -> Option<(Vec<BinaryTarget>, String)> {
    let targets = build_system::binary_targets(&repo.path);
    if targets.is_empty() {
        return None;
    }
    let version = repo.package_version.clone()?;
    Some((targets, version))
}

/// Resolve which forges to target from a repo's config.
fn resolve_forges(repo: &DiscoveredRepo, forge_override: &Option<String>) -> Vec<String> {
    if let Some(ref f) = forge_override {
        f.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    } else {
        repo.forges().iter().map(|s| s.to_string()).collect()
    }
}

/// Resolve the org from a discovered repo's config
fn resolve_org(repo: &DiscoveredRepo) -> Option<String> {
    repo.org().map(|s| s.to_string())
}

/// Parse comma-separated target triples, defaulting to native
fn parse_targets(targets: &Option<String>) -> Vec<TargetTriple> {
    match targets {
        Some(s) if !s.is_empty() => s
            .split(',')
            .map(|t| TargetTriple::new(t.trim()))
            .collect(),
        _ => vec![TargetTriple::new(host_triple())],
    }
}

/// Determine if a path is itself a single repo (has Cargo.toml or *.cabal)
fn is_single_repo(path: &std::path::Path) -> bool {
    path.join("Cargo.toml").exists()
        || std::fs::read_dir(path)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| {
                        e.path()
                            .extension()
                            .map(|ext| ext == "cabal")
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false)
}

/// Build a DiscoveredRepo from a single repo path (not a workspace)
fn discover_single_repo(path: &std::path::Path) -> Result<DiscoveredRepo, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {}", e))?;

    let dir_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let primary_bs = build_system::detect_build_system(&path);
    let all_bs = build_system::detect_all_build_systems(&path);
    let deps = build_system::parse_dependencies(&path, &primary_bs);
    let pkg_name = build_system::package_name(&path, &primary_bs);
    let pkg_version = build_system::package_version(&path, &primary_bs);
    let config = crate::config::HyperforgeConfig::load(&path).ok();
    let is_git = Git::is_repo(&path);

    Ok(DiscoveredRepo {
        path,
        dir_name,
        config,
        is_git_repo: is_git,
        is_hyperforge_repo: crate::config::HyperforgeConfig::exists(&std::path::PathBuf::from(".")),
        build_system: primary_bs,
        build_systems: all_bs,
        dependencies: deps,
        package_name: pkg_name,
        package_version: pkg_version,
    })
}

pub fn release(
    path: String,
    tag: String,
    targets: Option<String>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    forge: Option<String>,
    title: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    dry_run: Option<bool>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let filter = RepoFilter::new(include, exclude);
    let is_dry_run = dry_run.unwrap_or(false);
    let is_draft = draft.unwrap_or(false);
    let release_title = title.unwrap_or_else(|| tag.clone());
    let release_body = body.unwrap_or_default();
    let target_triples = parse_targets(&targets);

    stream! {
        let dry_prefix = if is_dry_run { "[dry-run] " } else { "" };
        let workspace_path = PathBuf::from(&path);

        // Determine repos: single repo or workspace
        let repos: Vec<DiscoveredRepo> = if is_single_repo(&workspace_path) {
            match discover_single_repo(&workspace_path) {
                Ok(r) => vec![r],
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to discover repo: {}", e),
                    };
                    return;
                }
            }
        } else {
            let ctx = match discover_or_bail(&workspace_path) {
                Ok(ctx) => ctx,
                Err(event) => {
                    yield event;
                    return;
                }
            };
            ctx.repos
                .into_iter()
                .filter(|r| filter.matches(&r.dir_name))
                .collect()
        };

        if repos.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No repos matched filter.".to_string(),
            };
            return;
        }

        yield HyperforgeEvent::Info {
            message: format!(
                "{}Release {} — {} repo(s), {} target(s)",
                dry_prefix,
                tag,
                repos.len(),
                target_triples.len(),
            ),
        };

        let mut summary_repos = 0usize;
        let mut summary_targets = 0usize;
        let mut summary_forges_set = std::collections::HashSet::new();
        let mut summary_assets = 0usize;
        let mut summary_failed = 0usize;

        for repo in &repos {
            let (bin_targets, version) = match repo_binary_info(repo) {
                Some(info) => info,
                None => {
                    yield HyperforgeEvent::Info {
                        message: format!("Skipping {} (no binary targets or no version)", repo.dir_name),
                    };
                    continue;
                }
            };

            summary_repos += 1;
            let binary_names: Vec<String> = bin_targets.iter().map(|b| b.name.clone()).collect();
            let bs_kind = &bin_targets[0].build_system;
            let repo_name = repo.dir_name.clone();

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}  {} v{} — {} binary target(s): {}",
                    dry_prefix,
                    repo_name,
                    version,
                    binary_names.len(),
                    binary_names.join(", "),
                ),
            };

            // Output dir for archives
            let output_dir = repo.path.join("target").join("dist");

            // Compile and package for each target triple
            let mut archives: Vec<PathBuf> = Vec::new();

            for triple in &target_triples {
                yield HyperforgeEvent::ReleaseBuildStep {
                    repo_name: repo_name.clone(),
                    target: triple.triple.clone(),
                    status: "compiling".to_string(),
                    detail: Some(format!("{} binary(ies)", binary_names.len())),
                };

                if is_dry_run {
                    let stem = format!(
                        "{}-{}-v{}{}",
                        repo_name,
                        triple.triple,
                        version,
                        triple.archive_format().extension()
                    );
                    yield HyperforgeEvent::ReleaseBuildStep {
                        repo_name: repo_name.clone(),
                        target: triple.triple.clone(),
                        status: "packaging".to_string(),
                        detail: Some(format!("would create {}", stem)),
                    };
                    summary_targets += 1;
                    continue;
                }

                let results = compile_and_package(
                    &repo.path,
                    bs_kind,
                    &[triple.clone()],
                    &binary_names,
                    &version,
                    &output_dir,
                )
                .await;

                for result in results {
                    if result.success {
                        if let Some(ref archive_path) = result.archive_path {
                            yield HyperforgeEvent::ReleaseBuildStep {
                                repo_name: repo_name.clone(),
                                target: triple.triple.clone(),
                                status: "packaging".to_string(),
                                detail: Some(format!(
                                    "created {}",
                                    archive_path.file_name().unwrap_or_default().to_string_lossy()
                                )),
                            };
                            archives.push(archive_path.clone());
                            summary_targets += 1;
                        }
                    } else {
                        let err = result.error.unwrap_or_else(|| "unknown error".to_string());
                        yield HyperforgeEvent::Error {
                            message: format!(
                                "{}: compile/package failed for {}: {}",
                                repo_name, triple.triple, err
                            ),
                        };
                        summary_failed += 1;
                    }
                }
            }

            // Create git tag if needed
            if !is_dry_run && !Git::tag_exists(&repo.path, &tag) {
                yield HyperforgeEvent::Info {
                    message: format!("  Creating git tag {} in {}", tag, repo_name),
                };
                let tag_output = tokio::process::Command::new("git")
                    .args(["tag", "-a", &tag, "-m", &format!("Release {}", tag)])
                    .current_dir(&repo.path)
                    .output()
                    .await;
                match tag_output {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to create tag {}: {}", tag, stderr),
                        };
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to run git tag: {}", e),
                        };
                    }
                }
            } else if is_dry_run {
                if Git::tag_exists(&repo.path, &tag) {
                    yield HyperforgeEvent::Info {
                        message: format!("{}  Tag {} already exists in {}", dry_prefix, tag, repo_name),
                    };
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("{}  Would create git tag {} in {}", dry_prefix, tag, repo_name),
                    };
                }
            }

            // Upload to forges
            let target_forges = resolve_forges(repo, &forge);
            if target_forges.is_empty() {
                yield HyperforgeEvent::Info {
                    message: format!("  {} has no configured forges — skipping release upload", repo_name),
                };
                continue;
            }

            let org = match resolve_org(repo) {
                Some(o) => o,
                None => {
                    yield HyperforgeEvent::Info {
                        message: format!("  {} has no org configured — skipping release upload", repo_name),
                    };
                    continue;
                }
            };

            if is_dry_run {
                for forge_name in &target_forges {
                    yield HyperforgeEvent::Info {
                        message: format!(
                            "{}  Would create release {} on {}/{} ({}), upload {} asset(s)",
                            dry_prefix,
                            tag,
                            org,
                            repo_name,
                            forge_name,
                            target_triples.len(),
                        ),
                    };
                    summary_forges_set.insert(forge_name.clone());
                }
                continue;
            }

            // Real upload path
            let auth = match make_auth() {
                Ok(a) => a,
                Err(e) => {
                    yield HyperforgeEvent::Error { message: e };
                    summary_failed += 1;
                    continue;
                }
            };

            for forge_name in &target_forges {
                let adapter = match make_release_adapter(forge_name, auth.clone(), &org) {
                    Ok(a) => a,
                    Err(e) => {
                        yield HyperforgeEvent::ReleaseCreate {
                            repo_name: repo_name.clone(),
                            forge: forge_name.clone(),
                            tag: tag.clone(),
                            success: false,
                            error: Some(e),
                        };
                        summary_failed += 1;
                        continue;
                    }
                };

                // Create release
                match adapter
                    .create_release(
                        &org,
                        &repo_name,
                        &tag,
                        &release_title,
                        &release_body,
                        is_draft,
                        false,
                    )
                    .await
                {
                    Ok(release_info) => {
                        yield HyperforgeEvent::ReleaseCreate {
                            repo_name: repo_name.clone(),
                            forge: forge_name.clone(),
                            tag: tag.clone(),
                            success: true,
                            error: None,
                        };
                        summary_forges_set.insert(forge_name.clone());

                        // Upload each archive
                        for archive_path in &archives {
                            let filename = archive_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown")
                                .to_string();

                            yield HyperforgeEvent::ReleaseBuildStep {
                                repo_name: repo_name.clone(),
                                target: "all".to_string(),
                                status: "uploading".to_string(),
                                detail: Some(format!("{} to {}", filename, forge_name)),
                            };

                            let data = match tokio::fs::read(archive_path).await {
                                Ok(d) => d,
                                Err(e) => {
                                    yield HyperforgeEvent::ReleaseUpload {
                                        repo_name: repo_name.clone(),
                                        forge: forge_name.clone(),
                                        tag: tag.clone(),
                                        asset_name: filename.clone(),
                                        size_bytes: 0,
                                        success: false,
                                        error: Some(format!("Failed to read archive: {}", e)),
                                    };
                                    summary_failed += 1;
                                    continue;
                                }
                            };

                            let size_bytes = data.len() as u64;
                            let content_type = guess_content_type(&filename);

                            match adapter
                                .upload_asset(
                                    &org,
                                    &repo_name,
                                    release_info.id,
                                    &filename,
                                    content_type,
                                    data,
                                )
                                .await
                            {
                                Ok(_) => {
                                    yield HyperforgeEvent::ReleaseUpload {
                                        repo_name: repo_name.clone(),
                                        forge: forge_name.clone(),
                                        tag: tag.clone(),
                                        asset_name: filename,
                                        size_bytes,
                                        success: true,
                                        error: None,
                                    };
                                    summary_assets += 1;
                                }
                                Err(e) => {
                                    yield HyperforgeEvent::ReleaseUpload {
                                        repo_name: repo_name.clone(),
                                        forge: forge_name.clone(),
                                        tag: tag.clone(),
                                        asset_name: filename,
                                        size_bytes,
                                        success: false,
                                        error: Some(e.to_string()),
                                    };
                                    summary_failed += 1;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield HyperforgeEvent::ReleaseCreate {
                            repo_name: repo_name.clone(),
                            forge: forge_name.clone(),
                            tag: tag.clone(),
                            success: false,
                            error: Some(e.to_string()),
                        };
                        summary_failed += 1;
                    }
                }
            }
        }

        yield HyperforgeEvent::ReleaseSummary {
            repos: summary_repos,
            targets: summary_targets,
            forges: summary_forges_set.len(),
            assets_uploaded: summary_assets,
            failed: summary_failed,
        };
    }
}
