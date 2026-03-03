//! Cross-repo execution and validation: exec, validate.

use async_stream::stream;
use futures::Stream;
use std::path::PathBuf;

use crate::commands::runner::discover_or_bail;
use crate::commands::workspace::build_dep_graph;
use crate::git::Git;
use crate::hub::HyperforgeEvent;
use crate::hubs::utils::{dry_prefix, glob_match};

pub fn exec(
    path: String,
    command: String,
    filter: Option<String>,
    sequential: Option<bool>,
    dirty: Option<bool>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let is_sequential = sequential.unwrap_or(false);
    let only_dirty = dirty.unwrap_or(false);

    stream! {
        let workspace_path = PathBuf::from(&path);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        // Filter repos by name glob if provided
        let mut repos: Vec<&crate::commands::workspace::DiscoveredRepo> = if let Some(ref pattern) = filter {
            ctx.repos.iter().filter(|r| {
                glob_match(pattern, &r.dir_name)
            }).collect()
        } else {
            ctx.repos.iter().collect()
        };

        // Filter to dirty repos only
        if only_dirty {
            repos.retain(|r| {
                match Git::repo_status(&r.path) {
                    Ok(s) => s.has_changes || s.has_staged || s.has_untracked,
                    Err(_) => false,
                }
            });
        }

        if repos.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No repos matched filter.".to_string(),
            };
            return;
        }

        yield HyperforgeEvent::Info {
            message: format!(
                "Executing `{}` across {} repos{}...",
                command,
                repos.len(),
                if is_sequential { " (sequential)" } else { " (parallel)" }
            ),
        };

        // Build exec inputs
        let exec_inputs: Vec<_> = repos.iter()
            .map(|r| (r.dir_name.clone(), r.path.clone(), command.clone()))
            .collect();

        let concurrency = if is_sequential { 1 } else { 0 }; // 0 = unbounded
        let exec_results = crate::commands::runner::run_batch(
            exec_inputs,
            concurrency,
            |(repo_name, repo_path, cmd)| async move {
                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .current_dir(&repo_path)
                    .output()
                    .await;

                match output {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let exit_code = output.status.code().unwrap_or(-1);
                        (repo_name, exit_code, stdout, stderr)
                    }
                    Err(e) => {
                        (repo_name, -1, String::new(), format!("Failed to execute: {}", e))
                    }
                }
            },
        ).await;

        for result in exec_results {
            match result {
                Ok((repo_name, exit_code, stdout, stderr)) => {
                    yield HyperforgeEvent::ExecResult {
                        repo_name,
                        exit_code,
                        stdout,
                        stderr,
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Task error: {}", e),
                    };
                }
            }
        }

        // Summary
        let total = repos.len();
        yield HyperforgeEvent::Info {
            message: format!("Exec complete: ran across {} repos", total),
        };
    }
}

pub fn validate(
    path: String,
    test: Option<bool>,
    dry_run: Option<bool>,
    image: Option<String>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let is_dry_run = dry_run.unwrap_or(false);
    let run_tests = test.unwrap_or(false);
    let docker_image = image.unwrap_or_else(|| "rust:latest".to_string());

    stream! {
        let workspace_path = PathBuf::from(&path);
        let dry_prefix = dry_prefix(is_dry_run);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        // Build dep graph
        let graph = build_dep_graph(&ctx.repos);

        // Build CI configs from per-repo .hyperforge/config.toml [ci] sections
        let ci_configs: Vec<(String, crate::build_system::validate::RepoCiConfig)> = ctx
            .repos
            .iter()
            .filter_map(|repo| {
                let config = repo.config.as_ref()?;
                let ci = config.ci.as_ref()?;
                let name = repo.effective_name();

                let mut cfg = crate::build_system::validate::RepoCiConfig::default();
                cfg.repo_name = name.clone();
                if !ci.build.is_empty() {
                    cfg.build_command = ci.build.clone();
                }
                if !ci.test.is_empty() {
                    cfg.test_command = ci.test.clone();
                }
                cfg.dockerfile = ci.dockerfile.clone();
                cfg.skip = ci.skip_validate;
                cfg.timeout_secs = ci.timeout_secs;
                cfg.env = ci.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                Some((name, cfg))
            })
            .collect();

        // Build validation plan
        let plan = match crate::build_system::validate::build_validation_plan(
            &graph,
            &ci_configs,
            run_tests,
        ) {
            Ok(mut p) => {
                p.default_image = docker_image;
                p
            }
            Err(e) => {
                yield HyperforgeEvent::Error {
                    message: format!("Failed to build validation plan: {}", e),
                };
                return;
            }
        };

        yield HyperforgeEvent::Info {
            message: format!(
                "{}Validation plan: {} steps, tests={}",
                dry_prefix,
                plan.steps.len(),
                run_tests
            ),
        };

        // Execute validation
        let results = crate::build_system::validate::execute_validation(
            &plan,
            &ctx.root,
            is_dry_run,
        );

        for result in &results {
            yield HyperforgeEvent::ValidateStep {
                repo_name: result.repo_name.clone(),
                step: result.step.clone(),
                status: format!("{}", result.status),
                duration_ms: result.duration_ms,
            };
        }

        let summary = crate::build_system::validate::summarize_results(&results);
        yield HyperforgeEvent::ValidateSummary {
            total: summary.total,
            passed: summary.passed,
            failed: summary.failed,
            skipped: summary.skipped,
            duration_ms: summary.duration_ms,
        };

        if summary.failed > 0 {
            yield HyperforgeEvent::Error {
                message: format!(
                    "Validation failed: {}/{} steps failed",
                    summary.failed, summary.total
                ),
            };
        } else {
            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Validation passed: {}/{} steps succeeded",
                    dry_prefix, summary.passed, summary.total
                ),
            };
        }
    }
}
