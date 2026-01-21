pub mod activations;
pub mod bridge;
pub mod error;
pub mod events;
pub mod hub;
pub mod storage;
pub mod templates;
pub mod types;

// Re-export serde_helpers from hub_core (required by hub-macro generated code)
pub use hub_core::serde_helpers;

// Explicit exports from activations (avoids conflict with types::forge, types::org, types::workspace)
pub use activations::{
    ForgeActivation, OrgActivation, ReposActivation, RepoChildRouter,
    SecretsActivation, WorkspaceActivation,
};
pub use bridge::*;
pub use error::*;
pub use events::*;
pub use hub::HyperforgeHub;
pub use storage::*;
pub use types::*;
