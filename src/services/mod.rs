//! Service layer for hyperforge hexagonal architecture.
//!
//! Services orchestrate business operations by coordinating between ports.
//! They contain application-level logic that doesn't belong in the pure domain
//! but needs to interact with external systems through ports.
//!
//! ## Architecture
//!
//! ```text
//!              ┌─────────────────────┐
//!              │    Application      │
//!              │   (CLI, Hub, API)   │
//!              └──────────┬──────────┘
//!                         │
//!              ┌──────────▼──────────┐
//!              │      Services       │
//!              │  - SyncService      │
//!              │  - ImportService    │
//!              │  - DiffService      │
//!              └────────┬─┬──────────┘
//!                       │ │
//!           ┌───────────┘ └───────────┐
//!           ▼                         ▼
//!   ┌───────────────┐         ┌───────────────┐
//!   │   ForgePort   │         │  StoragePort  │
//!   └───────────────┘         └───────────────┘
//! ```
//!
//! ## Design Principles
//!
//! 1. **Dependency Injection**: Services accept ports via constructor,
//!    enabling easy testing with mock implementations.
//!
//! 2. **Orchestration Only**: Services coordinate between ports and domain
//!    logic but don't contain domain rules themselves.
//!
//! 3. **Error Aggregation**: Services have their own error types that wrap
//!    port-specific errors with context.
//!
//! ## Example
//!
//! ```ignore
//! use hyperforge::services::{SyncService, SyncError};
//! use hyperforge::adapters::{GitHubAdapter, YamlStorageAdapter};
//! use std::sync::Arc;
//!
//! // Create adapters
//! let github = Arc::new(GitHubAdapter::new("token"));
//! let storage = Arc::new(YamlStorageAdapter::default_path());
//!
//! // Create service with injected dependencies
//! let sync = SyncService::new(vec![github], storage);
//!
//! // Execute sync operation
//! let results = sync.execute_plan(&plan, false).await?;
//! ```

pub mod diff;
pub mod import;
pub mod sync;

// Re-export services and their error types
pub use diff::{DiffService, DiffError, DiffOptions};
pub use import::{ImportService, ImportError};
pub use sync::{SyncService, SyncError, SyncResult, SyncOutcome};
