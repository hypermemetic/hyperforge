//! Repository identity types - the unique identifier for a repository.
//!
//! A repository is uniquely identified by the combination of its organization
//! and name. This module provides types for representing this identity.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Uniquely identifies a repository within the hyperforge system.
///
/// A repository is identified by its organization and name. This is consistent
/// across all forges - the same RepoIdentity represents the same logical repo
/// whether it exists on GitHub, Codeberg, or GitLab.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct RepoIdentity {
    /// The organization name (e.g., "hypermemetic")
    pub org: String,
    /// The repository name (e.g., "hyperforge")
    pub name: String,
}

impl RepoIdentity {
    /// Create a new repository identity
    pub fn new(org: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            name: name.into(),
        }
    }

    /// Returns the full path as "org/name"
    pub fn full_path(&self) -> String {
        format!("{}/{}", self.org, self.name)
    }
}

impl fmt::Display for RepoIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_identity_new() {
        let id = RepoIdentity::new("hypermemetic", "hyperforge");
        assert_eq!(id.org, "hypermemetic");
        assert_eq!(id.name, "hyperforge");
    }

    #[test]
    fn test_repo_identity_full_path() {
        let id = RepoIdentity::new("hypermemetic", "hyperforge");
        assert_eq!(id.full_path(), "hypermemetic/hyperforge");
    }

    #[test]
    fn test_repo_identity_display() {
        let id = RepoIdentity::new("hypermemetic", "hyperforge");
        assert_eq!(format!("{}", id), "hypermemetic/hyperforge");
    }

    #[test]
    fn test_repo_identity_equality() {
        let id1 = RepoIdentity::new("hypermemetic", "hyperforge");
        let id2 = RepoIdentity::new("hypermemetic", "hyperforge");
        let id3 = RepoIdentity::new("hypermemetic", "other");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_repo_identity_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(RepoIdentity::new("hypermemetic", "hyperforge"));
        set.insert(RepoIdentity::new("hypermemetic", "hyperforge")); // duplicate

        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_repo_identity_serialization() {
        let id = RepoIdentity::new("hypermemetic", "hyperforge");
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: RepoIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }
}
