use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Forge, ForgesConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Org {
    pub name: String,
    pub owner: String,
    pub ssh_key: String,
    pub origin: Forge,
    /// Forges configuration - supports both legacy array format and new object format
    pub forges: ForgesConfig,
    pub default_visibility: Visibility,
}

impl Org {
    /// Get the SSH host alias for a forge
    /// Pattern: `<forge>-<org_name>` (e.g., `github-hypermemetic`)
    pub fn ssh_host(&self, forge: &Forge) -> String {
        format!("{}-{}", forge.to_string().to_lowercase(), self.name)
    }

    /// Get the SSH URL for a repository on a forge
    /// Pattern: `git@<ssh_host>:<owner>/<repo>.git`
    pub fn ssh_url(&self, forge: &Forge, repo_name: &str) -> String {
        format!("git@{}:{}/{}.git", self.ssh_host(forge), self.owner, repo_name)
    }

    /// Get the SSH URL for origin forge
    pub fn origin_url(&self, repo_name: &str) -> String {
        self.ssh_url(&self.origin, repo_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OrgSummary {
    pub name: String,
    pub owner: String,
    pub forges: ForgesConfig,
}

impl From<&Org> for OrgSummary {
    fn from(org: &Org) -> Self {
        OrgSummary {
            name: org.name.clone(),
            owner: org.owner.clone(),
            forges: org.forges.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Private,
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Visibility::Public => write!(f, "public"),
            Visibility::Private => write!(f, "private"),
        }
    }
}
