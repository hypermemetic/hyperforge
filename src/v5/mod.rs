//! Hyperforge v5 — ground-up rewrite living alongside v4.
//!
//! Design invariants and epic docs live under `plans/v5/`. This module
//! holds the v5 activation tree, config loaders, and secret store. The
//! v4 activation tree in `crate::hub` / `crate::hubs` is untouched.

pub mod config;
pub mod hub;
pub mod orgs;
pub mod repos;
pub mod workspaces;
