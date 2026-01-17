//! Symmetric sync actions between any two forges.
//!
//! This module defines the actions needed to synchronize repositories
//! between a source forge and a target forge. The same actions work
//! regardless of which forges are involved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Repo, RepoIdentity};

/// What property changed between source and target
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PropertyDiff {
    /// Visibility changed
    Visibility {
        source: crate::types::Visibility,
        target: crate::types::Visibility,
    },
    /// Description changed
    Description {
        source: Option<String>,
        target: Option<String>,
    },
}

impl PropertyDiff {
    /// Get a human-readable name for this diff
    pub fn name(&self) -> &'static str {
        match self {
            PropertyDiff::Visibility { .. } => "visibility",
            PropertyDiff::Description { .. } => "description",
        }
    }
}

/// Action needed to sync source -> target for a single repo.
///
/// These actions are symmetric - they apply regardless of which
/// forge is source or target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
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

    /// Check if this is a create action
    pub fn is_create(&self) -> bool {
        matches!(self, SyncAction::Create(_))
    }

    /// Check if this is an update action
    pub fn is_update(&self) -> bool {
        matches!(self, SyncAction::Update { .. })
    }

    /// Check if this is a delete action
    pub fn is_delete(&self) -> bool {
        matches!(self, SyncAction::Delete(_))
    }

    /// Check if repos are in sync
    pub fn is_in_sync(&self) -> bool {
        matches!(self, SyncAction::InSync(_))
    }

    /// Get a short description of the action
    pub fn description(&self) -> String {
        match self {
            SyncAction::Create(repo) => format!("create {}", repo.name()),
            SyncAction::Update { repo, diffs } => {
                let props: Vec<_> = diffs.iter().map(|d| d.name()).collect();
                format!("update {} ({})", repo.name(), props.join(", "))
            }
            SyncAction::Delete(id) => format!("delete {}", id.name),
            SyncAction::InSync(id) => format!("{} in sync", id.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Visibility;

    fn make_repo(name: &str) -> Repo {
        Repo::new(RepoIdentity::new("org", name), Visibility::Public)
    }

    #[test]
    fn test_needs_action() {
        let create = SyncAction::Create(make_repo("new"));
        assert!(create.needs_action());

        let update = SyncAction::Update {
            repo: make_repo("existing"),
            diffs: vec![],
        };
        assert!(update.needs_action());

        let delete = SyncAction::Delete(RepoIdentity::new("org", "old"));
        assert!(delete.needs_action());

        let in_sync = SyncAction::InSync(RepoIdentity::new("org", "stable"));
        assert!(!in_sync.needs_action());
    }

    #[test]
    fn test_identity() {
        let repo = make_repo("test");
        let create = SyncAction::Create(repo.clone());
        assert_eq!(create.identity(), &repo.identity);

        let update = SyncAction::Update {
            repo: repo.clone(),
            diffs: vec![],
        };
        assert_eq!(update.identity(), &repo.identity);

        let identity = RepoIdentity::new("org", "other");
        let delete = SyncAction::Delete(identity.clone());
        assert_eq!(delete.identity(), &identity);
    }

    #[test]
    fn test_action_type_checks() {
        let create = SyncAction::Create(make_repo("a"));
        assert!(create.is_create());
        assert!(!create.is_update());
        assert!(!create.is_delete());
        assert!(!create.is_in_sync());

        let update = SyncAction::Update {
            repo: make_repo("b"),
            diffs: vec![],
        };
        assert!(update.is_update());
        assert!(!update.is_create());

        let delete = SyncAction::Delete(RepoIdentity::new("org", "c"));
        assert!(delete.is_delete());

        let in_sync = SyncAction::InSync(RepoIdentity::new("org", "d"));
        assert!(in_sync.is_in_sync());
    }

    #[test]
    fn test_property_diff_name() {
        let vis = PropertyDiff::Visibility {
            source: Visibility::Public,
            target: Visibility::Private,
        };
        assert_eq!(vis.name(), "visibility");

        let desc = PropertyDiff::Description {
            source: Some("old".into()),
            target: Some("new".into()),
        };
        assert_eq!(desc.name(), "description");
    }

    #[test]
    fn test_action_description() {
        let create = SyncAction::Create(make_repo("new-repo"));
        assert_eq!(create.description(), "create new-repo");

        let update = SyncAction::Update {
            repo: make_repo("existing"),
            diffs: vec![
                PropertyDiff::Visibility {
                    source: Visibility::Public,
                    target: Visibility::Private,
                },
                PropertyDiff::Description {
                    source: Some("new".into()),
                    target: Some("old".into()),
                },
            ],
        };
        assert_eq!(update.description(), "update existing (visibility, description)");

        let delete = SyncAction::Delete(RepoIdentity::new("org", "old"));
        assert_eq!(delete.description(), "delete old");
    }

    #[test]
    fn test_json_roundtrip() {
        let action = SyncAction::Update {
            repo: make_repo("test")
                .with_description("desc"),
            diffs: vec![PropertyDiff::Visibility {
                source: Visibility::Public,
                target: Visibility::Private,
            }],
        };

        let json = serde_json::to_string(&action).unwrap();
        let restored: SyncAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, restored);
    }
}
