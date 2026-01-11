use thiserror::Error;

#[derive(Debug, Error)]
pub enum HyperforgeError {
    #[error("Organization not found: {0}")]
    OrgNotFound(String),

    #[error("Repository not found: {org}/{repo}")]
    RepoNotFound { org: String, repo: String },

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Storage error: {0}")]
    StorageError(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("Pulumi error: {0}")]
    PulumiError(String),

    #[error("Forge API error: {forge} - {message}")]
    ForgeApiError { forge: String, message: String },
}

pub type Result<T> = std::result::Result<T, HyperforgeError>;
