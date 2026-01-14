//! Adapters for external systems in hyperforge hexagonal architecture.
//!
//! Adapters implement port traits, translating between domain types and
//! external systems (APIs, databases, filesystems, etc.). They handle
//! the messy details of external communication so the domain stays pure.
//!
//! ## Forge Adapters
//!
//! - [`GitHubAdapter`] - Adapter for GitHub API
//! - [`CodebergAdapter`] - Adapter for Codeberg (Gitea) API
//!
//! ## Storage Adapters
//!
//! - [`YamlStorageAdapter`] - YAML file-based storage (production)
//! - [`InMemoryStorageAdapter`] - In-memory storage (testing)
//!
//! ## Usage
//!
//! ```ignore
//! use hyperforge::adapters::{GitHubAdapter, CodebergAdapter};
//! use hyperforge::ports::ForgePort;
//!
//! // Create adapters with API tokens
//! let github = GitHubAdapter::new("github-token");
//! let codeberg = CodebergAdapter::new("codeberg-token");
//!
//! // Use through the ForgePort trait
//! let repos = github.list_repos("my-org").await?;
//! ```
//!
//! ```ignore
//! use hyperforge::adapters::{YamlStorageAdapter, InMemoryStorageAdapter};
//! use hyperforge::ports::StoragePort;
//!
//! // Production: use YAML files
//! let storage = YamlStorageAdapter::default_path();
//!
//! // Testing: use in-memory
//! let storage = InMemoryStorageAdapter::new();
//! ```

pub mod codeberg;
pub mod github;
pub mod memory_storage;
pub mod org_storage;
pub mod yaml_storage;

pub use codeberg::CodebergAdapter;
pub use github::GitHubAdapter;
pub use memory_storage::InMemoryStorageAdapter;
pub use org_storage::OrgStorageAdapter;
pub use yaml_storage::YamlStorageAdapter;
