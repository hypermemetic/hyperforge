//! Shared helpers used by both WorkspaceHub and BuildHub.

use std::sync::Arc;

use crate::adapters::{CodebergAdapter, ForgePort, GitHubAdapter, GitLabAdapter};
use crate::auth::YamlAuthProvider;
use crate::config::HyperforgeConfig;
use crate::hub::HyperforgeEvent;
use crate::types::{Forge, OwnerType};

/// Create a forge adapter from a forge name string.
pub(crate) fn make_adapter(
    forge: &str,
    org: &str,
    owner_type: Option<OwnerType>,
) -> Result<Arc<dyn ForgePort>, String> {
    let auth = YamlAuthProvider::new()
        .map_err(|e| format!("Failed to create auth provider: {}", e))?;
    let auth = Arc::new(auth);
    let target_forge = HyperforgeConfig::parse_forge(forge)
        .ok_or_else(|| format!("Invalid forge: {}. Must be github, codeberg, or gitlab", forge))?;
    let adapter: Arc<dyn ForgePort> = match target_forge {
        Forge::GitHub => {
            let a = GitHubAdapter::new(auth, org)
                .map_err(|e| format!("Failed to create GitHub adapter: {}", e))?;
            Arc::new(match owner_type {
                Some(ot) => a.with_owner_type(ot),
                None => a,
            })
        }
        Forge::Codeberg => {
            let a = CodebergAdapter::new(auth, org)
                .map_err(|e| format!("Failed to create Codeberg adapter: {}", e))?;
            Arc::new(match owner_type {
                Some(ot) => a.with_owner_type(ot),
                None => a,
            })
        }
        Forge::GitLab => {
            let a = GitLabAdapter::new(auth, org)
                .map_err(|e| format!("Failed to create GitLab adapter: {}", e))?;
            Arc::new(match owner_type {
                Some(ot) => a.with_owner_type(ot),
                None => a,
            })
        }
    };
    Ok(adapter)
}

/// Simple glob matching for repo name filtering.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if pattern.contains('*') {
        // Simple prefix*suffix matching
        let parts: Vec<&str> = pattern.splitn(2, '*').collect();
        if parts.len() == 2 {
            return name.starts_with(parts[0]) && name.ends_with(parts[1]);
        }
    }
    name == pattern
}

/// Build a default WorkspaceSummary event with all optional fields set to None.
pub(crate) fn workspace_summary(
    ctx: &crate::commands::workspace::WorkspaceContext,
) -> HyperforgeEvent {
    HyperforgeEvent::WorkspaceSummary {
        total_repos: ctx.repos.len() + ctx.unconfigured_repos.len(),
        configured_repos: ctx.repos.len(),
        unconfigured_repos: ctx.unconfigured_repos.len(),
        clean_repos: None,
        dirty_repos: None,
        wrong_branch_repos: None,
        push_success: None,
        push_failed: None,
        validation_passed: None,
    }
}

/// Return a prefix string for dry-run messages.
pub fn dry_prefix(is_dry_run: bool) -> &'static str {
    if is_dry_run {
        "[DRY RUN] "
    } else {
        ""
    }
}
