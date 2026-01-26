# LFORGE-5: LocalForge Persistence

**blocked_by:** [LFORGE-2]
**unlocks:** [LFORGE-6]

## Scope

Add persistence capabilities to `LocalForge` so it can save/load state from disk. The format should be compatible with the existing `repos.yaml` format used by `YamlStorageAdapter`. This allows LocalForge to replace YamlStorageAdapter while maintaining backward compatibility.

## Deliverables

1. `save(&self, path: &Path) -> Result<()>` method on LocalForge
2. `load(path: &Path) -> Result<Self>` constructor
3. `LocalForge::with_auto_save(path: &Path)` constructor for automatic persistence
4. YAML format compatible with existing `repos.yaml`
5. Migration tests showing old format loads correctly

## Verification Steps

```bash
# Run persistence tests
cd ~/dev/controlflow/hypermemetic/hyperforge
cargo test local_forge::persistence

# Test loading existing config
cargo test test_load_existing_repos_yaml

# Verify YAML format compatibility
cat ~/.config/hyperforge/orgs/hypermemetic/repos.yaml
```

## Implementation Notes

### Persistence Methods

Add to `src/adapters/local_forge.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Configuration for LocalForge persistence
#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    /// Path to save/load from
    pub path: PathBuf,
    /// Organization name (for YAML format compatibility)
    pub org: String,
    /// Auto-save on mutations
    pub auto_save: bool,
}

impl LocalForge {
    /// Load LocalForge state from a YAML file.
    ///
    /// The file format is compatible with the existing repos.yaml format:
    /// ```yaml
    /// owner: myorg
    /// repos:
    ///   repo-name:
    ///     description: "..."
    ///     visibility: public
    ///     forges: [github, codeberg]
    /// ```
    pub fn load(path: &Path) -> Result<Self, PersistenceError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| PersistenceError::ReadError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let file: ReposYamlFile = serde_yaml::from_str(&contents)
            .map_err(|e| PersistenceError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let forge = Self::new();
        {
            let mut map = forge.repos.write().unwrap();
            let org_repos = map.entry(file.owner.clone()).or_default();

            for (name, config) in file.repos {
                org_repos.insert(name.clone(), LocalRepo {
                    identity: RepoIdentity::new(&file.owner, &name),
                    description: config.description,
                    visibility: config.visibility,
                });
            }
        }

        Ok(forge)
    }

    /// Load or create a LocalForge from a path.
    ///
    /// If the file doesn't exist, returns an empty LocalForge.
    pub fn load_or_create(path: &Path) -> Result<Self, PersistenceError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::new())
        }
    }

    /// Save LocalForge state to a YAML file.
    ///
    /// Saves in the repos.yaml compatible format.
    pub fn save(&self, path: &Path, org: &str) -> Result<(), PersistenceError> {
        let map = self.repos.read().unwrap();
        let org_repos = map.get(org).cloned().unwrap_or_default();

        let mut repos = std::collections::HashMap::new();
        for (name, local) in org_repos {
            repos.insert(name, RepoYamlConfig {
                description: local.description,
                visibility: local.visibility,
                forges: vec![], // LocalForge doesn't track target forges
                protected: false,
                delete: false,
            });
        }

        let file = ReposYamlFile {
            owner: org.to_string(),
            repos,
        };

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PersistenceError::WriteError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
        }

        let contents = serde_yaml::to_string(&file)
            .map_err(|e| PersistenceError::SerializeError {
                message: e.to_string(),
            })?;

        std::fs::write(path, contents)
            .map_err(|e| PersistenceError::WriteError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        Ok(())
    }

    /// Create a LocalForge with auto-save enabled.
    ///
    /// Every mutation (create, update, delete) will automatically
    /// save to the specified path.
    pub fn with_auto_save(path: PathBuf, org: String) -> Result<Self, PersistenceError> {
        let forge = Self::load_or_create(&path)?;
        forge.persistence_config.write().unwrap().replace(PersistenceConfig {
            path,
            org,
            auto_save: true,
        });
        Ok(forge)
    }

    /// Trigger auto-save if enabled
    fn maybe_auto_save(&self) -> Result<(), PersistenceError> {
        let config = self.persistence_config.read().unwrap();
        if let Some(config) = config.as_ref() {
            if config.auto_save {
                self.save(&config.path, &config.org)?;
            }
        }
        Ok(())
    }
}

/// YAML file format (compatible with existing repos.yaml)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ReposYamlFile {
    owner: String,
    repos: std::collections::HashMap<String, RepoYamlConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RepoYamlConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    visibility: Visibility,
    #[serde(default)]
    forges: Vec<Forge>,
    #[serde(default)]
    protected: bool,
    #[serde(default, rename = "_delete")]
    delete: bool,
}

/// Errors from persistence operations
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("Failed to read {path}: {message}")]
    ReadError { path: PathBuf, message: String },

    #[error("Failed to parse {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Failed to write {path}: {message}")]
    WriteError { path: PathBuf, message: String },

    #[error("Failed to serialize: {message}")]
    SerializeError { message: String },
}
```

### Updated LocalForge Struct

```rust
pub struct LocalForge {
    /// Nested map: org_name -> (repo_name -> repo_data)
    repos: RwLock<HashMap<String, HashMap<String, LocalRepo>>>,
    /// Optional persistence configuration
    persistence_config: RwLock<Option<PersistenceConfig>>,
}

impl LocalForge {
    pub fn new() -> Self {
        Self {
            repos: RwLock::new(HashMap::new()),
            persistence_config: RwLock::new(None),
        }
    }
}

// Update ForgePort implementation to trigger auto-save
#[async_trait]
impl ForgePort for LocalForge {
    async fn create_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        // ... existing implementation ...

        // Trigger auto-save after successful mutation
        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(observed)
    }

    async fn update_repo(&self, repo: &DesiredRepo) -> Result<ObservedRepo, ForgeError> {
        // ... existing implementation ...

        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(observed)
    }

    async fn delete_repo(&self, identity: &RepoIdentity) -> Result<(), ForgeError> {
        // ... existing implementation ...

        self.maybe_auto_save()
            .map_err(|e| ForgeError::api_error(Forge::Local, e.to_string()))?;

        Ok(())
    }
}
```

### Tests

```rust
#[cfg(test)]
mod persistence_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("repos.yaml");

        // Create and populate a LocalForge
        let forge1 = LocalForge::new();
        {
            let mut map = forge1.repos.write().unwrap();
            let org_repos = map.entry("myorg".to_string()).or_default();
            org_repos.insert("repo1".to_string(), LocalRepo {
                identity: RepoIdentity::new("myorg", "repo1"),
                description: Some("Test repo".to_string()),
                visibility: Visibility::Public,
            });
        }

        // Save
        forge1.save(&path, "myorg").unwrap();

        // Load into new instance
        let forge2 = LocalForge::load(&path).unwrap();

        // Verify content matches
        let repos = forge2.get_org_repos("myorg");
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "repo1");
    }

    #[test]
    fn test_load_existing_repos_yaml_format() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("repos.yaml");

        // Write a file in the existing format
        let yaml = r#"
owner: hypermemetic
repos:
  hyperforge:
    description: "Multi-forge repo manager"
    visibility: public
    forges:
      - github
      - codeberg
    protected: true
  dotfiles:
    visibility: private
    forges:
      - github
"#;
        std::fs::write(&path, yaml).unwrap();

        // Load it
        let forge = LocalForge::load(&path).unwrap();

        let repos = forge.get_org_repos("hypermemetic");
        assert_eq!(repos.len(), 2);

        // Verify repo details preserved
        let map = forge.repos.read().unwrap();
        let hyperforge = &map["hypermemetic"]["hyperforge"];
        assert_eq!(hyperforge.description, Some("Multi-forge repo manager".to_string()));
        assert_eq!(hyperforge.visibility, Visibility::Public);

        let dotfiles = &map["hypermemetic"]["dotfiles"];
        assert_eq!(dotfiles.visibility, Visibility::Private);
    }

    #[test]
    fn test_load_or_create_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does-not-exist.yaml");

        let forge = LocalForge::load_or_create(&path).unwrap();
        assert!(forge.get_org_repos("anything").is_empty());
    }

    #[tokio::test]
    async fn test_auto_save_on_create() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("repos.yaml");

        let forge = LocalForge::with_auto_save(path.clone(), "myorg".to_string()).unwrap();

        // Create a repo
        let desired = make_desired("myorg", "new-repo");
        forge.create_repo(&desired).await.unwrap();

        // File should exist now
        assert!(path.exists());

        // Load from file and verify
        let forge2 = LocalForge::load(&path).unwrap();
        assert_eq!(forge2.get_org_repos("myorg").len(), 1);
    }

    #[tokio::test]
    async fn test_auto_save_on_delete() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("repos.yaml");

        // Start with one repo
        let yaml = r#"
owner: myorg
repos:
  to-delete:
    visibility: public
"#;
        std::fs::write(&path, yaml).unwrap();

        let forge = LocalForge::with_auto_save(path.clone(), "myorg".to_string()).unwrap();
        assert_eq!(forge.get_org_repos("myorg").len(), 1);

        // Delete it
        forge.delete_repo(&RepoIdentity::new("myorg", "to-delete")).await.unwrap();

        // Reload and verify it's gone
        let forge2 = LocalForge::load(&path).unwrap();
        assert!(forge2.get_org_repos("myorg").is_empty());
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("deep").join("repos.yaml");

        let forge = LocalForge::new();
        forge.save(&path, "myorg").unwrap();

        assert!(path.exists());
    }

    fn make_desired(org: &str, name: &str) -> DesiredRepo {
        use std::collections::HashSet;
        DesiredRepo::new(
            RepoIdentity::new(org, name),
            Visibility::Public,
            HashSet::new(),
        )
    }
}
```

### Key Design Decisions

1. **Format compatibility**: Uses same YAML structure as existing repos.yaml
2. **Auto-save optional**: Can use LocalForge in-memory only or with persistence
3. **Single org per file**: Matches existing file structure
4. **Graceful degradation**: load_or_create returns empty forge if file missing
5. **Parent directory creation**: save() creates directories as needed
6. **Thread-safe persistence config**: Uses RwLock for config storage
