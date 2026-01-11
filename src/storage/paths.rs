use std::path::PathBuf;

/// Manages all filesystem paths for hyperforge configuration
pub struct HyperforgePaths {
    pub config_dir: PathBuf,
}

impl HyperforgePaths {
    /// Create a new HyperforgePaths instance, using ~/.config/hyperforge
    pub fn new() -> Self {
        let home = std::env::var("HOME").expect("HOME environment variable not set");
        let config_dir = PathBuf::from(home).join(".config").join("hyperforge");
        Self { config_dir }
    }

    /// Create directories if they don't exist
    pub async fn ensure_dirs(&self) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.config_dir).await?;
        tokio::fs::create_dir_all(self.orgs_dir()).await?;
        Ok(())
    }

    /// Path to the global config.yaml file
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    /// Path to the orgs directory
    pub fn orgs_dir(&self) -> PathBuf {
        self.config_dir.join("orgs")
    }

    /// Path to a specific org's directory
    pub fn org_dir(&self, org_name: &str) -> PathBuf {
        self.orgs_dir().join(org_name)
    }

    /// Path to an org's repos.yaml file
    pub fn repos_file(&self, org_name: &str) -> PathBuf {
        self.org_dir(org_name).join("repos.yaml")
    }

    /// Path to an org's staged-repos.yaml file
    pub fn staged_repos_file(&self, org_name: &str) -> PathBuf {
        self.org_dir(org_name).join("staged-repos.yaml")
    }
}

impl Default for HyperforgePaths {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HyperforgePaths {
    fn clone(&self) -> Self {
        Self {
            config_dir: self.config_dir.clone(),
        }
    }
}
