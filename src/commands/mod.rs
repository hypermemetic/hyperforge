//! Hyperforge commands
//!
//! This module contains the implementation of hyperforge CLI commands.

pub mod hooks;
pub mod init;
pub mod materialize;
pub mod push;
pub mod runner;
pub mod status;
pub mod workspace;

pub use init::{init, InitOptions, InitResult};
pub use materialize::{materialize, MaterializeOpts, MaterializeReport};
pub use push::{push, ForgePushResult, PushOptions, PushReport, PushResult};
pub use status::{status, ForgeStatus, RepoStatusReport, StatusResult};
pub use workspace::{
    discover_workspace, repo_from_config, DiscoveredRepo, WorkspaceContext, WorkspaceResult,
};
