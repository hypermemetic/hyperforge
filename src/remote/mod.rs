//! Remote forge operations
//!
//! Import repositories from GitHub, Codeberg, GitLab

use std::sync::Arc;
use std::path::Path;
use anyhow::{Context, Result};

use crate::adapters::{ForgePort, GitHubAdapter, CodebergAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::types::{Forge, Repo};
use crate::git::Git;

/// Get forge adapter for a given forge type
fn get_forge_adapter(forge: &Forge, org: &str) -> Result<Arc<dyn ForgePort>> {
    let auth = Arc::new(YamlAuthProvider::new()?);

    let adapter: Arc<dyn ForgePort> = match forge {
        Forge::GitHub => Arc::new(GitHubAdapter::new(auth)?),
        Forge::Codeberg => Arc::new(CodebergAdapter::new(auth)?),
        Forge::GitLab => Arc::new(GitLabAdapter::new(auth)?),
    };

    Ok(adapter)
}

/// List repositories for an org on a forge
pub async fn list_repos(forge: &Forge, org: &str) -> Result<Vec<Repo>> {
    let adapter = get_forge_adapter(forge, org)?;
    adapter.list_repos(org).await
        .context(format!("Failed to list repos for {} on {:?}", org, forge))
}

/// Import repositories from a forge
///
/// Clones all repositories from the specified forge/org into target_dir
pub async fn import_repos(forge: &Forge, org: &str, target_dir: &str) -> Result<()> {
    let repos = list_repos(forge, org).await?;

    if repos.is_empty() {
        println!("No repositories found for {} on {:?}", org, forge);
        return Ok(());
    }

    println!("Found {} repositories to import", repos.len());

    let target_path = Path::new(target_dir);
    tokio::fs::create_dir_all(target_path).await
        .context("Failed to create target directory")?;

    for repo in repos {
        let repo_path = target_path.join(&repo.name);

        if repo_path.exists() {
            println!("  {} - skipping (already exists)", repo.name);
            continue;
        }

        let clone_url = format_clone_url(forge, org, &repo.name);
        println!("  {} - cloning from {}", repo.name, clone_url);

        Git::clone(&clone_url, repo_path.to_str().unwrap())
            .context(format!("Failed to clone {}", repo.name))?;
    }

    println!("Import complete!");
    Ok(())
}

/// Format clone URL for a repository
fn format_clone_url(forge: &Forge, org: &str, repo_name: &str) -> String {
    match forge {
        Forge::GitHub => format!("https://github.com/{}/{}.git", org, repo_name),
        Forge::Codeberg => format!("https://codeberg.org/{}/{}.git", org, repo_name),
        Forge::GitLab => format!("https://gitlab.com/{}/{}.git", org, repo_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_clone_url_github() {
        let url = format_clone_url(&Forge::GitHub, "myorg", "myrepo");
        assert_eq!(url, "https://github.com/myorg/myrepo.git");
    }

    #[test]
    fn test_format_clone_url_codeberg() {
        let url = format_clone_url(&Forge::Codeberg, "myorg", "myrepo");
        assert_eq!(url, "https://codeberg.org/myorg/myrepo.git");
    }

    #[test]
    fn test_format_clone_url_gitlab() {
        let url = format_clone_url(&Forge::GitLab, "myorg", "myrepo");
        assert_eq!(url, "https://gitlab.com/myorg/myrepo.git");
    }
}
