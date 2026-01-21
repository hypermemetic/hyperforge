pub mod codeberg;
pub mod forge_client;
mod git_remote;
pub mod github;
mod keychain;
pub mod package_registry;
pub mod pulumi;
pub mod secret_store;
pub mod validated_client;

// NOTE: ssh_config.rs removed - we now use per-repo git config (core.sshCommand)
// instead of global ~/.ssh/config host aliases. See GitRemoteBridge::ensure_ssh_config().

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
pub use package_registry::{PackageRegistry, create_registry, CratesRegistry, NpmRegistry, HexRegistry, HackageRegistry, PyPiRegistry};
pub use pulumi::PulumiBridge;
pub use secret_store::{SecretStore, create_secret_store, KeychainStore, EnvStore, FileStore, PassStore};
pub use validated_client::{create_validated_client, ValidatedForgeClient};
