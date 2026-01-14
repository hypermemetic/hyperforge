//! Port traits for hyperforge hexagonal architecture.
//!
//! This module defines the **ports** (interfaces) that separate domain logic
//! from infrastructure concerns. Ports define capabilities needed by the domain
//! without specifying implementations.
//!
//! ## Architecture
//!
//! ```text
//!                    ┌─────────────────┐
//!                    │     Domain      │
//!                    │ (pure logic)    │
//!                    └────────┬────────┘
//!                             │
//!              ┌──────────────┼──────────────┐
//!              │              │              │
//!        ┌─────▼─────┐  ┌─────▼─────┐  ┌─────▼─────┐
//!        │ ForgePort │  │StoragePort│  │SecretsPort│
//!        └─────┬─────┘  └─────┬─────┘  └─────┬─────┘
//!              │              │              │
//!        ┌─────▼─────┐  ┌─────▼─────┐  ┌─────▼─────┐
//!        │  GitHub   │  │   Disk    │  │  Keychain │
//!        │  Codeberg │  │   JSON    │  │   Env     │
//!        └───────────┘  └───────────┘  └───────────┘
//! ```
//!
//! ## Design Principles
//!
//! 1. **Domain types only**: Ports use types from `crate::domain`, never
//!    infrastructure-specific types.
//!
//! 2. **Object-safe traits**: All traits can be used as `dyn Trait` for
//!    runtime polymorphism.
//!
//! 3. **Async-first**: Methods are async using `#[async_trait]` for
//!    compatibility with async runtimes.
//!
//! 4. **Error per port**: Each port has its own error type for precise
//!    error handling.

pub mod forge;
pub mod secrets;
pub mod storage;

// Re-export traits and error types
pub use forge::{ForgeError, ForgePort};
pub use secrets::{SecretsError, SecretsPort};
pub use storage::{StorageError, StoragePort};
