//! Containerized workspace validation (UNIFY-10)
//!
//! Runs builds and tests in Docker containers using the dependency graph
//! for ordering and CiConfig for per-repo overrides.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use super::dep_graph::DepGraph;

/// CI configuration for a specific repo
#[derive(Debug, Clone)]
pub struct RepoCiConfig {
    pub repo_name: String,
    pub build_command: Vec<String>,
    pub test_command: Vec<String>,
    pub dockerfile: Option<String>,
    pub skip: bool,
    pub timeout_secs: u64,
    pub env: Vec<(String, String)>,
}

impl Default for RepoCiConfig {
    fn default() -> Self {
        Self {
            repo_name: String::new(),
            build_command: vec!["cargo".to_string(), "build".to_string()],
            test_command: vec!["cargo".to_string(), "test".to_string()],
            dockerfile: None,
            skip: false,
            timeout_secs: 300,
            env: Vec::new(),
        }
    }
}

/// Status of a validation step
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Passed,
    Failed,
    Skipped,
}

impl std::fmt::Display for StepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Result of a single validation step
#[derive(Debug, Clone)]
pub struct ValidateStepResult {
    pub repo_name: String,
    pub step: String, // "build" or "test"
    pub status: StepStatus,
    pub duration_ms: u64,
    pub output: Option<String>,
}

/// Summary of the entire validation run
#[derive(Debug, Clone)]
pub struct ValidateSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: u64,
}

/// A validation plan built from the dep graph and CI configs
#[derive(Debug, Clone)]
pub struct ValidationPlan {
    /// Repos to validate in order (topological)
    pub steps: Vec<ValidationStep>,
    /// Whether to run tests after builds
    pub run_tests: bool,
    /// Docker image to use (if not per-repo)
    pub default_image: String,
}

#[derive(Debug, Clone)]
pub struct ValidationStep {
    pub repo_name: String,
    pub repo_path: String,
    pub ci_config: RepoCiConfig,
    pub tier: usize,
}

/// Build a validation plan from the dependency graph and CI configs.
pub fn build_validation_plan(
    graph: &DepGraph,
    ci_configs: &[(String, RepoCiConfig)],
    run_tests: bool,
) -> Result<ValidationPlan, String> {
    let tiers = graph
        .build_tiers()
        .map_err(|e| format!("Cycle in dependency graph: {}", e))?;

    let config_map: std::collections::HashMap<&str, &RepoCiConfig> = ci_configs
        .iter()
        .map(|(name, cfg)| (name.as_str(), cfg))
        .collect();

    let mut steps = Vec::new();
    for (tier_idx, tier) in tiers.iter().enumerate() {
        for &node_idx in tier {
            let node = &graph.nodes[node_idx];
            let ci = config_map
                .get(node.name.as_str())
                .cloned()
                .cloned()
                .unwrap_or_else(|| {
                    let mut cfg = RepoCiConfig::default();
                    cfg.repo_name = node.name.clone();
                    cfg
                });

            steps.push(ValidationStep {
                repo_name: node.name.clone(),
                repo_path: node.path.clone(),
                ci_config: ci,
                tier: tier_idx,
            });
        }
    }

    Ok(ValidationPlan {
        steps,
        run_tests,
        default_image: "rust:latest".to_string(),
    })
}

/// Execute a validation plan using Docker.
///
/// Returns step results as they complete. The workspace is bind-mounted
/// read-only at /workspace, with a writable overlay for build artifacts.
pub fn execute_validation(
    plan: &ValidationPlan,
    workspace_root: &Path,
    dry_run: bool,
) -> Vec<ValidateStepResult> {
    let mut results = Vec::new();

    for step in &plan.steps {
        if step.ci_config.skip {
            results.push(ValidateStepResult {
                repo_name: step.repo_name.clone(),
                step: "build".to_string(),
                status: StepStatus::Skipped,
                duration_ms: 0,
                output: Some("Skipped via ci.skip_validate".to_string()),
            });
            continue;
        }

        // Build step
        let build_result = if dry_run {
            ValidateStepResult {
                repo_name: step.repo_name.clone(),
                step: "build".to_string(),
                status: StepStatus::Passed,
                duration_ms: 0,
                output: Some(format!(
                    "[DRY RUN] Would run: {} in /workspace/{}",
                    step.ci_config.build_command.join(" "),
                    step.repo_path
                )),
            }
        } else {
            run_docker_step(
                workspace_root,
                &step.repo_path,
                &step.ci_config.build_command,
                &step.ci_config.env,
                &plan.default_image,
                step.ci_config.timeout_secs,
                "build",
                &step.repo_name,
            )
        };

        let build_passed = build_result.status == StepStatus::Passed;
        results.push(build_result);

        // Test step (only if build passed and tests requested)
        if plan.run_tests && build_passed {
            let test_result = if dry_run {
                ValidateStepResult {
                    repo_name: step.repo_name.clone(),
                    step: "test".to_string(),
                    status: StepStatus::Passed,
                    duration_ms: 0,
                    output: Some(format!(
                        "[DRY RUN] Would run: {} in /workspace/{}",
                        step.ci_config.test_command.join(" "),
                        step.repo_path
                    )),
                }
            } else {
                run_docker_step(
                    workspace_root,
                    &step.repo_path,
                    &step.ci_config.test_command,
                    &step.ci_config.env,
                    &plan.default_image,
                    step.ci_config.timeout_secs,
                    "test",
                    &step.repo_name,
                )
            };
            results.push(test_result);
        }
    }

    results
}

/// Run a single step inside a Docker container.
fn run_docker_step(
    workspace_root: &Path,
    repo_path: &str,
    command: &[String],
    env: &[(String, String)],
    image: &str,
    _timeout_secs: u64,
    step_name: &str,
    repo_name: &str,
) -> ValidateStepResult {
    let start = Instant::now();

    let workspace_str = workspace_root.to_string_lossy();
    let workdir = format!("/workspace/{}", repo_path);

    let mut docker_args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/workspace:ro", workspace_str),
        "--tmpfs".to_string(),
        "/workspace/target:exec".to_string(),
        "-w".to_string(),
        workdir,
    ];

    for (key, val) in env {
        docker_args.push("-e".to_string());
        docker_args.push(format!("{}={}", key, val));
    }

    docker_args.push(image.to_string());
    docker_args.extend(command.iter().cloned());

    let output = Command::new("docker")
        .args(&docker_args)
        .output();

    let duration_ms = start.elapsed().as_millis() as u64;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            if output.status.success() {
                ValidateStepResult {
                    repo_name: repo_name.to_string(),
                    step: step_name.to_string(),
                    status: StepStatus::Passed,
                    duration_ms,
                    output: if combined.is_empty() {
                        None
                    } else {
                        Some(combined)
                    },
                }
            } else {
                ValidateStepResult {
                    repo_name: repo_name.to_string(),
                    step: step_name.to_string(),
                    status: StepStatus::Failed,
                    duration_ms,
                    output: Some(combined),
                }
            }
        }
        Err(e) => ValidateStepResult {
            repo_name: repo_name.to_string(),
            step: step_name.to_string(),
            status: StepStatus::Failed,
            duration_ms,
            output: Some(format!("Failed to run docker: {}", e)),
        },
    }
}

/// Compute validation summary from step results.
pub fn summarize_results(results: &[ValidateStepResult]) -> ValidateSummary {
    let total = results.len();
    let passed = results.iter().filter(|r| r.status == StepStatus::Passed).count();
    let failed = results.iter().filter(|r| r.status == StepStatus::Failed).count();
    let skipped = results
        .iter()
        .filter(|r| r.status == StepStatus::Skipped)
        .count();
    let duration_ms = results.iter().map(|r| r.duration_ms).sum();

    ValidateSummary {
        total,
        passed,
        failed,
        skipped,
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_system::dep_graph::{DepGraph, DepNode};
    use crate::build_system::DepRef;

    #[test]
    fn test_build_validation_plan() {
        let nodes = vec![
            DepNode {
                name: "core".to_string(),
                version: Some("0.1.0".to_string()),
                build_system: "cargo".to_string(),
                path: "core".to_string(),
            },
            DepNode {
                name: "app".to_string(),
                version: Some("1.0.0".to_string()),
                build_system: "cargo".to_string(),
                path: "app".to_string(),
            },
        ];

        let deps = vec![(1, vec![DepRef {
            name: "core".to_string(),
            version_req: Some("0.1.0".to_string()),
            is_path_dep: false,
            path: None,
        }])];

        let graph = DepGraph::build(nodes, &deps);
        let plan = build_validation_plan(&graph, &[], true).unwrap();

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].repo_name, "core");
        assert_eq!(plan.steps[0].tier, 0);
        assert_eq!(plan.steps[1].repo_name, "app");
        assert_eq!(plan.steps[1].tier, 1);
    }

    #[test]
    fn test_dry_run_validation() {
        let nodes = vec![DepNode {
            name: "test".to_string(),
            version: Some("0.1.0".to_string()),
            build_system: "cargo".to_string(),
            path: "test".to_string(),
        }];

        let graph = DepGraph::build(nodes, &[]);
        let plan = build_validation_plan(&graph, &[], false).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let results = execute_validation(&plan, tmp.path(), true);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, StepStatus::Passed);
        assert!(results[0].output.as_ref().unwrap().contains("DRY RUN"));
    }

    #[test]
    fn test_summarize_results() {
        let results = vec![
            ValidateStepResult {
                repo_name: "a".to_string(),
                step: "build".to_string(),
                status: StepStatus::Passed,
                duration_ms: 100,
                output: None,
            },
            ValidateStepResult {
                repo_name: "b".to_string(),
                step: "build".to_string(),
                status: StepStatus::Failed,
                duration_ms: 200,
                output: None,
            },
            ValidateStepResult {
                repo_name: "c".to_string(),
                step: "build".to_string(),
                status: StepStatus::Skipped,
                duration_ms: 0,
                output: None,
            },
        ];

        let summary = summarize_results(&results);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.duration_ms, 300);
    }
}
