//! Package registry operations: package_diff, publish, bump.

use async_stream::stream;
use futures::Stream;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::commands::runner::{discover_or_bail, run_batch};
use crate::commands::workspace::build_publish_dep_graph;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::utils::{dry_prefix, glob_match};
use crate::package::DriftResult;

pub fn package_diff(
    path: String,
    filter: Option<String>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    stream! {
        let workspace_path = PathBuf::from(&path);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        // Build dep graph (excluding dev deps for publish ordering)
        let graph = build_publish_dep_graph(&ctx.repos);

        if graph.nodes.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No packages found in workspace.".to_string(),
            };
            return;
        }

        // Filter packages
        let indices: Vec<usize> = graph.nodes.iter().enumerate()
            .filter(|(_, node)| {
                if let Some(ref pat) = filter {
                    glob_match(pat, &node.name)
                } else {
                    true
                }
            })
            .map(|(i, _)| i)
            .collect();

        if indices.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No packages matched filter.".to_string(),
            };
            return;
        }

        yield HyperforgeEvent::Info {
            message: format!("Checking {} packages against registries...", indices.len()),
        };

        // Build work items — skip packages without registries or versions up front
        struct DiffItem {
            name: String,
            repo_path: PathBuf,
            build_system: crate::build_system::BuildSystemKind,
            local_version: String,
        }

        let mut work_items = Vec::new();
        let mut skip_events = Vec::new();

        for &idx in &indices {
            let node = &graph.nodes[idx];
            let build_system = match node.build_system.as_str() {
                "cargo" => crate::build_system::BuildSystemKind::Cargo,
                "cabal" => crate::build_system::BuildSystemKind::Cabal,
                "node" => crate::build_system::BuildSystemKind::Node,
                _ => crate::build_system::BuildSystemKind::Unknown,
            };

            if crate::package::registry_for(&build_system).is_none() {
                skip_events.push(HyperforgeEvent::Info {
                    message: format!("  {}: skipped (no registry for {})", node.name, node.build_system),
                });
                continue;
            }

            match &node.version {
                Some(v) => {
                    work_items.push(DiffItem {
                        name: node.name.clone(),
                        repo_path: workspace_path.join(&node.path),
                        build_system,
                        local_version: v.clone(),
                    });
                }
                None => {
                    skip_events.push(HyperforgeEvent::Info {
                        message: format!("  {}: skipped (no version)", node.name),
                    });
                }
            }
        }

        // Yield skip messages
        for event in skip_events {
            yield event;
        }

        // Run all registry queries + drift detection in parallel
        let results = run_batch(work_items, 8, |item| async move {
            let registry = match crate::package::registry_for(&item.build_system) {
                Some(r) => r,
                None => return None,
            };

            let registry_kind = registry.registry_kind();

            let published = match registry.published_version(&item.name).await {
                Ok(pv) => pv,
                Err(e) => {
                    return Some(Err(format!("  {}: registry query failed: {}", item.name, e)));
                }
            };

            let published_version = published.as_ref().map(|p| p.version.clone());

            let mut status = match &published_version {
                None => crate::hub::PackageStatus::Unpublished,
                Some(pub_v) => {
                    match crate::build_system::version::compare_versions(&item.local_version, pub_v) {
                        Some(std::cmp::Ordering::Greater) => crate::hub::PackageStatus::Ahead,
                        Some(std::cmp::Ordering::Equal) => crate::hub::PackageStatus::UpToDate,
                        Some(std::cmp::Ordering::Less) => crate::hub::PackageStatus::Stale,
                        None => crate::hub::PackageStatus::Stale,
                    }
                }
            };

            let mut changed_files = None;
            if matches!(status, crate::hub::PackageStatus::UpToDate) {
                match registry.detect_drift(&item.repo_path, &item.name, &item.local_version).await {
                    Ok(DriftResult::Drifted { changed_files: files }) => {
                        status = crate::hub::PackageStatus::Drifted;
                        if !files.is_empty() {
                            changed_files = Some(files);
                        }
                    }
                    Ok(DriftResult::Identical) | Ok(DriftResult::Unknown) => {}
                    Err(_) => {}
                }
            }

            Some(Ok(HyperforgeEvent::PackageDiff {
                package_name: item.name,
                build_system: item.build_system,
                local_version: item.local_version,
                published_version,
                registry: registry_kind,
                status,
                changed_files,
            }))
        }).await;

        // Yield results
        for result in results {
            match result {
                Ok(Some(Ok(event))) => yield event,
                Ok(Some(Err(msg))) => yield HyperforgeEvent::Error { message: msg },
                Ok(None) => {}
                Err(e) => yield HyperforgeEvent::Error { message: e },
            }
        }
    }
}

pub fn publish(
    path: String,
    filter: Option<String>,
    execute: Option<bool>,
    no_tag: Option<bool>,
    no_commit: Option<bool>,
    bump: Option<String>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let is_dry_run = !execute.unwrap_or(false);
    let skip_tags = no_tag.unwrap_or(false);
    let skip_commits = no_commit.unwrap_or(false);
    let bump_kind = crate::types::VersionBump::from_str_or_patch(bump.as_deref());
    let dry_prefix = dry_prefix(is_dry_run);

    stream! {
        let workspace_path = PathBuf::from(&path);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        // Build dep graph (excluding dev deps for publish ordering)
        let graph = build_publish_dep_graph(&ctx.repos);

        if graph.nodes.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No packages found in workspace.".to_string(),
            };
            return;
        }

        // Resolve targets from filter
        let targets: Vec<usize> = graph.nodes.iter().enumerate()
            .filter(|(_, node)| {
                if let Some(ref pat) = filter {
                    glob_match(pat, &node.name)
                } else {
                    // Default: all packages with a registry
                    match node.build_system.as_str() {
                        "cargo" | "cabal" => true,
                        _ => false,
                    }
                }
            })
            .map(|(i, _)| i)
            .collect();

        if targets.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No publishable packages matched filter.".to_string(),
            };
            return;
        }

        // Build publish plan (queries registries, computes transitive closure)
        let plan = match crate::build_system::publish::build_publish_plan(
            &graph,
            &targets,
            &workspace_path,
            &bump_kind,
        ).await {
            Ok(p) => p,
            Err(e) => {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to build publish plan: {}", e),
                };
                return;
            }
        };

        // Report exclusions
        for (name, reason) in &plan.excluded {
            yield HyperforgeEvent::Info {
                message: format!("{}Excluded {}: {}", dry_prefix, name, reason),
            };
        }

        yield HyperforgeEvent::Info {
            message: format!(
                "{}Publish plan: {} packages in dependency order",
                dry_prefix,
                plan.steps.len()
            ),
        };

        // Track failed nodes to skip dependents
        let mut failed_nodes: HashSet<usize> = HashSet::new();
        let mut published_count = 0usize;
        let mut auto_bumped_count = 0usize;
        let mut skipped_count = 0usize;
        let mut failed_count = 0usize;
        let mut tags_created = 0usize;

        for step in &plan.steps {
            // Check if any dependency failed
            let dep_failed = graph.direct_deps(step.node_idx)
                .iter()
                .any(|dep_idx| failed_nodes.contains(dep_idx));

            if dep_failed {
                failed_nodes.insert(step.node_idx);
                failed_count += 1;
                yield HyperforgeEvent::PublishStep {
                    package_name: step.name.clone(),
                    version: step.target_version.clone(),
                    registry: crate::hub::PackageRegistry::CratesIo, // placeholder
                    action: crate::hub::PublishActionKind::Failed,
                    success: false,
                    error: Some("dependency failed to publish".to_string()),
                };
                continue;
            }

            let build_system = &step.build_system;
            let registry = match crate::package::registry_for(build_system) {
                Some(r) => r,
                None => continue,
            };
            let registry_kind = registry.registry_kind();

            match &step.action {
                crate::build_system::publish::PublishAction::Skip => {
                    skipped_count += 1;
                    yield HyperforgeEvent::PublishStep {
                        package_name: step.name.clone(),
                        version: step.target_version.clone(),
                        registry: registry_kind,
                        action: crate::hub::PublishActionKind::Skip,
                        success: true,
                        error: None,
                    };
                }
                crate::build_system::publish::PublishAction::Error(msg) => {
                    failed_nodes.insert(step.node_idx);
                    failed_count += 1;
                    yield HyperforgeEvent::PublishStep {
                        package_name: step.name.clone(),
                        version: step.target_version.clone(),
                        registry: registry_kind,
                        action: crate::hub::PublishActionKind::Failed,
                        success: false,
                        error: Some(msg.clone()),
                    };
                }
                action => {
                    let is_auto_bump = matches!(action, crate::build_system::publish::PublishAction::AutoBump);
                    let action_kind = match action {
                        crate::build_system::publish::PublishAction::Publish => crate::hub::PublishActionKind::Publish,
                        crate::build_system::publish::PublishAction::AutoBump => crate::hub::PublishActionKind::AutoBump,
                        crate::build_system::publish::PublishAction::InitialPublish => crate::hub::PublishActionKind::InitialPublish,
                        _ => unreachable!(),
                    };

                    // Auto-bump: edit manifest and optionally commit
                    if is_auto_bump && !is_dry_run {
                        if let Err(e) = crate::build_system::version::set_package_version(
                            &step.path,
                            build_system,
                            &step.target_version,
                        ) {
                            failed_nodes.insert(step.node_idx);
                            failed_count += 1;
                            yield HyperforgeEvent::PublishStep {
                                package_name: step.name.clone(),
                                version: step.target_version.clone(),
                                registry: registry_kind,
                                action: crate::hub::PublishActionKind::Failed,
                                success: false,
                                error: Some(format!("version bump failed: {}", e)),
                            };
                            continue;
                        }

                        if !skip_commits {
                            // Stage and commit the version bump
                            let manifest_file = match build_system {
                                crate::build_system::BuildSystemKind::Cargo => "Cargo.toml",
                                crate::build_system::BuildSystemKind::Cabal => {
                                    // Find .cabal file name
                                    &step.name
                                }
                                _ => "package.json",
                            };

                            // For cabal, we need the actual filename
                            if *build_system == crate::build_system::BuildSystemKind::Cabal {
                                // Stage all .cabal files
                                let _ = Git::add(&step.path, "*.cabal");
                            } else {
                                let _ = Git::add(&step.path, manifest_file);
                            }

                            let commit_msg = format!(
                                "chore: bump {} to {}",
                                step.name, step.target_version
                            );
                            let _ = Git::commit(&step.path, &commit_msg);
                        }

                        auto_bumped_count += 1;
                    } else if is_auto_bump {
                        // Dry run auto-bump
                        auto_bumped_count += 1;
                    }

                    // Publish
                    let result = registry.publish(&step.path, &step.name, is_dry_run).await;

                    match result {
                        Ok(pr) if pr.success => {
                            published_count += 1;

                            yield HyperforgeEvent::PublishStep {
                                package_name: step.name.clone(),
                                version: step.target_version.clone(),
                                registry: registry_kind.clone(),
                                action: action_kind,
                                success: true,
                                error: None,
                            };

                            // Git tag
                            if !skip_tags && !is_dry_run {
                                let tag_name = format!("{}-v{}", step.name, step.target_version);
                                let tag_msg = format!("Release {} v{}", step.name, step.target_version);
                                if let Err(e) = Git::tag(&step.path, &tag_name, Some(&tag_msg)) {
                                    yield HyperforgeEvent::Info {
                                        message: format!("  Warning: failed to create tag {}: {}", tag_name, e),
                                    };
                                } else {
                                    tags_created += 1;
                                    yield HyperforgeEvent::PublishStep {
                                        package_name: step.name.clone(),
                                        version: step.target_version.clone(),
                                        registry: registry_kind.clone(),
                                        action: crate::hub::PublishActionKind::Tag,
                                        success: true,
                                        error: None,
                                    };
                                }
                            }
                        }
                        Ok(pr) => {
                            // Publish returned but was not successful
                            failed_nodes.insert(step.node_idx);
                            failed_count += 1;
                            yield HyperforgeEvent::PublishStep {
                                package_name: step.name.clone(),
                                version: step.target_version.clone(),
                                registry: registry_kind,
                                action: crate::hub::PublishActionKind::Failed,
                                success: false,
                                error: pr.error,
                            };
                        }
                        Err(e) => {
                            failed_nodes.insert(step.node_idx);
                            failed_count += 1;
                            yield HyperforgeEvent::PublishStep {
                                package_name: step.name.clone(),
                                version: step.target_version.clone(),
                                registry: registry_kind,
                                action: crate::hub::PublishActionKind::Failed,
                                success: false,
                                error: Some(format!("{}", e)),
                            };
                        }
                    }
                }
            }
        }

        yield HyperforgeEvent::PublishSummary {
            total: plan.steps.len(),
            published: published_count,
            auto_bumped: auto_bumped_count,
            skipped: skipped_count,
            failed: failed_count,
            tags_created,
        };
    }
}

pub fn bump(
    path: String,
    filter: Option<String>,
    bump: Option<String>,
    commit: Option<bool>,
    dry_run: Option<bool>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let bump_kind = crate::types::VersionBump::from_str_or_patch(bump.as_deref());
    let auto_commit = commit.unwrap_or(false);
    let is_dry_run = dry_run.unwrap_or(false);
    let dry_prefix = dry_prefix(is_dry_run);

    stream! {
        let workspace_path = PathBuf::from(&path);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        let repos: Vec<_> = if let Some(ref pattern) = filter {
            ctx.repos.iter().filter(|r| {
                let name = r.package_name.as_deref().unwrap_or(&r.dir_name);
                glob_match(pattern, name)
            }).collect()
        } else {
            ctx.repos.iter().collect()
        };

        if repos.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No packages matched filter.".to_string(),
            };
            return;
        }

        yield HyperforgeEvent::Info {
            message: format!("{}Bumping {} packages ({:?})...", dry_prefix, repos.len(), bump_kind),
        };

        let mut bumped = 0usize;
        let mut failed = 0usize;

        for repo in &repos {
            let name = repo.package_name.as_deref().unwrap_or(&repo.dir_name);
            let current_version = match &repo.package_version {
                Some(v) => v.clone(),
                None => {
                    yield HyperforgeEvent::Info {
                        message: format!("  {}: skipped (no version)", name),
                    };
                    continue;
                }
            };

            let parsed = match crate::build_system::version::SemVer::parse(&current_version) {
                Some(v) => v,
                None => {
                    yield HyperforgeEvent::Info {
                        message: format!("  {}: skipped (unparseable version: {})", name, current_version),
                    };
                    continue;
                }
            };

            let new_version = parsed.bump(&bump_kind).to_string();

            if !is_dry_run {
                match crate::build_system::version::set_package_version(
                    &repo.path,
                    &repo.build_system,
                    &new_version,
                ) {
                    Ok(_) => {
                        bumped += 1;

                        if auto_commit {
                            let manifest_file = match repo.build_system {
                                crate::build_system::BuildSystemKind::Cargo => "Cargo.toml",
                                crate::build_system::BuildSystemKind::Cabal => "*.cabal",
                                _ => "package.json",
                            };
                            let _ = Git::add(&repo.path, manifest_file);
                            let commit_msg = format!("chore: bump {} to {}", name, new_version);
                            let _ = Git::commit(&repo.path, &commit_msg);
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        yield HyperforgeEvent::Error {
                            message: format!("  {}: bump failed: {}", name, e),
                        };
                        continue;
                    }
                }
            } else {
                bumped += 1;
            }

            yield HyperforgeEvent::PublishStep {
                package_name: name.to_string(),
                version: new_version.clone(),
                registry: match repo.build_system {
                    crate::build_system::BuildSystemKind::Cargo => crate::hub::PackageRegistry::CratesIo,
                    crate::build_system::BuildSystemKind::Cabal => crate::hub::PackageRegistry::Hackage,
                    _ => crate::hub::PackageRegistry::Npm,
                },
                action: crate::hub::PublishActionKind::AutoBump,
                success: true,
                error: None,
            };
        }

        yield HyperforgeEvent::Info {
            message: format!("{}Bump complete: {} bumped, {} failed", dry_prefix, bumped, failed),
        };
    }
}
