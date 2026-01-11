//! Pulumi bridge for running infrastructure operations
//!
//! This module spawns the `./forge` subprocess from the Pulumi project directory
//! and streams events back for real-time progress updates.

use async_stream::stream;
use futures::Stream;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::events::{PulumiEvent, PulumiOperation};
use crate::storage::HyperforgePaths;

/// Captured outputs from Pulumi stack after apply
#[derive(Debug, Clone)]
pub struct PulumiOutputs {
    pub repos: HashMap<String, RepoOutput>,
}

/// Repository output from a Pulumi stack
#[derive(Debug, Clone)]
pub struct RepoOutput {
    pub github_url: Option<String>,
    pub github_id: Option<String>,
    pub codeberg_url: Option<String>,
    pub codeberg_id: Option<String>,
}

/// Bridge to run Pulumi operations via the `./forge` wrapper script
pub struct PulumiBridge {
    pulumi_dir: PathBuf,
}

impl PulumiBridge {
    /// Create a new PulumiBridge
    ///
    /// The Pulumi project is expected to be in `~/.hypermemetic-infra/projects/forge-pulumi`
    pub fn new(_paths: &HyperforgePaths) -> Self {
        let pulumi_dir = PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".hypermemetic-infra/projects/forge-pulumi");

        Self { pulumi_dir }
    }

    /// Run pulumi preview and stream events
    ///
    /// This shows what changes would be made without actually applying them.
    pub fn preview(
        &self,
        org_name: &str,
        repos_file: &PathBuf,
        staged_file: &PathBuf,
    ) -> impl Stream<Item = PulumiEvent> + Send + 'static {
        let pulumi_dir = self.pulumi_dir.clone();
        let org_name = org_name.to_string();
        let repos_file = repos_file.clone();
        let staged_file = staged_file.clone();

        stream! {
            yield PulumiEvent::PreviewStarted {
                org_name: org_name.clone(),
                stack: org_name.clone(),
            };

            // Build the command
            let mut cmd = Command::new("./forge");
            cmd.current_dir(&pulumi_dir)
                .arg("preview")
                .env("HYPERFORGE_ORG", &org_name)
                .env("HYPERFORGE_REPOS_FILE", &repos_file)
                .env("HYPERFORGE_STAGED_FILE", &staged_file)
                .env("PULUMI_CONFIG_PASSPHRASE", "") // Empty passphrase for local state
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            match cmd.spawn() {
                Ok(mut child) => {
                    let stdout = child.stdout.take().unwrap();
                    let mut reader = BufReader::new(stdout).lines();

                    let mut creates = 0usize;
                    let mut updates = 0usize;
                    let mut deletes = 0usize;
                    let mut unchanged = 0usize;

                    while let Ok(Some(line)) = reader.next_line().await {
                        yield PulumiEvent::Output { line: line.clone() };

                        // Parse Pulumi output for structured events
                        if line.contains("+ ") {
                            creates += 1;
                            if let Some(resource) = parse_resource_line(&line, "+") {
                                yield PulumiEvent::ResourcePlanned {
                                    operation: PulumiOperation::Create,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                };
                            }
                        } else if line.contains("~ ") {
                            updates += 1;
                            if let Some(resource) = parse_resource_line(&line, "~") {
                                yield PulumiEvent::ResourcePlanned {
                                    operation: PulumiOperation::Update,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                };
                            }
                        } else if line.contains("- ") {
                            deletes += 1;
                            if let Some(resource) = parse_resource_line(&line, "-") {
                                yield PulumiEvent::ResourcePlanned {
                                    operation: PulumiOperation::Delete,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                };
                            }
                        } else if line.contains("  ") && !line.trim().is_empty() {
                            // Lines with just spaces (no +/-/~) are typically unchanged resources
                            // Only count if it looks like a resource line
                            if line.contains("::") {
                                unchanged += 1;
                            }
                        }
                    }

                    let status = child.wait().await;
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    if success {
                        yield PulumiEvent::PreviewComplete {
                            creates,
                            updates,
                            deletes,
                            unchanged,
                        };
                    } else {
                        yield PulumiEvent::Error {
                            message: "Pulumi preview failed".into(),
                        };
                    }
                }
                Err(e) => {
                    yield PulumiEvent::Error {
                        message: format!("Failed to spawn pulumi: {}", e),
                    };
                }
            }
        }
    }

    /// Run pulumi up and stream events
    ///
    /// This applies the changes to create/update/delete resources on the forges.
    pub fn up(
        &self,
        org_name: &str,
        repos_file: &PathBuf,
        staged_file: &PathBuf,
        yes: bool,
    ) -> impl Stream<Item = PulumiEvent> + Send + 'static {
        let pulumi_dir = self.pulumi_dir.clone();
        let org_name = org_name.to_string();
        let repos_file = repos_file.clone();
        let staged_file = staged_file.clone();

        stream! {
            yield PulumiEvent::UpStarted {
                org_name: org_name.clone(),
                stack: org_name.clone(),
            };

            // Call forge script with "up" to skip discover.ts (hub manages repos.yaml)
            let mut cmd = Command::new("./forge");
            cmd.current_dir(&pulumi_dir)
                .arg("up") // Just run pulumi up, skip discover.ts
                .env("HYPERFORGE_ORG", &org_name)
                .env("HYPERFORGE_REPOS_FILE", &repos_file)
                .env("HYPERFORGE_STAGED_FILE", &staged_file)
                .env("PULUMI_CONFIG_PASSPHRASE", "") // Empty passphrase for local state
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if yes {
                cmd.arg("--yes");
            }

            match cmd.spawn() {
                Ok(mut child) => {
                    let stdout = child.stdout.take().unwrap();
                    let mut reader = BufReader::new(stdout).lines();

                    let mut creates = 0usize;
                    let mut updates = 0usize;
                    let mut deletes = 0usize;

                    while let Ok(Some(line)) = reader.next_line().await {
                        yield PulumiEvent::Output { line: line.clone() };

                        // Parse for resource operations in `up` output
                        if line.contains("created") {
                            creates += 1;
                            if let Some(resource) = parse_applied_resource_line(&line) {
                                yield PulumiEvent::ResourceApplied {
                                    operation: PulumiOperation::Create,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                    success: true,
                                };
                            }
                        } else if line.contains("updated") {
                            updates += 1;
                            if let Some(resource) = parse_applied_resource_line(&line) {
                                yield PulumiEvent::ResourceApplied {
                                    operation: PulumiOperation::Update,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                    success: true,
                                };
                            }
                        } else if line.contains("deleted") {
                            deletes += 1;
                            if let Some(resource) = parse_applied_resource_line(&line) {
                                yield PulumiEvent::ResourceApplied {
                                    operation: PulumiOperation::Delete,
                                    resource_type: resource.0,
                                    resource_name: resource.1,
                                    success: true,
                                };
                            }
                        }
                    }

                    let status = child.wait().await;
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    yield PulumiEvent::UpComplete {
                        success,
                        creates,
                        updates,
                        deletes,
                    };
                }
                Err(e) => {
                    yield PulumiEvent::Error {
                        message: format!("Failed to spawn pulumi: {}", e),
                    };
                }
            }
        }
    }

    /// Get stack outputs after apply
    ///
    /// Runs `pulumi stack output --json` to retrieve repository URLs and IDs
    /// that were created during the apply operation.
    pub async fn get_outputs(&self, org_name: &str) -> Result<PulumiOutputs, String> {
        let output = Command::new("pulumi")
            .current_dir(&self.pulumi_dir)
            .args(["stack", "output", "--json"])
            .env("PULUMI_STACK", org_name)
            .env("PULUMI_CONFIG_PASSPHRASE", "") // Empty passphrase for local state
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| e.to_string())?;

        // Parse outputs into structured format
        // Actual format from forge-pulumi:
        // {
        //   "repositories": {
        //     "substrate": {
        //       "github": "https://github.com/...",
        //       "codeberg": "https://codeberg.org/..."
        //     }
        //   }
        // }

        let mut repos = HashMap::new();

        if let Some(repos_obj) = json.get("repositories").and_then(|v| v.as_object()) {
            for (name, data) in repos_obj {
                let repo_output = RepoOutput {
                    // URLs are direct strings in the output
                    github_url: data.get("github")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    github_id: None, // IDs not in current output format
                    codeberg_url: data.get("codeberg")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    codeberg_id: None, // IDs not in current output format
                };
                repos.insert(name.clone(), repo_output);
            }
        }

        Ok(PulumiOutputs { repos })
    }

    /// Select or create a Pulumi stack for an organization
    ///
    /// Each organization gets its own Pulumi stack for isolated state management.
    pub async fn select_stack(&self, org_name: &str) -> Result<(), String> {
        let output = Command::new("pulumi")
            .current_dir(&self.pulumi_dir)
            .args(["stack", "select", org_name])
            .env("PULUMI_CONFIG_PASSPHRASE", "") // Empty passphrase for local state
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(())
        } else {
            // Try to create stack if it doesn't exist
            let create_output = Command::new("pulumi")
                .current_dir(&self.pulumi_dir)
                .args(["stack", "init", org_name])
                .env("PULUMI_CONFIG_PASSPHRASE", "") // Empty passphrase for local state
                .output()
                .await
                .map_err(|e| e.to_string())?;

            if create_output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&create_output.stderr).to_string())
            }
        }
    }
}

/// Parse a resource line from Pulumi preview output
///
/// Lines look like: "+ github:index/repository:Repository substrate"
fn parse_resource_line(line: &str, _op: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 3 {
        let resource_type = parts.get(1)?.to_string();
        let resource_name = parts.get(2)?.to_string();
        Some((resource_type, resource_name))
    } else {
        None
    }
}

/// Parse a resource line from Pulumi up output
///
/// Lines look like: "github:index/repository:Repository (substrate) created"
fn parse_applied_resource_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        let resource_type = parts.first()?.to_string();
        // Extract name from parentheses if present
        let resource_name = parts
            .iter()
            .find(|p| p.starts_with('(') && p.ends_with(')'))
            .map(|p| p.trim_matches(|c| c == '(' || c == ')').to_string())
            .unwrap_or_else(|| parts.get(1).unwrap_or(&"").to_string());
        Some((resource_type, resource_name))
    } else {
        None
    }
}
