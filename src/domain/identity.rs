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
    use std::collections::{HashSet, HashMap};

    /// Table-driven tests for full_path formatting
    #[test]
    fn test_full_path_formatting() {
        let cases = [
            ("hypermemetic", "hyperforge", "hypermemetic/hyperforge"),
            ("my-org", "my-repo", "my-org/my-repo"),
            ("org_with_underscore", "repo.with.dots", "org_with_underscore/repo.with.dots"),
            ("", "empty-org", "/empty-org"),  // Edge case: empty org
            ("org", "", "org/"),              // Edge case: empty name
        ];

        for (org, name, expected) in cases {
            let id = RepoIdentity::new(org, name);
            assert_eq!(
                id.full_path(),
                expected,
                "full_path() failed for org={:?}, name={:?}",
                org,
                name
            );
            // Display should match full_path
            assert_eq!(format!("{}", id), expected);
        }
    }

    /// Verify equality semantics: same org+name = equal, different = not equal
    #[test]
    fn test_equality_semantics() {
        let base = RepoIdentity::new("hypermemetic", "hyperforge");

        // Same values = equal
        assert_eq!(base, RepoIdentity::new("hypermemetic", "hyperforge"));

        // Different name = not equal
        assert_ne!(base, RepoIdentity::new("hypermemetic", "other-repo"));

        // Different org = not equal
        assert_ne!(base, RepoIdentity::new("other-org", "hyperforge"));

        // Both different = not equal
        assert_ne!(base, RepoIdentity::new("other-org", "other-repo"));
    }

    /// Verify Hash consistency for use in HashSet/HashMap
    #[test]
    fn test_hash_consistency_for_collections() {
        // HashSet deduplication
        let mut set = HashSet::new();
        set.insert(RepoIdentity::new("org", "repo1"));
        set.insert(RepoIdentity::new("org", "repo1")); // duplicate
        set.insert(RepoIdentity::new("org", "repo2")); // different
        assert_eq!(set.len(), 2, "HashSet should deduplicate identical identities");

        // HashMap key lookup
        let mut map = HashMap::new();
        map.insert(RepoIdentity::new("org", "repo"), "value1");
        map.insert(RepoIdentity::new("org", "repo"), "value2"); // overwrites
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&RepoIdentity::new("org", "repo")), Some(&"value2"));
    }

    /// Verify JSON roundtrip preserves identity
    #[test]
    fn test_json_roundtrip() {
        let original = RepoIdentity::new("hypermemetic", "hyperforge");
        let json = serde_json::to_string(&original).unwrap();
        let restored: RepoIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);

        // Verify JSON structure
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["org"], "hypermemetic");
        assert_eq!(value["name"], "hyperforge");
    }
}
