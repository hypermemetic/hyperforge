//! Domain types for hyperforge hexagonal architecture.
//!
//! This module contains pure domain types with NO I/O dependencies.
//! All types here are plain data structures that can be:
//! - Serialized/deserialized
//! - Compared for equality
//! - Used in pure functions
//!
//! ## Design Principles
//!
//! 1. **No I/O**: Types in this module never perform network requests,
//!    file operations, or any other side effects.
//!
//! 2. **Pure functions**: Computation (like diffing) is done through
//!    pure functions that take inputs and return outputs.
//!
//! 3. **Reuse existing types**: Where appropriate, we reuse types from
//!    `crate::types` (like `Forge`, `Visibility`) rather than duplicating.
//!
//! ## Module Structure
//!
//! - `identity` - Repository identity (org + name)
//! - `repo` - Unified repository type (replaces desired/observed split)
//! - `sync_action` - Symmetric sync actions between forges
//! - `desired` - Desired state (legacy, use `Repo` instead)
//! - `observed` - Observed state (legacy, use `Repo` instead)
//! - `diff` - Diff computation (what actions are needed)
//! - `plan` - Sync plan (aggregated diffs with summary)

pub mod identity;
pub mod repo;
pub mod sync_action;
pub mod desired;
pub mod observed;
pub mod diff;
pub mod plan;

// Re-export main types for convenience
pub use identity::RepoIdentity;

// New unified types
pub use repo::Repo;
pub use sync_action::{SyncAction, PropertyDiff};
pub use diff::compute_sync_actions;

// Legacy types (still used during migration)
pub use desired::DesiredRepo;
pub use observed::{ObservedRepo, ForgeRepoState};
pub use diff::{RepoDiff, ForgeAction, PropertyChanges};
pub use plan::{SyncPlan, PlanSummary};
