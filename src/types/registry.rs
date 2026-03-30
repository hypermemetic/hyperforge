//! Container registry types — strongly typed image references, registries, and auth.

use serde::{Deserialize, Serialize};

use super::Forge;

/// Supported container registries, derived from forge or explicit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRegistry {
    /// GitHub Container Registry (ghcr.io)
    Ghcr,
    /// Codeberg OCI registry (codeberg.org)
    Codeberg,
    /// GitLab Container Registry (registry.gitlab.com)
    GitLab,
    /// Arbitrary OCI-compliant registry
    Custom(String),
}

impl ContainerRegistry {
    /// Registry hostname for OCI operations
    pub fn host(&self) -> &str {
        match self {
            Self::Ghcr => "ghcr.io",
            Self::Codeberg => "codeberg.org",
            Self::GitLab => "registry.gitlab.com",
            Self::Custom(host) => host.as_str(),
        }
    }

    /// The forge name used for token lookup (e.g. "github" -> "github/{org}/packages_token")
    pub fn token_forge_name(&self) -> &str {
        match self {
            Self::Ghcr => "github",
            Self::Codeberg => "codeberg",
            Self::GitLab => "gitlab",
            Self::Custom(_) => "custom",
        }
    }
}

impl From<&Forge> for ContainerRegistry {
    fn from(forge: &Forge) -> Self {
        match forge {
            Forge::GitHub => Self::Ghcr,
            Forge::Codeberg => Self::Codeberg,
            Forge::GitLab => Self::GitLab,
        }
    }
}

impl From<Forge> for ContainerRegistry {
    fn from(forge: Forge) -> Self {
        Self::from(&forge)
    }
}

/// A fully typed container image reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRef {
    pub registry: ContainerRegistry,
    pub org: String,
    pub name: String,
    pub tag: String,
}

impl ImageRef {
    pub fn new(registry: ContainerRegistry, org: impl Into<String>, name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            registry,
            org: org.into(),
            name: name.into(),
            tag: tag.into(),
        }
    }

    /// Full image reference: {host}/{org}/{name}:{tag}
    pub fn full_name(&self) -> String {
        format!("{}/{}/{}:{}", self.registry.host(), self.org, self.name, self.tag)
    }

    /// OCI reference string for oci-client: {host}/{org}/{name}:{tag}
    pub fn oci_reference(&self) -> String {
        self.full_name()
    }

    /// Local image ref (no registry): {name}:{tag}
    pub fn local_name(&self) -> String {
        format!("{}:{}", self.name, self.tag)
    }
}

impl std::fmt::Display for ImageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.full_name())
    }
}

/// Credentials for authenticating to a container registry.
#[derive(Debug, Clone)]
pub enum RegistryAuth {
    /// Bearer/PAT token
    Token(String),
    /// Username + password/token
    Basic { username: String, password: String },
    /// No authentication
    Anonymous,
}

impl RegistryAuth {
    /// Resolve credentials for a registry + org from the auth provider.
    /// Tries packages_token first, then falls back to token.
    pub async fn resolve(
        registry: &ContainerRegistry,
        org: &str,
        auth: &dyn crate::auth::AuthProvider,
    ) -> Result<Self, String> {
        let forge = registry.token_forge_name();

        // Try packages_token first (classic PAT with package scopes)
        let packages_path = format!("{}/{}/packages_token", forge, org);
        if let Ok(Some(token)) = auth.get_secret(&packages_path).await {
            return Ok(Self::Token(token));
        }

        // Fall back to regular token
        let default_path = format!("{}/{}/token", forge, org);
        match auth.get_secret(&default_path).await {
            Ok(Some(token)) => Ok(Self::Token(token)),
            Ok(None) => Err(format!(
                "No token found for {} (tried {}/{}/packages_token and {}/{}/token)",
                registry.host(), forge, org, forge, org,
            )),
            Err(e) => Err(format!("Auth error: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_registry_from_forge() {
        assert_eq!(ContainerRegistry::from(Forge::GitHub), ContainerRegistry::Ghcr);
        assert_eq!(ContainerRegistry::from(Forge::Codeberg), ContainerRegistry::Codeberg);
        assert_eq!(ContainerRegistry::from(Forge::GitLab), ContainerRegistry::GitLab);
    }

    #[test]
    fn test_container_registry_host() {
        assert_eq!(ContainerRegistry::Ghcr.host(), "ghcr.io");
        assert_eq!(ContainerRegistry::Codeberg.host(), "codeberg.org");
        assert_eq!(ContainerRegistry::GitLab.host(), "registry.gitlab.com");
        assert_eq!(ContainerRegistry::Custom("my.reg.io".into()).host(), "my.reg.io");
    }

    #[test]
    fn test_image_ref_full_name() {
        let img = ImageRef::new(ContainerRegistry::Ghcr, "hypermemetic", "substrate", "v1.0");
        assert_eq!(img.full_name(), "ghcr.io/hypermemetic/substrate:v1.0");
        assert_eq!(img.local_name(), "substrate:v1.0");
    }

    #[test]
    fn test_image_ref_custom_registry() {
        let img = ImageRef::new(
            ContainerRegistry::Custom("harbor.internal".into()),
            "team", "app", "latest",
        );
        assert_eq!(img.full_name(), "harbor.internal/team/app:latest");
    }
}
