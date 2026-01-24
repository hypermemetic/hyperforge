//! Remote forge operations
//!
//! Import repositories from GitHub, Codeberg, GitLab

use crate::types::Forge;

/// List repositories for an org on a forge
pub async fn list_repos(_forge: &Forge, _org: &str) -> anyhow::Result<Vec<String>> {
    todo!("List repos on forge")
}

/// Import repositories from a forge
pub async fn import_repos(_forge: &Forge, _org: &str, _target_dir: &str) -> anyhow::Result<()> {
    todo!("Import repos from forge")
}
