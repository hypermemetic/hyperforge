//! Forge adapters implementing ForgePort trait

pub mod codeberg;
pub mod forge_port;
pub mod github;
pub mod gitlab;
pub mod local_forge;

pub use codeberg::CodebergAdapter;
pub use forge_port::{ForgeError, ForgePort, ForgeResult, ListResult};
pub use github::GitHubAdapter;
pub use gitlab::GitLabAdapter;
pub use local_forge::{ForgeSyncState, LocalForge};
