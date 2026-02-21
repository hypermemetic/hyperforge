//! HyperforgeHub - Root activation for hyperforge
//!
//! This is a hub plugin that routes to child sub-hubs:
//! - repo: Single-repo operations and registry CRUD
//! - workspace: Multi-repo workspace orchestration

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use plexus_core::plexus::{Activation, ChildRouter, ChildSummary, PlexusError, PlexusStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use chrono::Utc;
use std::collections::HashMap;

use crate::adapters::{ForgePort, ForgeSyncState};
use crate::config::HyperforgeConfig;
use crate::hubs::workspace::make_adapter;
use crate::hubs::{HyperforgeState, RepoHub, WorkspaceHub};
use crate::types::repo::RepoRecord;
use crate::types::Forge;

/// Package registry identifier
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageRegistry {
    CratesIo,
    Hackage,
    Npm,
}

impl std::fmt::Display for PackageRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CratesIo => write!(f, "crates.io"),
            Self::Hackage => write!(f, "hackage"),
            Self::Npm => write!(f, "npm"),
        }
    }
}

/// Package status relative to its registry
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageStatus {
    /// Local version > published version
    Ahead,
    /// Local version == published version, no changes needed
    UpToDate,
    /// Never published to registry
    Unpublished,
    /// Local version == published version but needs bump (code changed)
    Stale,
}

/// Action taken or planned during publish
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublishActionKind {
    /// Local > published, publish as-is
    Publish,
    /// Local == published, bump patch then publish
    AutoBump,
    /// First publish ever
    InitialPublish,
    /// Already up to date, skip
    Skip,
    /// Git tag created
    Tag,
    /// Publish failed
    Failed,
}

/// Hyperforge event types
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HyperforgeEvent {
    /// Status information
    Status {
        version: String,
        description: String,
    },
    /// General info message
    Info { message: String },
    /// Error message
    Error { message: String },
    /// Repository information
    Repo {
        name: String,
        description: Option<String>,
        visibility: String,
        origin: String,
        mirrors: Vec<String>,
        protected: bool,
        #[serde(default, skip_serializing_if = "crate::types::repo::is_false")]
        staged_for_deletion: bool,
    },
    /// Sync diff result - repo operation
    SyncOp {
        repo_name: String,
        operation: String, // "create", "update", "delete", "in_sync"
        forge: String,
    },
    /// Sync summary
    SyncSummary {
        forge: String,
        total: usize,
        to_create: usize,
        to_update: usize,
        to_delete: usize,
        in_sync: usize,
    },
    /// Per-repo check result (branch + clean status)
    RepoCheck {
        repo_name: String,
        path: String,
        branch: String,
        expected_branch: String,
        is_clean: bool,
        on_correct_branch: bool,
    },
    /// Per-repo push result
    RepoPush {
        repo_name: String,
        path: String,
        forge: String,
        success: bool,
        error: Option<String>,
    },
    /// Workspace-level summary
    WorkspaceSummary {
        total_repos: usize,
        configured_repos: usize,
        unconfigured_repos: usize,
        clean_repos: Option<usize>,
        dirty_repos: Option<usize>,
        wrong_branch_repos: Option<usize>,
        push_success: Option<usize>,
        push_failed: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        validation_passed: Option<bool>,
    },
    /// Result of workspace unify (native workspace file generation)
    UnifyResult {
        language: String,
        file_path: String,
        action: String, // "created", "updated", "unchanged"
    },
    /// Dependency version mismatch between pinned and local
    DepMismatch {
        repo: String,
        dependency: String,
        pinned_version: String,
        local_version: String,
    },
    /// Validation step result
    ValidateStep {
        repo_name: String,
        step: String, // "build" or "test"
        status: String, // "passed", "failed", "skipped"
        duration_ms: u64,
    },
    /// Validation summary
    ValidateSummary {
        total: usize,
        passed: usize,
        failed: usize,
        skipped: usize,
        duration_ms: u64,
    },
    /// Result of workspace exec command for a single repo
    ExecResult {
        repo_name: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    /// Package registry diff — local vs published version
    PackageDiff {
        package_name: String,
        build_system: crate::build_system::BuildSystemKind,
        local_version: String,
        published_version: Option<String>,
        registry: PackageRegistry,
        status: PackageStatus,
    },
    /// Per-package publish step result
    PublishStep {
        package_name: String,
        version: String,
        registry: PackageRegistry,
        action: PublishActionKind,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Summary of a publish run
    PublishSummary {
        total: usize,
        published: usize,
        auto_bumped: usize,
        skipped: usize,
        failed: usize,
        tags_created: usize,
    },
}

/// Root hub for hyperforge operations
#[derive(Clone)]
pub struct HyperforgeHub {
    pub(crate) state: HyperforgeState,
}

impl HyperforgeHub {
    /// Create a new HyperforgeHub instance
    pub fn new() -> Self {
        Self {
            state: HyperforgeState::new(),
        }
    }
}

impl Default for HyperforgeHub {
    fn default() -> Self {
        Self::new()
    }
}

#[plexus_macros::hub_methods(
    namespace = "hyperforge",
    description = "Multi-forge repository management",
    crate_path = "plexus_core",
    hub
)]
impl HyperforgeHub {
    /// Show hyperforge status
    #[plexus_macros::hub_method(description = "Show hyperforge status and version")]
    pub async fn status(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        stream! {
            yield HyperforgeEvent::Status {
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "Multi-forge repository management (FORGE4: state mirror + SSH safety)".to_string(),
            };
        }
    }

    /// Reload cached state from disk (repos.yaml for all known orgs)
    #[plexus_macros::hub_method(description = "Reload all cached LocalForge state from disk")]
    pub async fn reload(&self) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        stream! {
            let reloaded = state.reload().await;
            if reloaded.is_empty() {
                yield HyperforgeEvent::Info {
                    message: "No cached orgs to reload. Run a command first to populate the cache.".to_string(),
                };
            } else {
                yield HyperforgeEvent::Info {
                    message: format!("Reloaded {} org(s): {}", reloaded.len(), reloaded.join(", ")),
                };
            }
        }
    }

    /// Bootstrap an org — import all repos from remote forges into LocalForge
    #[plexus_macros::hub_method(
        description = "Bootstrap an org — import all repos from remote forges into LocalForge, creating the canonical state mirror",
        params(
            org = "Organization name",
            forges = "Forges to import from (e.g. github,codeberg)",
            dry_run = "Preview without writing state (optional, default: false)"
        )
    )]
    pub async fn begin(
        &self,
        org: String,
        forges: Vec<String>,
        dry_run: Option<bool>,
    ) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
        let state = self.state.clone();
        let is_dry_run = dry_run.unwrap_or(false);

        stream! {
            let dry_prefix = if is_dry_run { "[dry-run] " } else { "" };

            // Parse and validate forge strings
            let mut parsed_forges: Vec<(String, Forge)> = Vec::new();
            for forge_str in &forges {
                for part in forge_str.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    match HyperforgeConfig::parse_forge(part) {
                        Some(forge) => parsed_forges.push((part.to_lowercase().to_string(), forge)),
                        None => {
                            yield HyperforgeEvent::Error {
                                message: format!("Invalid forge: {}. Must be github, codeberg, or gitlab", part),
                            };
                            return;
                        }
                    }
                }
            }

            if parsed_forges.is_empty() {
                yield HyperforgeEvent::Error {
                    message: "No forges specified. Provide at least one forge (github, codeberg, gitlab).".to_string(),
                };
                return;
            }

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Begin: bootstrapping org '{}' from {} forge(s): {}",
                    dry_prefix,
                    org,
                    parsed_forges.len(),
                    parsed_forges.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(", "),
                ),
            };

            // Phase 1: Verify auth by creating adapters
            let mut adapters: Vec<(String, Forge, Arc<dyn ForgePort>)> = Vec::new();

            let ot = state.get_local_forge(&org).await.owner_type();
            for (forge_str, forge_enum) in &parsed_forges {
                match make_adapter(forge_str, &org, ot.clone()) {
                    Ok(adapter) => {
                        yield HyperforgeEvent::Info {
                            message: format!("  Authenticated with {}", forge_str),
                        };
                        adapters.push((forge_str.clone(), forge_enum.clone(), adapter));
                    }
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to authenticate with {}: {}", forge_str, e),
                        };
                        return;
                    }
                }
            }

            // Phase 2: Import repos from each forge
            let local = state.get_local_forge(&org).await;
            let mut per_forge_counts: HashMap<String, usize> = HashMap::new();
            let mut total_upserted = 0usize;

            for (forge_str, forge_enum, adapter) in &adapters {
                yield HyperforgeEvent::Info {
                    message: format!("  {}Importing repos from {}...", dry_prefix, forge_str),
                };

                let list_result = match adapter.list_repos_incremental(&org, None).await {
                    Ok(lr) => lr,
                    Err(e) => {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to list repos from {}: {}", forge_str, e),
                        };
                        continue;
                    }
                };

                let mut forge_count = 0usize;

                if list_result.modified {
                    if let Some(repos) = &list_result.repos {
                        for repo in repos {
                            let record = RepoRecord::from_repo(repo);

                            if !is_dry_run {
                                if let Err(e) = local.upsert_record(record) {
                                    yield HyperforgeEvent::Error {
                                        message: format!("  Failed to upsert {}: {}", repo.name, e),
                                    };
                                    continue;
                                }
                            }

                            forge_count += 1;
                            total_upserted += 1;
                        }

                        yield HyperforgeEvent::Info {
                            message: format!("  {}Found {} repos on {}", dry_prefix, forge_count, forge_str),
                        };
                    }
                } else {
                    yield HyperforgeEvent::Info {
                        message: format!("  {} returned not-modified (no repos to import)", forge_str),
                    };
                }

                per_forge_counts.insert(forge_str.clone(), forge_count);

                // Store ETags
                if !is_dry_run {
                    if let Err(e) = local.set_forge_state(forge_enum.clone(), ForgeSyncState {
                        last_synced: Utc::now(),
                        etag: list_result.etag.clone(),
                    }) {
                        yield HyperforgeEvent::Error {
                            message: format!("  Failed to store sync state for {}: {}", forge_str, e),
                        };
                    }
                }
            }

            // Save LocalForge to disk
            if !is_dry_run && total_upserted > 0 {
                if let Err(e) = local.save_to_yaml().await {
                    yield HyperforgeEvent::Error {
                        message: format!("Failed to save LocalForge for {}: {}", org, e),
                    };
                }
            }

            // Summary
            let unique_count = match local.all_records() {
                Ok(records) => records.len(),
                Err(_) => total_upserted,
            };

            yield HyperforgeEvent::Info {
                message: format!(
                    "{}Begin complete: {} unique repos in LocalForge for '{}'",
                    dry_prefix, unique_count, org,
                ),
            };

            for (forge_str, count) in &per_forge_counts {
                yield HyperforgeEvent::Info {
                    message: format!("  {}: {} repos imported", forge_str, count),
                };
            }

            if is_dry_run {
                yield HyperforgeEvent::Info {
                    message: "Dry run — no changes written to disk.".to_string(),
                };
            }
        }
    }

    /// Get child plugin summaries for the hub schema
    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        let repo = RepoHub::new(self.state.clone());
        let workspace = WorkspaceHub::new(self.state.clone());

        vec![
            child_summary(&repo),
            child_summary(&workspace),
        ]
    }
}

/// Extract a ChildSummary from any Activation
fn child_summary<T: Activation>(activation: &T) -> ChildSummary {
    let schema = activation.plugin_schema();
    ChildSummary {
        namespace: schema.namespace,
        description: schema.description,
        hash: schema.hash,
    }
}

/// ChildRouter implementation for nested method routing
#[async_trait]
impl ChildRouter for HyperforgeHub {
    fn router_namespace(&self) -> &str {
        "hyperforge"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "repo" => Some(Box::new(RepoHub::new(self.state.clone()))),
            "workspace" => Some(Box::new(WorkspaceHub::new(self.state.clone()))),
            _ => None,
        }
    }
}
