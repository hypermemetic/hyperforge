//! Hyperforge - Multi-forge repository management
//!
//! Hyperforge manages repositories across multiple git forges (GitHub, Codeberg, GitLab)
//! using declarative configuration and git as the source of truth.

pub mod adapters;
pub mod auth;
pub mod auth_hub;
pub mod build_system;
pub mod commands;
pub mod config;
pub mod git;
pub mod hub;
pub mod hubs;
pub mod package;
pub mod registry;
pub mod remote;
pub mod services;
pub mod types;

// Re-export serde_helpers from plexus_core (required by plexus_macros generated code)
pub use plexus_core::serde_helpers;

// Re-exports for convenience
pub use adapters::{ForgePort, LocalForge};
pub use auth_hub::{AuthEvent, AuthHub};
pub use config::HyperforgeConfig;
pub use hub::{HyperforgeEvent, HyperforgeHub};
pub use services::{SymmetricSyncService, SyncDiff, SyncOp};
pub use types::*;
