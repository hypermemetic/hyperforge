//! Publish plan builder with transitive dependency closure.
//!
//! Given a dependency graph and target packages, computes which packages
//! need publishing in topological order, including all transitive
//! local dependencies.

use super::dep_graph::DepGraph;
use super::version::{compare_versions, SemVer};
use crate::package;
use crate::types::VersionBump;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Action to take for a package during publish
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishAction {
    /// Local > published, publish as-is
    Publish,
    /// Local == published, bump patch then publish
    AutoBump,
    /// Never published before, publish at current version
    InitialPublish,
    /// Already up to date, skip
    Skip,
    /// Error determining action
    Error(String),
}

/// A single step in a publish plan
#[derive(Debug, Clone)]
pub struct PublishStep {
    pub name: String,
    pub build_system: super::BuildSystemKind,
    pub path: PathBuf,
    pub local_version: String,
    pub published_version: Option<String>,
    pub action: PublishAction,
    /// Version that will be published (after any auto-bump)
    pub target_version: String,
    pub node_idx: usize,
}

/// Complete publish plan
#[derive(Debug, Clone)]
pub struct PublishPlan {
    /// Steps in topological order (dependencies first)
    pub steps: Vec<PublishStep>,
    /// Packages excluded from publishing (name, reason)
    pub excluded: Vec<(String, String)>,
}

/// Compute the transitive closure of dependencies for the given target nodes.
///
/// Walks `direct_deps()` recursively from each target, collecting all
/// reachable node indices. Returns indices in topological order.
pub fn transitive_closure(graph: &DepGraph, targets: &[usize]) -> Vec<usize> {
    let mut visited = HashSet::new();
    let mut stack: Vec<usize> = targets.to_vec();

    while let Some(idx) = stack.pop() {
        if visited.insert(idx) {
            for dep_idx in graph.direct_deps(idx) {
                if !visited.contains(&dep_idx) {
                    stack.push(dep_idx);
                }
            }
        }
    }

    // Filter topo_order to closure set for correct ordering.
    // If cycle detected, still return the full closure — use discovery order
    // (deps-first since we DFS from targets) which is a best-effort ordering.
    match graph.topo_order() {
        Ok(order) => order.into_iter().filter(|idx| visited.contains(idx)).collect(),
        Err(_) => {
            let mut result: Vec<usize> = visited.into_iter().collect();
            result.sort(); // deterministic fallback
            result
        }
    }
}

/// Build a publish plan for the given targets.
///
/// Computes the transitive closure of all target packages, queries registries
/// for published versions, and determines the action for each package.
pub async fn build_publish_plan(
    graph: &DepGraph,
    targets: &[usize],
    workspace_root: &Path,
    auto_bump_kind: &VersionBump,
) -> anyhow::Result<PublishPlan> {
    let closure = transitive_closure(graph, targets);

    let mut steps = Vec::new();
    let mut excluded = Vec::new();

    for &idx in &closure {
        let node = &graph.nodes[idx];

        // Get build system kind from string
        let build_system = match node.build_system.as_str() {
            "cargo" => super::BuildSystemKind::Cargo,
            "cabal" => super::BuildSystemKind::Cabal,
            "node" => super::BuildSystemKind::Node,
            _ => super::BuildSystemKind::Unknown,
        };

        // Get registry client
        let registry = match package::registry_for(&build_system) {
            Some(r) => r,
            None => {
                excluded.push((
                    node.name.clone(),
                    format!("no registry for build system '{}'", node.build_system),
                ));
                continue;
            }
        };

        let local_version = match &node.version {
            Some(v) => v.clone(),
            None => {
                excluded.push((node.name.clone(), "no version in manifest".to_string()));
                continue;
            }
        };

        let pkg_path = workspace_root.join(&node.path);

        // Query registry for published version
        let published = match registry.published_version(&node.name).await {
            Ok(pv) => pv,
            Err(e) => {
                steps.push(PublishStep {
                    name: node.name.clone(),
                    build_system: build_system.clone(),
                    path: pkg_path,
                    local_version: local_version.clone(),
                    published_version: None,
                    action: PublishAction::Error(format!("registry query failed: {}", e)),
                    target_version: local_version,
                    node_idx: idx,
                });
                continue;
            }
        };

        let published_version = published.as_ref().map(|p| p.version.clone());

        let (action, target_version) = determine_action(&local_version, &published_version, auto_bump_kind);

        steps.push(PublishStep {
            name: node.name.clone(),
            build_system,
            path: pkg_path,
            local_version,
            published_version,
            action,
            target_version,
            node_idx: idx,
        });
    }

    Ok(PublishPlan { steps, excluded })
}

/// Determine the publish action based on local vs published version.
fn determine_action(
    local_version: &str,
    published_version: &Option<String>,
    auto_bump_kind: &VersionBump,
) -> (PublishAction, String) {
    match published_version {
        None => {
            // Never published — use local version as-is
            (PublishAction::InitialPublish, local_version.to_string())
        }
        Some(published) => {
            match compare_versions(local_version, published) {
                Some(Ordering::Greater) => {
                    // Already bumped manually
                    (PublishAction::Publish, local_version.to_string())
                }
                Some(Ordering::Equal) => {
                    // Same version — auto-bump
                    let bumped = SemVer::parse(local_version)
                        .map(|v| v.bump(auto_bump_kind).to_string())
                        .unwrap_or_else(|| local_version.to_string());
                    (PublishAction::AutoBump, bumped)
                }
                Some(Ordering::Less) => {
                    // Local behind published — error
                    (
                        PublishAction::Error(format!(
                            "local version {} < published {}",
                            local_version, published
                        )),
                        local_version.to_string(),
                    )
                }
                None => {
                    // Parse failure
                    (
                        PublishAction::Error(format!(
                            "cannot compare versions: local={}, published={}",
                            local_version, published
                        )),
                        local_version.to_string(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_system::dep_graph::{DepGraph, DepNode};
    use crate::build_system::DepRef;

    fn make_test_graph() -> DepGraph {
        // A -> B -> C (A depends on B, B depends on C)
        let nodes = vec![
            DepNode {
                name: "a".to_string(),
                version: Some("0.2.0".to_string()),
                build_system: "cargo".to_string(),
                path: "a".to_string(),
            },
            DepNode {
                name: "b".to_string(),
                version: Some("0.1.0".to_string()),
                build_system: "cargo".to_string(),
                path: "b".to_string(),
            },
            DepNode {
                name: "c".to_string(),
                version: Some("1.0.0".to_string()),
                build_system: "cargo".to_string(),
                path: "c".to_string(),
            },
        ];

        let deps = vec![
            (
                0,
                vec![DepRef {
                    name: "b".to_string(),
                    version_req: Some("0.1".to_string()),
                    is_path_dep: true,
                    path: Some("../b".to_string()),
                    is_dev: false,
                }],
            ),
            (
                1,
                vec![DepRef {
                    name: "c".to_string(),
                    version_req: Some("1.0".to_string()),
                    is_path_dep: true,
                    path: Some("../c".to_string()),
                    is_dev: false,
                }],
            ),
        ];

        DepGraph::build(nodes, &deps)
    }

    #[test]
    fn test_transitive_closure_includes_all_deps() {
        let graph = make_test_graph();
        // Target is "a" (idx 0), should include b (1) and c (2)
        let closure = transitive_closure(&graph, &[0]);
        assert_eq!(closure.len(), 3);
        // Should be in topo order: c, b, a
        assert_eq!(closure[0], 2); // c first (no deps)
        assert_eq!(closure[1], 1); // b next (depends on c)
        assert_eq!(closure[2], 0); // a last (depends on b)
    }

    #[test]
    fn test_transitive_closure_single_node() {
        let graph = make_test_graph();
        // Target is "c" (idx 2), has no deps
        let closure = transitive_closure(&graph, &[2]);
        assert_eq!(closure.len(), 1);
        assert_eq!(closure[0], 2);
    }

    #[test]
    fn test_determine_action_initial_publish() {
        let (action, version) = determine_action("0.1.0", &None, &VersionBump::Patch);
        assert_eq!(action, PublishAction::InitialPublish);
        assert_eq!(version, "0.1.0");
    }

    #[test]
    fn test_determine_action_publish() {
        let (action, version) = determine_action("0.2.0", &Some("0.1.0".to_string()), &VersionBump::Patch);
        assert_eq!(action, PublishAction::Publish);
        assert_eq!(version, "0.2.0");
    }

    #[test]
    fn test_determine_action_auto_bump() {
        let (action, version) = determine_action("0.3.0", &Some("0.3.0".to_string()), &VersionBump::Patch);
        assert_eq!(action, PublishAction::AutoBump);
        assert_eq!(version, "0.3.1"); // auto-bumped patch
    }

    #[test]
    fn test_determine_action_behind() {
        let (action, _) = determine_action("0.1.0", &Some("0.2.0".to_string()), &VersionBump::Patch);
        assert!(matches!(action, PublishAction::Error(_)));
    }
}
