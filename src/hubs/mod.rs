//! Sub-hub modules for hyperforge's nested plugin hierarchy
//!
//! Each sub-hub is a leaf plugin under the root `hyperforge` hub.

pub mod repo;
pub mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::LocalForge;
use crate::services::SymmetricSyncService;

pub use repo::RepoHub;
pub use workspace::WorkspaceHub;

/// Shared state for all hyperforge sub-hubs
#[derive(Clone)]
pub struct HyperforgeState {
    pub sync_service: Arc<SymmetricSyncService>,
    /// Cached LocalForge instances per org
    pub local_forges: Arc<RwLock<HashMap<String, Arc<LocalForge>>>>,
    /// Base config directory (~/.config/hyperforge)
    pub config_dir: PathBuf,
}

impl HyperforgeState {
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("hyperforge");

        Self {
            sync_service: Arc::new(SymmetricSyncService::new()),
            local_forges: Arc::new(RwLock::new(HashMap::new())),
            config_dir,
        }
    }

    /// Get or create LocalForge for an org with file persistence
    pub async fn get_local_forge(&self, org: &str) -> Arc<LocalForge> {
        // Try to get existing
        {
            let forges = self.local_forges.read().unwrap();
            if let Some(forge) = forges.get(org) {
                return forge.clone();
            }
        }

        // Create new with persistence
        let yaml_path = self.config_dir.join("orgs").join(org).join("repos.yaml");
        let forge = Arc::new(LocalForge::with_config_path(org, yaml_path));

        // Try to load existing state
        let _ = forge.load_from_yaml().await;

        // Cache it
        {
            let mut forges = self.local_forges.write().unwrap();
            forges.insert(org.to_string(), forge.clone());
        }

        forge
    }
}

impl Default for HyperforgeState {
    fn default() -> Self {
        Self::new()
    }
}
