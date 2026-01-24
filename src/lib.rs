//! Hyperforge - Multi-forge repository management
//!
//! Hyperforge manages repositories across multiple git forges (GitHub, Codeberg, GitLab)
//! using declarative configuration and git as the source of truth.

pub mod auth;
pub mod config;
pub mod git;
pub mod package;
pub mod remote;
pub mod types;

// Re-exports for convenience
pub use config::HyperforgeConfig;
pub use types::*;
