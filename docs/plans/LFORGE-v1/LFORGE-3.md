# LFORGE-3: Simplify Domain Types

**blocked_by:** [LFORGE-2]
**unlocks:** [LFORGE-6]

## Scope

Merge `DesiredRepo` and `ObservedRepo` into a single unified `Repo` type. Simplify `diff.rs` to compute differences between repos from any two forges. Update `SyncAction` for symmetric operations. This enables treating all forges uniformly - the same Repo type works everywhere.

## Deliverables

1. New unified `Repo` type in `src/domain/repo.rs`
2. Simplified `SyncAction` enum for symmetric operations
3. Updated `diff.rs` that works with `Repo` from any source
4. Add `Forge::Local` variant to the `Forge` enum
5. Migration of existing code to use new types
6. Deprecation markers on old types (full removal in LFORGE-9)

## Verification Steps

```bash
# Run all domain tests
cd ~/dev/controlflow/hypermemetic/hyperforge
cargo test domain::

# Verify Repo type is exported
cargo doc --open
# Navigate to domain::Repo

# Check that both old and new types exist (deprecation period)
grep -r "DesiredRepo" src/
grep -r "Repo" src/domain/repo.rs
```

## Implementation Notes

### New Unified Repo Type

Create `src/domain/repo.rs`:

```rust
//! Unified repository type for symmetric forge operations.
//!
//! A Repo represents a repository as it exists (or should exist) in any forge.
//! This unified type replaces the previous DesiredRepo/ObservedRepo split.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::Visibility;
use super::RepoIdentity;

/// A repository as it exists in any forge.
///
/// This unified type works for both local config and remote forges.
/// Previously this was split into DesiredRepo (intent) and ObservedRepo (reality),
/// but with LocalForge treating local as a forge, we unify them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repo {
    /// Unique identifier (org + name)
    pub identity: RepoIdentity,
    /// Human-readable description
    pub description: Option<String>,
    /// Public or private
    pub visibility: Visibility,
    /// Optional homepage URL
    pub homepage: Option<String>,
}

impl Repo {
    /// Create a new repo with required fields
    pub fn new(identity: RepoIdentity, visibility: Visibility) -> Self {
        Self {
            identity,
            description: None,
            visibility,
            homepage: None,
        }
    }

    /// Builder: set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder: set homepage
    pub fn with_homepage(mut self, homepage: impl Into<String>) -> Self {
        self.homepage = Some(homepage.into());
        self
    }

    /// Get repository name
    pub fn name(&self) -> &str {
        &self.identity.name
    }

    /// Get organization name
    pub fn org(&self) -> &str {
        &self.identity.org
    }
}

/// Convert from legacy DesiredRepo
impl From<super::DesiredRepo> for Repo {
    fn from(desired: super::DesiredRepo) -> Self {
        Self {
            identity: desired.identity,
            description: desired.description,
            visibility: desired.visibility,
            homepage: None,
        }
    }
}

/// Convert from legacy ObservedRepo (takes first forge state's properties)
impl From<super::ObservedRepo> for Repo {
    fn from(observed: super::ObservedRepo) -> Self {
        let first_state = observed.forge_states.first();
        Self {
            identity: observed.identity,
            description: first_state.and_then(|s| s.description.clone()),
            visibility: first_state
                .and_then(|s| s.visibility.clone())
                .unwrap_or(Visibility::Private),
            homepage: None,
        }
    }
}
```

### Simplified SyncAction

Update or create in `src/domain/sync_action.rs`:

```rust
//! Symmetric sync actions between any two forges.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Repo, RepoIdentity};

/// What property changed between source and target
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PropertyDiff {
    Visibility {
        source: crate::types::Visibility,
        target: crate::types::Visibility,
    },
    Description {
        source: Option<String>,
        target: Option<String>,
    },
}

/// Action needed to sync source -> target for a single repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SyncAction {
    /// Repo exists in source but not target - create in target
    Create(Repo),

    /// Repo exists in both but properties differ - update target
    Update {
        repo: Repo,
        diffs: Vec<PropertyDiff>,
    },

    /// Repo exists in target but not source - optionally delete from target
    Delete(RepoIdentity),

    /// Repo is identical in both - no action needed
    InSync(RepoIdentity),
}

impl SyncAction {
    /// Check if this action requires changes
    pub fn needs_action(&self) -> bool {
        !matches!(self, SyncAction::InSync(_))
    }

    /// Get the repo identity this action affects
    pub fn identity(&self) -> &RepoIdentity {
        match self {
            SyncAction::Create(repo) => &repo.identity,
            SyncAction::Update { repo, .. } => &repo.identity,
            SyncAction::Delete(id) => id,
            SyncAction::InSync(id) => id,
        }
    }

    pub fn is_create(&self) -> bool {
        matches!(self, SyncAction::Create(_))
    }

    pub fn is_update(&self) -> bool {
        matches!(self, SyncAction::Update { .. })
    }

    pub fn is_delete(&self) -> bool {
        matches!(self, SyncAction::Delete(_))
    }

    pub fn is_in_sync(&self) -> bool {
        matches!(self, SyncAction::InSync(_))
    }
}
```

### Updated Diff Logic

Simplify `src/domain/diff.rs` or create new symmetric diff:

```rust
//! Compute differences between repos in two forges.

use std::collections::HashMap;

use super::{Repo, RepoIdentity, SyncAction, PropertyDiff};

/// Compute sync actions needed to make target match source.
///
/// # Arguments
/// * `source_repos` - Repos from source forge (the "truth")
/// * `target_repos` - Repos from target forge (to be updated)
/// * `delete_missing` - If true, delete repos in target not in source
///
/// # Returns
/// List of actions to apply to target forge
pub fn compute_sync_actions(
    source_repos: &[Repo],
    target_repos: &[Repo],
    delete_missing: bool,
) -> Vec<SyncAction> {
    let mut actions = Vec::new();

    // Index target repos by identity for fast lookup
    let target_map: HashMap<&RepoIdentity, &Repo> = target_repos
        .iter()
        .map(|r| (&r.identity, r))
        .collect();

    // Index source repos by identity
    let source_map: HashMap<&RepoIdentity, &Repo> = source_repos
        .iter()
        .map(|r| (&r.identity, r))
        .collect();

    // Check each source repo
    for source in source_repos {
        match target_map.get(&source.identity) {
            Some(target) => {
                // Exists in both - check for differences
                let diffs = compute_property_diffs(source, target);
                if diffs.is_empty() {
                    actions.push(SyncAction::InSync(source.identity.clone()));
                } else {
                    actions.push(SyncAction::Update {
                        repo: source.clone(),
                        diffs,
                    });
                }
            }
            None => {
                // Only in source - needs create in target
                actions.push(SyncAction::Create(source.clone()));
            }
        }
    }

    // Check for repos only in target (potential deletes)
    if delete_missing {
        for target in target_repos {
            if !source_map.contains_key(&target.identity) {
                actions.push(SyncAction::Delete(target.identity.clone()));
            }
        }
    }

    actions
}

/// Compute property differences between two repos
fn compute_property_diffs(source: &Repo, target: &Repo) -> Vec<PropertyDiff> {
    let mut diffs = Vec::new();

    if source.visibility != target.visibility {
        diffs.push(PropertyDiff::Visibility {
            source: source.visibility.clone(),
            target: target.visibility.clone(),
        });
    }

    if source.description != target.description {
        diffs.push(PropertyDiff::Description {
            source: source.description.clone(),
            target: target.description.clone(),
        });
    }

    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Visibility;

    fn make_repo(org: &str, name: &str) -> Repo {
        Repo::new(RepoIdentity::new(org, name), Visibility::Public)
    }

    #[test]
    fn test_empty_source_and_target() {
        let actions = compute_sync_actions(&[], &[], false);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_create_when_only_in_source() {
        let source = vec![make_repo("org", "repo1")];
        let target = vec![];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_create());
    }

    #[test]
    fn test_in_sync_when_identical() {
        let source = vec![make_repo("org", "repo1")];
        let target = vec![make_repo("org", "repo1")];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_in_sync());
    }

    #[test]
    fn test_update_when_visibility_differs() {
        let source = vec![Repo::new(RepoIdentity::new("org", "repo1"), Visibility::Private)];
        let target = vec![Repo::new(RepoIdentity::new("org", "repo1"), Visibility::Public)];

        let actions = compute_sync_actions(&source, &target, false);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_update());
    }

    #[test]
    fn test_delete_when_only_in_target_and_flag_set() {
        let source = vec![];
        let target = vec![make_repo("org", "orphan")];

        let actions = compute_sync_actions(&source, &target, true);

        assert_eq!(actions.len(), 1);
        assert!(actions[0].is_delete());
    }

    #[test]
    fn test_no_delete_when_flag_not_set() {
        let source = vec![];
        let target = vec![make_repo("org", "orphan")];

        let actions = compute_sync_actions(&source, &target, false);

        assert!(actions.is_empty());
    }
}
```

### Add Forge::Local Variant

Update `src/types/forge.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    Codeberg,
    GitLab,
    Local,  // NEW: represents local config storage
}

impl Forge {
    pub fn ssh_host(&self) -> &str {
        match self {
            Forge::GitHub => "github.com",
            Forge::Codeberg => "codeberg.org",
            Forge::GitLab => "gitlab.com",
            Forge::Local => "local",  // No SSH for local
        }
    }
}
```

### Update mod.rs Exports

Update `src/domain/mod.rs`:

```rust
mod repo;
mod sync_action;
// ... existing modules

pub use repo::Repo;
pub use sync_action::{SyncAction, PropertyDiff};

// Keep old types for backward compatibility during migration
#[deprecated(since = "0.2.0", note = "Use Repo instead")]
pub use desired::DesiredRepo;
#[deprecated(since = "0.2.0", note = "Use Repo instead")]
pub use observed::ObservedRepo;
```

### Key Design Decisions

1. **Single Repo type**: No more desired/observed split - a repo is a repo
2. **PropertyDiff enum**: Explicit about what changed, useful for UI/logging
3. **delete_missing flag**: Explicit control over destructive operations
4. **Forge::Local**: First-class local storage representation
5. **Backward compatibility**: Old types still exist with deprecation warnings
