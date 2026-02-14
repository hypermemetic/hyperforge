//! Workspace discovery - scan filesystem to find repos and derive context
//!
//! Scans immediate children of a workspace directory to find git repos
//! with hyperforge configuration, building a WorkspaceContext that
//! aggregates orgs and forges across all discovered repos.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::build_system::{self, BuildSystemKind, DepRef};
use crate::config::HyperforgeConfig;
use crate::git::Git;
use crate::types::Repo;

/// Errors during workspace discovery
#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Workspace path does not exist: {path}")]
    PathNotFound { path: PathBuf },

    #[error("Workspace path is not a directory: {path}")]
    NotADirectory { path: PathBuf },

    #[error("Failed to read workspace directory: {0}")]
    IoError(#[from] std::io::Error),
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

/// A discovered repository within a workspace
#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    /// Absolute path to the repo directory
    pub path: PathBuf,
    /// Directory name (last component of path)
    pub dir_name: String,
    /// Parsed hyperforge config, if .hyperforge/config.toml exists
    pub config: Option<HyperforgeConfig>,
    /// Whether the directory contains .git
    pub is_git_repo: bool,
    /// Whether the directory has .hyperforge/config.toml
    pub is_hyperforge_repo: bool,
    /// Detected build system (Cargo, Cabal, Node, Unknown)
    pub build_system: BuildSystemKind,
    /// All detected build systems (a repo may have multiple)
    pub build_systems: Vec<BuildSystemKind>,
    /// Dependencies parsed from the build manifest
    pub dependencies: Vec<DepRef>,
    /// Package name from the build manifest
    pub package_name: Option<String>,
    /// Package version from the build manifest
    pub package_version: Option<String>,
}

impl DiscoveredRepo {
    /// Get the org from config, if available
    pub fn org(&self) -> Option<&str> {
        self.config.as_ref().and_then(|c| c.org.as_deref())
    }

    /// Get the forges from config, if available
    pub fn forges(&self) -> Vec<&str> {
        self.config
            .as_ref()
            .map(|c| c.forges.iter().map(|f| f.as_str()).collect())
            .unwrap_or_default()
    }
}

/// Aggregated workspace context from filesystem discovery
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Root workspace directory
    pub root: PathBuf,
    /// All discovered repos with hyperforge config
    pub repos: Vec<DiscoveredRepo>,
    /// Unique orgs derived from configs (sorted)
    pub orgs: Vec<String>,
    /// Unique forges derived from configs (sorted)
    pub forges: Vec<String>,
    /// Directories with .git but no .hyperforge
    pub unconfigured_repos: Vec<PathBuf>,
    /// Directories with neither .git nor .hyperforge
    pub skipped_dirs: Vec<PathBuf>,
}

impl WorkspaceContext {
    /// Get repos filtered by org
    pub fn repos_for_org(&self, org: &str) -> Vec<&DiscoveredRepo> {
        self.repos
            .iter()
            .filter(|r| r.org() == Some(org))
            .collect()
    }

    /// Get repos filtered by org and forge
    pub fn repos_for_org_and_forge(&self, org: &str, forge: &str) -> Vec<&DiscoveredRepo> {
        self.repos
            .iter()
            .filter(|r| {
                r.org() == Some(org)
                    && r.config
                        .as_ref()
                        .map(|c| c.forges.iter().any(|f| f == forge))
                        .unwrap_or(false)
            })
            .collect()
    }

    /// Get all unique build systems across all repos
    pub fn build_systems(&self) -> Vec<BuildSystemKind> {
        let mut systems = BTreeSet::new();
        for repo in &self.repos {
            for bs in &repo.build_systems {
                systems.insert(format!("{}", bs));
            }
        }
        systems
            .into_iter()
            .filter_map(|s| match s.as_str() {
                "cargo" => Some(BuildSystemKind::Cargo),
                "cabal" => Some(BuildSystemKind::Cabal),
                "node" => Some(BuildSystemKind::Node),
                _ => Some(BuildSystemKind::Unknown),
            })
            .collect()
    }

    /// Get repos filtered by build system
    pub fn repos_for_build_system(&self, kind: &BuildSystemKind) -> Vec<&DiscoveredRepo> {
        self.repos
            .iter()
            .filter(|r| r.build_systems.contains(kind))
            .collect()
    }

    /// Get all unique (org, forge) pairs
    pub fn org_forge_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = BTreeSet::new();
        for repo in &self.repos {
            if let Some(config) = &repo.config {
                if let Some(org) = &config.org {
                    for forge in &config.forges {
                        pairs.insert((org.clone(), forge.clone()));
                    }
                }
            }
        }
        pairs.into_iter().collect()
    }
}

/// Build a `Repo` (suitable for LocalForge) from a discovered repo's config.
///
/// First forge → origin, remaining → mirrors.
/// Returns `None` if config is missing, has no org, or has no valid forges.
pub fn repo_from_config(discovered: &DiscoveredRepo) -> Option<Repo> {
    let config = discovered.config.as_ref()?;
    let _org = config.org.as_ref()?;

    // Parse forge strings into Forge enums, keeping only valid ones
    let parsed_forges: Vec<_> = config
        .forges
        .iter()
        .filter_map(|f| HyperforgeConfig::parse_forge(f))
        .collect();

    if parsed_forges.is_empty() {
        return None;
    }

    let origin = parsed_forges[0].clone();
    let mirrors: Vec<_> = parsed_forges[1..].to_vec();

    let repo_name = config.get_repo_name(&discovered.path);

    let mut repo = Repo::new(repo_name, origin)
        .with_visibility(config.visibility.clone())
        .with_mirrors(mirrors);

    if let Some(ref desc) = config.description {
        repo = repo.with_description(desc);
    }

    Some(repo)
}

/// Scan immediate children of workspace_path to discover repos.
///
/// Pure filesystem reads — no git commands, no network.
/// Only scans one level deep (immediate children).
pub fn discover_workspace(workspace_path: &Path) -> WorkspaceResult<WorkspaceContext> {
    let workspace_path = workspace_path
        .canonicalize()
        .map_err(|_| WorkspaceError::PathNotFound {
            path: workspace_path.to_path_buf(),
        })?;

    if !workspace_path.is_dir() {
        return Err(WorkspaceError::NotADirectory {
            path: workspace_path.clone(),
        });
    }

    let mut repos = Vec::new();
    let mut unconfigured_repos = Vec::new();
    let mut skipped_dirs = Vec::new();
    let mut orgs_set = BTreeSet::new();
    let mut forges_set = BTreeSet::new();

    let entries = std::fs::read_dir(&workspace_path)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Only look at directories
        if !path.is_dir() {
            continue;
        }

        // Skip hidden directories
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) if name.starts_with('.') => continue,
            Some(name) => name.to_string(),
            None => continue,
        };

        let is_git_repo = Git::is_repo(&path);
        let is_hyperforge_repo = HyperforgeConfig::exists(&path);

        if !is_git_repo && !is_hyperforge_repo {
            skipped_dirs.push(path);
            continue;
        }

        if is_git_repo && !is_hyperforge_repo {
            unconfigured_repos.push(path);
            continue;
        }

        // Load config for hyperforge repos
        let config = HyperforgeConfig::load(&path).ok();

        if let Some(ref config) = config {
            if let Some(ref org) = config.org {
                orgs_set.insert(org.clone());
            }
            for forge in &config.forges {
                forges_set.insert(forge.clone());
            }
        }

        // Detect build system
        let primary_bs = build_system::detect_build_system(&path);
        let all_bs = build_system::detect_all_build_systems(&path);
        let deps = build_system::parse_dependencies(&path, &primary_bs);
        let pkg_name = build_system::package_name(&path, &primary_bs);
        let pkg_version = build_system::package_version(&path, &primary_bs);

        repos.push(DiscoveredRepo {
            path,
            dir_name,
            config,
            is_git_repo,
            is_hyperforge_repo,
            build_system: primary_bs,
            build_systems: all_bs,
            dependencies: deps,
            package_name: pkg_name,
            package_version: pkg_version,
        });
    }

    // Sort repos by name for deterministic output
    repos.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));

    Ok(WorkspaceContext {
        root: workspace_path,
        repos,
        orgs: orgs_set.into_iter().collect(),
        forges: forges_set.into_iter().collect(),
        unconfigured_repos,
        skipped_dirs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_workspace() -> TempDir {
        let workspace = TempDir::new().unwrap();

        // Create a configured repo
        let repo_a = workspace.path().join("repo-a");
        std::fs::create_dir(&repo_a).unwrap();
        Git::init(&repo_a).unwrap();
        let config = HyperforgeConfig::new(vec!["github".to_string(), "codeberg".to_string()])
            .with_org("alice")
            .with_repo_name("repo-a");
        config.save(&repo_a).unwrap();

        // Create another configured repo with different org
        let repo_b = workspace.path().join("repo-b");
        std::fs::create_dir(&repo_b).unwrap();
        Git::init(&repo_b).unwrap();
        let config = HyperforgeConfig::new(vec!["github".to_string()])
            .with_org("bob")
            .with_repo_name("repo-b");
        config.save(&repo_b).unwrap();

        // Create an unconfigured git repo
        let repo_c = workspace.path().join("repo-c");
        std::fs::create_dir(&repo_c).unwrap();
        Git::init(&repo_c).unwrap();

        // Create a non-repo directory
        let random = workspace.path().join("notes");
        std::fs::create_dir(&random).unwrap();

        // Create a hidden directory (should be skipped)
        let hidden = workspace.path().join(".hidden");
        std::fs::create_dir(&hidden).unwrap();

        workspace
    }

    #[test]
    fn test_discover_workspace() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        assert_eq!(ctx.repos.len(), 2);
        assert_eq!(ctx.unconfigured_repos.len(), 1);
        assert_eq!(ctx.skipped_dirs.len(), 1);
    }

    #[test]
    fn test_discover_orgs_and_forges() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        assert_eq!(ctx.orgs, vec!["alice", "bob"]);
        assert_eq!(ctx.forges, vec!["codeberg", "github"]);
    }

    #[test]
    fn test_discover_org_forge_pairs() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        let pairs = ctx.org_forge_pairs();
        assert_eq!(
            pairs,
            vec![
                ("alice".to_string(), "codeberg".to_string()),
                ("alice".to_string(), "github".to_string()),
                ("bob".to_string(), "github".to_string()),
            ]
        );
    }

    #[test]
    fn test_discover_repos_for_org() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        let alice_repos = ctx.repos_for_org("alice");
        assert_eq!(alice_repos.len(), 1);
        assert_eq!(alice_repos[0].dir_name, "repo-a");
    }

    #[test]
    fn test_discover_nonexistent_path() {
        let result = discover_workspace(Path::new("/nonexistent/path"));
        assert!(matches!(result, Err(WorkspaceError::PathNotFound { .. })));
    }

    #[test]
    fn test_repo_from_config_basic() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        // repo-a has github + codeberg, org=alice
        let repo_a = ctx.repos.iter().find(|r| r.dir_name == "repo-a").unwrap();
        let repo = repo_from_config(repo_a).unwrap();

        assert_eq!(repo.name, "repo-a");
        assert_eq!(repo.origin, crate::types::Forge::GitHub);
        assert_eq!(repo.mirrors, vec![crate::types::Forge::Codeberg]);
    }

    #[test]
    fn test_repo_from_config_single_forge() {
        let workspace = setup_workspace();
        let ctx = discover_workspace(workspace.path()).unwrap();

        // repo-b has github only, org=bob
        let repo_b = ctx.repos.iter().find(|r| r.dir_name == "repo-b").unwrap();
        let repo = repo_from_config(repo_b).unwrap();

        assert_eq!(repo.name, "repo-b");
        assert_eq!(repo.origin, crate::types::Forge::GitHub);
        assert!(repo.mirrors.is_empty());
    }

    #[test]
    fn test_repo_from_config_no_config() {
        let discovered = DiscoveredRepo {
            path: PathBuf::from("/tmp/fake"),
            dir_name: "fake".to_string(),
            config: None,
            is_git_repo: true,
            is_hyperforge_repo: false,
            build_system: BuildSystemKind::Unknown,
            build_systems: vec![],
            dependencies: vec![],
            package_name: None,
            package_version: None,
        };
        assert!(repo_from_config(&discovered).is_none());
    }

    #[test]
    fn test_repo_from_config_no_org() {
        let discovered = DiscoveredRepo {
            path: PathBuf::from("/tmp/fake"),
            dir_name: "fake".to_string(),
            config: Some(HyperforgeConfig::new(vec!["github".to_string()])),
            is_git_repo: true,
            is_hyperforge_repo: true,
            build_system: BuildSystemKind::Unknown,
            build_systems: vec![],
            dependencies: vec![],
            package_name: None,
            package_version: None,
        };
        // No org set → returns None
        assert!(repo_from_config(&discovered).is_none());
    }

    #[test]
    fn test_discover_empty_workspace() {
        let workspace = TempDir::new().unwrap();
        let ctx = discover_workspace(workspace.path()).unwrap();

        assert!(ctx.repos.is_empty());
        assert!(ctx.orgs.is_empty());
        assert!(ctx.forges.is_empty());
    }
}
