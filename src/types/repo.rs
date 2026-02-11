//! Repository types for LFORGE2

use serde::{Deserialize, Serialize};
use super::{Forge, Visibility};

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// Repository configuration with origin and mirrors
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    /// Repository name (without owner prefix)
    pub name: String,

    /// Repository description
    pub description: Option<String>,

    /// Repository visibility (public/private)
    pub visibility: Visibility,

    /// Primary forge (source of truth)
    pub origin: Forge,

    /// Mirror forges (read-only copies)
    #[serde(default)]
    pub mirrors: Vec<Forge>,

    /// Whether this repo is protected from deletion
    #[serde(default)]
    pub protected: bool,

    /// Whether this repo is staged for deletion by `workspace reflect`
    #[serde(default, skip_serializing_if = "is_false")]
    pub staged_for_deletion: bool,
}

impl Repo {
    /// Create a new repository configuration
    pub fn new(name: impl Into<String>, origin: Forge) -> Self {
        Self {
            name: name.into(),
            description: None,
            visibility: Visibility::Public,
            origin,
            mirrors: Vec::new(),
            protected: false,
            staged_for_deletion: false,
        }
    }

    /// Set repository description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set repository visibility
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Add a mirror forge
    pub fn with_mirror(mut self, forge: Forge) -> Self {
        if !self.mirrors.contains(&forge) && forge != self.origin {
            self.mirrors.push(forge);
        }
        self
    }

    /// Set multiple mirrors
    pub fn with_mirrors(mut self, mirrors: Vec<Forge>) -> Self {
        self.mirrors = mirrors.into_iter()
            .filter(|f| *f != self.origin)
            .collect();
        self
    }

    /// Mark as protected
    pub fn with_protected(mut self, protected: bool) -> Self {
        self.protected = protected;
        self
    }

    /// Mark as staged for deletion
    pub fn with_staged_for_deletion(mut self, staged: bool) -> Self {
        self.staged_for_deletion = staged;
        self
    }

    /// Get all forges (origin + mirrors)
    pub fn all_forges(&self) -> Vec<Forge> {
        let mut forges = vec![self.origin.clone()];
        forges.extend(self.mirrors.clone());
        forges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_new() {
        let repo = Repo::new("test-repo", Forge::GitHub);
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.origin, Forge::GitHub);
        assert!(repo.mirrors.is_empty());
        assert!(!repo.protected);
    }

    #[test]
    fn test_repo_builder() {
        let repo = Repo::new("my-app", Forge::GitHub)
            .with_description("My application")
            .with_visibility(Visibility::Private)
            .with_mirror(Forge::Codeberg)
            .with_protected(true);

        assert_eq!(repo.name, "my-app");
        assert_eq!(repo.description, Some("My application".to_string()));
        assert_eq!(repo.visibility, Visibility::Private);
        assert_eq!(repo.origin, Forge::GitHub);
        assert_eq!(repo.mirrors, vec![Forge::Codeberg]);
        assert!(repo.protected);
    }

    #[test]
    fn test_repo_mirrors_excludes_origin() {
        let repo = Repo::new("test", Forge::GitHub)
            .with_mirrors(vec![Forge::GitHub, Forge::Codeberg, Forge::GitLab]);

        // Should exclude GitHub since it's the origin
        assert_eq!(repo.mirrors.len(), 2);
        assert!(repo.mirrors.contains(&Forge::Codeberg));
        assert!(repo.mirrors.contains(&Forge::GitLab));
        assert!(!repo.mirrors.contains(&Forge::GitHub));
    }

    #[test]
    fn test_repo_all_forges() {
        let repo = Repo::new("test", Forge::GitHub)
            .with_mirror(Forge::Codeberg);

        let all = repo.all_forges();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], Forge::GitHub); // Origin first
        assert_eq!(all[1], Forge::Codeberg);
    }
}
