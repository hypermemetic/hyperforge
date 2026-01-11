pub mod codeberg;
pub mod forge_client;
mod git_remote;
pub mod github;
mod keychain;
pub mod pulumi;
mod ssh_config;
pub mod validated_client;

#[cfg(test)]
pub mod mock_forge_client;

// Re-export mock for integration tests
#[cfg(any(test, feature = "test-support"))]
pub use mock_forge_client::{MockForgeClient, MockConfig, MockError, ForgeRepoBuilder};

pub use codeberg::CodebergClient;
pub use forge_client::{
    create_client, AuthStatus, ForgeClient, ForgeError, ForgeRepo, ForgeResult, RepoCreateConfig,
};
pub use git_remote::GitRemoteBridge;
pub use github::GitHubClient;
pub use keychain::KeychainBridge;
pub use pulumi::PulumiBridge;
pub use ssh_config::SshConfigBridge;
pub use validated_client::{create_validated_client, ValidatedForgeClient};
