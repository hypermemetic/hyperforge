use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SecretProvider {
    Keychain,
    Env,
    File,
    Pass,
}

impl Default for SecretProvider {
    fn default() -> Self {
        SecretProvider::Keychain
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretKey {
    pub key: String,
    pub provider: SecretProvider,
    pub is_set: bool,
}

/// Well-known secret keys
pub mod keys {
    pub const GITHUB_TOKEN: &str = "github-token";
    pub const CODEBERG_TOKEN: &str = "codeberg-token";
    pub const CRATES_TOKEN: &str = "crates-token";
    pub const HACKAGE_TOKEN: &str = "hackage-token";
}
