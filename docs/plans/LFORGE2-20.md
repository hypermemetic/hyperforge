# LFORGE2-20: Remote Import Commands

**Status**: Planning
**Blocked by**: LFORGE2-2, LFORGE2-3, LFORGE2-16 (forge API)
**Unlocks**: Migration from existing forges

---

## Goal

Import existing repositories from forges (GitHub, Codeberg, GitLab) into hyperforge management.

---

## Commands

### `hyperforge remote <forge> list`

List all repositories for an org on a forge.

```bash
hyperforge remote github list --org alice

# Output:
# alice/repo1 (public)
# alice/repo2 (private)
# alice/archive-old (public, archived)
```

**Implementation**:
- Request token: scope `git/github/alice/*/token` (or more specific)
- Call forge API: `GET /orgs/alice/repos` or `GET /users/alice/repos`
- Display repo names, visibility, archived status

### `hyperforge remote <forge> pull`

Import repos from forge into hyperforge management.

```bash
# Import specific repos
hyperforge remote github pull \
  --org alice \
  --repo "^alice-tool-.*" \
  --repo "alice-lib" \
  --exclude ".*-archive$" \
  --dry-run

# Import all repos
hyperforge remote github pull \
  --org alice \
  --all

# Import to specific directory
hyperforge remote github pull \
  --org alice \
  --repo "alice-tool" \
  --target ~/projects/tools
```

**What it does**:
1. List repos on forge matching criteria
2. For each repo:
   - Clone to local directory (or use existing if present)
   - Create `.hyperforge/config.toml` in repo
   - Configure git remotes based on workspace defaults
   - Set git config for SSH keys
3. Show summary of imported repos

---

## Directory-Level Defaults

When importing, check for workspace-level defaults:

```
~/projects/alice/
  ├── .hyperforge/
  │   └── defaults.toml    # Default config for all repos in this workspace
  ├── repo1/
  │   └── .hyperforge/
  │       └── config.toml  # Inherits from ../defaults.toml
  └── repo2/
      └── .hyperforge/
          └── config.toml
```

**defaults.toml**:
```toml
# Defaults for all repos in this workspace
forges = ["github", "codeberg"]
org = "alice"

[ssh]
github = "~/.ssh/alice_github"
codeberg = "~/.ssh/alice_codeberg"

[defaults]
visibility = "public"
```

**Inheritance**:
- Repo `config.toml` inherits from parent `defaults.toml`
- Repo can override any field
- Multiple levels: `/projects/.hyperforge/defaults.toml` → `/projects/alice/.hyperforge/defaults.toml` → `repo/.hyperforge/config.toml`

---

## Implementation Details

### 1. List Command

```rust
// src/remote/list.rs

pub async fn list_repos(
    forge: &Forge,
    org: &str,
    auth: &dyn AuthProvider,
) -> Result<Vec<RemoteRepo>> {
    // 1. Request token
    let scope = format!("git/{}/{}/*/token", forge, org);
    let token = auth.get_secret_lazy(&scope).await?;

    // 2. Call forge API
    let repos = match forge {
        Forge::GitHub => {
            github_api::list_repos(org, &token).await?
        }
        Forge::Codeberg => {
            codeberg_api::list_repos(org, &token).await?
        }
        _ => todo!()
    };

    Ok(repos)
}

pub struct RemoteRepo {
    pub name: String,
    pub full_name: String,  // "alice/repo1"
    pub visibility: Visibility,
    pub archived: bool,
    pub clone_url: String,
}
```

### 2. Pull Command

```rust
// src/remote/pull.rs

pub struct PullOptions {
    pub org: String,
    pub forge: Forge,
    pub include_patterns: Vec<Regex>,
    pub exclude_patterns: Vec<Regex>,
    pub target_dir: Option<PathBuf>,
    pub dry_run: bool,
}

pub async fn pull_repos(
    options: PullOptions,
    auth: &dyn AuthProvider,
) -> Result<PullResult> {
    // 1. List all repos on forge
    let all_repos = list_repos(&options.forge, &options.org, auth).await?;

    // 2. Filter by patterns
    let filtered = filter_repos(all_repos, &options)?;

    if options.dry_run {
        return Ok(PullResult::DryRun(filtered));
    }

    // 3. Load workspace defaults (if they exist)
    let defaults = load_workspace_defaults(&options.target_dir)?;

    // 4. For each repo
    let mut results = Vec::new();
    for repo in filtered {
        let result = import_repo(&repo, &defaults, &options, auth).await;
        results.push(result);
    }

    Ok(PullResult::Imported(results))
}

async fn import_repo(
    repo: &RemoteRepo,
    defaults: &Option<WorkspaceDefaults>,
    options: &PullOptions,
    auth: &dyn AuthProvider,
) -> Result<ImportedRepo> {
    let target = options.target_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    let repo_path = target.join(&repo.name);

    // 1. Clone repo if doesn't exist
    if !repo_path.exists() {
        git::clone(&repo.clone_url, &repo_path).await?;
    }

    // 2. Create .hyperforge/config.toml
    let config = build_config_from_defaults(repo, defaults)?;
    config.save(&repo_path.join(".hyperforge/config.toml"))?;

    // 3. Sync git remotes to match config
    git::sync_remotes(&repo_path, &config).await?;

    // 4. Set git config for SSH
    git::configure_ssh(&repo_path, &config).await?;

    Ok(ImportedRepo {
        name: repo.name.clone(),
        path: repo_path,
    })
}
```

### 3. Workspace Defaults Discovery

```rust
// src/config/defaults.rs

pub fn load_workspace_defaults(path: &Path) -> Result<Option<WorkspaceDefaults>> {
    // Walk up directory tree looking for .hyperforge/defaults.toml
    let mut current = path.to_path_buf();

    loop {
        let defaults_path = current.join(".hyperforge/defaults.toml");

        if defaults_path.exists() {
            let content = fs::read_to_string(&defaults_path)?;
            let defaults: WorkspaceDefaults = toml::from_str(&content)?;
            return Ok(Some(defaults));
        }

        if !current.pop() {
            break;  // Reached root
        }
    }

    Ok(None)  // No defaults found
}

pub fn build_config_from_defaults(
    repo: &RemoteRepo,
    defaults: &Option<WorkspaceDefaults>,
) -> Result<HyperforgeConfig> {
    let mut config = HyperforgeConfig::default();

    // Apply defaults if present
    if let Some(d) = defaults {
        config.forges = d.forges.clone();
        config.org = Some(d.org.clone());
        config.ssh = d.ssh.clone();
        config.visibility = d.defaults.visibility.clone();
    }

    // Set repo-specific fields
    config.repo_name = repo.name.clone();
    config.visibility = repo.visibility.clone();

    Ok(config)
}
```

### 4. Pattern Matching

```rust
fn filter_repos(
    repos: Vec<RemoteRepo>,
    options: &PullOptions,
) -> Result<Vec<RemoteRepo>> {
    repos.into_iter()
        .filter(|repo| {
            // Must match at least one include pattern
            let included = options.include_patterns.is_empty()
                || options.include_patterns.iter().any(|p| p.is_match(&repo.name));

            // Must not match any exclude pattern
            let excluded = options.exclude_patterns.iter().any(|p| p.is_match(&repo.name));

            included && !excluded
        })
        .collect()
}
```

---

## Usage Examples

### Example 1: Import all repos for an org

```bash
cd ~/projects

# Create workspace defaults
mkdir -p .hyperforge
cat > .hyperforge/defaults.toml <<EOF
forges = ["github", "codeberg"]
org = "alice"

[ssh]
github = "~/.ssh/alice_github"
codeberg = "~/.ssh/alice_codeberg"
EOF

# Import all repos from GitHub
hyperforge remote github pull --org alice --all

# Result:
# ~/projects/
#   ├── .hyperforge/defaults.toml
#   ├── alice-tool-1/.hyperforge/config.toml
#   ├── alice-tool-2/.hyperforge/config.toml
#   └── alice-lib/.hyperforge/config.toml
```

### Example 2: Import specific repos with patterns

```bash
# Import only tools, exclude archives
hyperforge remote github pull \
  --org alice \
  --repo "^tool-.*" \
  --exclude ".*-archive$" \
  --dry-run

# Dry run output:
# Would import:
#   - tool-cli
#   - tool-server
# Would skip:
#   - lib-core (doesn't match pattern)
#   - tool-old-archive (excluded)

# Actually import
hyperforge remote github pull \
  --org alice \
  --repo "^tool-.*" \
  --exclude ".*-archive$"
```

### Example 3: Import from multiple forges

```bash
# Import from GitHub
hyperforge remote github pull --org alice --all

# Import from Codeberg (merges with existing)
hyperforge remote codeberg pull --org alice --all

# Result: repos have both github and codeberg remotes
```

### Example 4: Import without defaults (prompted to create)

```bash
cd ~/new-workspace
hyperforge remote github pull --org bob --all

# Output:
# ✗ No workspace defaults found
#
# Create .hyperforge/defaults.toml? [Y/n]: y
#
# Enter default forges (comma-separated): github,codeberg
# Enter SSH key for github: ~/.ssh/bob_github
# Enter SSH key for codeberg: ~/.ssh/bob_codeberg
#
# Created .hyperforge/defaults.toml
#
# Importing repos...
```

---

## Acceptance Criteria

- ✅ `hyperforge remote github list --org alice` shows all repos
- ✅ `hyperforge remote github pull --org alice --all` imports all repos
- ✅ Pattern matching works (--repo, --exclude)
- ✅ Dry run shows what would be imported
- ✅ Workspace defaults are discovered and applied
- ✅ Prompts to create defaults if not found
- ✅ Config inheritance: workspace defaults → repo config
- ✅ Git remotes synced to match config
- ✅ SSH keys configured via git config

---

## Testing

### Unit Tests
- Pattern matching logic
- Config inheritance
- Defaults discovery (walk up tree)

### Integration Tests
- Mock forge API
- Import repos
- Verify .hyperforge/config.toml created
- Verify git remotes configured

### Manual Tests
- Real GitHub/Codeberg import
- Verify workspace defaults applied
- Verify SSH keys work

---

## Next Steps

This enables migration from existing forges:
1. Create workspace with defaults
2. Import repos from GitHub
3. Repos now managed by hyperforge
4. Can push to multiple forges via `workspace push`
