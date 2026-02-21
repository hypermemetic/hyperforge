//! Workspace dependency graph builder (UNIFY-7)
//!
//! Cross-language dependency graph from enriched workspace context.
//! Supports topological ordering, build tiers, reverse deps, and cycle detection.

use std::collections::HashMap;

use super::DepRef;

/// A node in the dependency graph
#[derive(Debug, Clone)]
pub struct DepNode {
    /// Package name
    pub name: String,
    /// Package version
    pub version: Option<String>,
    /// Build system that owns this package
    pub build_system: String,
    /// Relative path within workspace
    pub path: String,
}

/// An edge in the dependency graph
#[derive(Debug, Clone)]
pub struct DepEdge {
    /// Index of the dependent (source)
    pub from: usize,
    /// Index of the dependency (target)
    pub to: usize,
    /// Version requirement string
    pub version_req: Option<String>,
    /// Whether this is a path dependency
    pub is_path_dep: bool,
}

/// A version mismatch between pinned and local versions
#[derive(Debug, Clone)]
pub struct VersionMismatch {
    pub repo_name: String,
    pub dependency: String,
    pub pinned_version: String,
    pub local_version: String,
}

/// Error for cycle detection
#[derive(Debug, Clone)]
pub struct CycleError {
    pub cycle: Vec<String>,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Dependency cycle: {}", self.cycle.join(" -> "))
    }
}

/// Cross-language workspace dependency graph
#[derive(Debug, Clone)]
pub struct DepGraph {
    pub nodes: Vec<DepNode>,
    pub edges: Vec<DepEdge>,
    name_to_idx: HashMap<String, usize>,
}

impl DepGraph {
    /// Build a dependency graph from workspace nodes and their dependencies.
    pub fn build(
        nodes: Vec<DepNode>,
        all_deps: &[(usize, Vec<DepRef>)],
    ) -> Self {
        let name_to_idx: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i))
            .collect();

        let mut edges = Vec::new();

        for (from_idx, deps) in all_deps {
            for dep in deps {
                if let Some(&to_idx) = name_to_idx.get(&dep.name) {
                    edges.push(DepEdge {
                        from: *from_idx,
                        to: to_idx,
                        version_req: dep.version_req.clone(),
                        is_path_dep: dep.is_path_dep,
                    });
                }
            }
        }

        Self {
            nodes,
            edges,
            name_to_idx,
        }
    }

    /// Topological sort of nodes (dependencies first).
    /// Returns error if a cycle is detected.
    pub fn topo_order(&self) -> Result<Vec<usize>, CycleError> {
        let n = self.nodes.len();
        let mut in_degree = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

        for edge in &self.edges {
            adj[edge.to].push(edge.from);
            in_degree[edge.from] += 1;
        }

        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);

        while let Some(node) = queue.pop() {
            order.push(node);
            for &dependent in &adj[node] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push(dependent);
                }
            }
        }

        if order.len() != n {
            // Find a cycle for error reporting
            let in_cycle: Vec<String> = (0..n)
                .filter(|&i| in_degree[i] > 0)
                .map(|i| self.nodes[i].name.clone())
                .collect();
            return Err(CycleError { cycle: in_cycle });
        }

        Ok(order)
    }

    /// Compute build tiers (groups of packages that can build in parallel).
    /// Tier 0 has no dependencies, tier 1 depends only on tier 0, etc.
    pub fn build_tiers(&self) -> Result<Vec<Vec<usize>>, CycleError> {
        let order = self.topo_order()?;
        let n = self.nodes.len();

        // For each node, compute the longest path from a root
        let mut depth = vec![0usize; n];
        let mut adj_forward: Vec<Vec<usize>> = vec![Vec::new(); n];
        for edge in &self.edges {
            adj_forward[edge.to].push(edge.from);
        }

        // Process in topo order (dependencies first)
        for &node in &order {
            for &dependent in &adj_forward[node] {
                depth[dependent] = depth[dependent].max(depth[node] + 1);
            }
        }

        let max_depth = depth.iter().copied().max().unwrap_or(0);
        let mut tiers = vec![Vec::new(); max_depth + 1];
        for (i, &d) in depth.iter().enumerate() {
            tiers[d].push(i);
        }

        // Sort within tiers for determinism
        for tier in &mut tiers {
            tier.sort_by(|a, b| self.nodes[*a].name.cmp(&self.nodes[*b].name));
        }

        Ok(tiers)
    }

    /// Get reverse dependencies (packages that depend on the given node).
    pub fn reverse_deps(&self, node_idx: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|e| e.to == node_idx)
            .map(|e| e.from)
            .collect()
    }

    /// Get direct dependencies of a node.
    pub fn direct_deps(&self, node_idx: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|e| e.from == node_idx)
            .map(|e| e.to)
            .collect()
    }

    /// Detect version mismatches between pinned versions and local versions.
    pub fn version_mismatches(&self) -> Vec<VersionMismatch> {
        let mut mismatches = Vec::new();

        for edge in &self.edges {
            if let (Some(version_req), Some(local_version)) =
                (&edge.version_req, &self.nodes[edge.to].version)
            {
                if !edge.is_path_dep && !versions_compatible(version_req, local_version) {
                    mismatches.push(VersionMismatch {
                        repo_name: self.nodes[edge.from].name.clone(),
                        dependency: self.nodes[edge.to].name.clone(),
                        pinned_version: version_req.clone(),
                        local_version: local_version.clone(),
                    });
                }
            }
        }

        mismatches.sort_by(|a, b| {
            a.repo_name
                .cmp(&b.repo_name)
                .then(a.dependency.cmp(&b.dependency))
        });
        mismatches
    }

    /// Look up node index by name.
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.name_to_idx.get(name).copied()
    }
}

/// Simple version compatibility check.
/// Returns true if the requirement could plausibly match the local version.
/// This is a heuristic — it checks if the major version matches for semver.
fn versions_compatible(req: &str, local: &str) -> bool {
    // Strip leading operators
    let req_clean = req
        .trim_start_matches('^')
        .trim_start_matches('~')
        .trim_start_matches(">=")
        .trim_start_matches("<=")
        .trim_start_matches('>')
        .trim_start_matches('<')
        .trim_start_matches('=')
        .trim();

    // Parse major versions
    let req_major = req_clean.split('.').next().and_then(|s| s.parse::<u64>().ok());
    let local_major = local.split('.').next().and_then(|s| s.parse::<u64>().ok());

    match (req_major, local_major) {
        (Some(r), Some(l)) => {
            // For 0.x versions, check minor too
            if r == 0 && l == 0 {
                let req_minor = req_clean
                    .split('.')
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok());
                let local_minor = local.split('.').nth(1).and_then(|s| s.parse::<u64>().ok());
                match (req_minor, local_minor) {
                    (Some(rm), Some(lm)) => rm == lm,
                    _ => true,
                }
            } else {
                r == l
            }
        }
        _ => true, // Can't parse, assume compatible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_graph() -> DepGraph {
        let nodes = vec![
            DepNode {
                name: "core".to_string(),
                version: Some("0.2.1".to_string()),
                build_system: "cargo".to_string(),
                path: "plexus-core".to_string(),
            },
            DepNode {
                name: "macros".to_string(),
                version: Some("0.2.2".to_string()),
                build_system: "cargo".to_string(),
                path: "plexus-macros".to_string(),
            },
            DepNode {
                name: "hyperforge".to_string(),
                version: Some("3.3.0".to_string()),
                build_system: "cargo".to_string(),
                path: "hyperforge".to_string(),
            },
        ];

        let deps = vec![
            (1, vec![DepRef {
                name: "core".to_string(),
                version_req: Some("0.2.1".to_string()),
                is_path_dep: false,
                path: None,
                is_dev: false,
            }]),
            (2, vec![
                DepRef {
                    name: "core".to_string(),
                    version_req: Some("0.2.1".to_string()),
                    is_path_dep: false,
                    path: None,
                    is_dev: false,
                },
                DepRef {
                    name: "macros".to_string(),
                    version_req: Some("0.2.2".to_string()),
                    is_path_dep: false,
                    path: None,
                    is_dev: false,
                },
            ]),
        ];

        DepGraph::build(nodes, &deps)
    }

    #[test]
    fn test_topo_order() {
        let graph = make_test_graph();
        let order = graph.topo_order().unwrap();

        // core (0) must come before macros (1), macros before hyperforge (2)
        let core_pos = order.iter().position(|&x| x == 0).unwrap();
        let macros_pos = order.iter().position(|&x| x == 1).unwrap();
        let hf_pos = order.iter().position(|&x| x == 2).unwrap();

        assert!(core_pos < macros_pos);
        assert!(macros_pos < hf_pos);
    }

    #[test]
    fn test_build_tiers() {
        let graph = make_test_graph();
        let tiers = graph.build_tiers().unwrap();

        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0], vec![0]); // core
        assert_eq!(tiers[1], vec![1]); // macros
        assert_eq!(tiers[2], vec![2]); // hyperforge
    }

    #[test]
    fn test_reverse_deps() {
        let graph = make_test_graph();
        let rdeps = graph.reverse_deps(0); // who depends on core?
        assert!(rdeps.contains(&1)); // macros
        assert!(rdeps.contains(&2)); // hyperforge
    }

    #[test]
    fn test_version_mismatch() {
        let nodes = vec![
            DepNode {
                name: "core".to_string(),
                version: Some("3.0.0".to_string()),
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
            version_req: Some("2.0.0".to_string()),
            is_path_dep: false,
            path: None,
            is_dev: false,
        }])];

        let graph = DepGraph::build(nodes, &deps);
        let mismatches = graph.version_mismatches();
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].dependency, "core");
        assert_eq!(mismatches[0].pinned_version, "2.0.0");
        assert_eq!(mismatches[0].local_version, "3.0.0");
    }

    #[test]
    fn test_cycle_detection() {
        let nodes = vec![
            DepNode {
                name: "a".to_string(),
                version: None,
                build_system: "cargo".to_string(),
                path: "a".to_string(),
            },
            DepNode {
                name: "b".to_string(),
                version: None,
                build_system: "cargo".to_string(),
                path: "b".to_string(),
            },
        ];

        let deps = vec![
            (0, vec![DepRef {
                name: "b".to_string(),
                version_req: None,
                is_path_dep: false,
                path: None,
                is_dev: false,
            }]),
            (1, vec![DepRef {
                name: "a".to_string(),
                version_req: None,
                is_path_dep: false,
                path: None,
                is_dev: false,
            }]),
        ];

        let graph = DepGraph::build(nodes, &deps);
        assert!(graph.topo_order().is_err());
    }

    #[test]
    fn test_versions_compatible() {
        assert!(versions_compatible("1.0", "1.5.0"));
        assert!(!versions_compatible("2.0.0", "3.0.0"));
        assert!(versions_compatible("^0.2.1", "0.2.3"));
        assert!(!versions_compatible("0.1.0", "0.2.0"));
    }
}
