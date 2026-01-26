//! Forge adapters implementing ForgePort trait

pub mod forge_port;
pub mod github;
pub mod local_forge;

pub use forge_port::{ForgeError, ForgePort, ForgeResult};
pub use github::GitHubAdapter;
pub use local_forge::LocalForge;
