# LFORGE2 Design Clarifications

This document captures critical design decisions and clarifications.

---

## 1. No SSH Host Aliases

**Old approach** (deprecated):
```bash
# ~/.ssh/config
Host github-alice
    HostName github.com
    IdentityFile ~/.ssh/alice_key

# Git remote
git@github-alice:alice/repo.git
```

**New approach**:
Use git's native `core.sshCommand` config per repo:

```bash
# Set per-repo SSH key
git config core.sshCommand "ssh -i ~/.ssh/alice_github"

# Git remote (standard format)
git@github.com:alice/repo.git
```

**Why**:
- Standard git remote URLs (portable, works everywhere)
- Per-repo SSH key configuration via git config
- No global SSH config pollution
- Works in CI/CD without SSH config setup

**Implementation**: LFORGE2-3, LFORGE2-4

---

## 2. Secret Path Structure (Granular Scopes)

Secrets use fully-qualified paths:

```
git/{forge}/{org}/{repo}/{repo_name}/token

Examples:
  git/github/alice/repo/my-tool/token
  git/codeberg/acme-corp/repo/web-app/token
  git/gitlab/bob/repo/lib-core/token

cargo/{user}/{package}/token
npm/@{org}/{package}/token
pypi/{user}/{package}/token
```

**Why granular**:
- Can't predict which repos need which tokens at startup
- Request scopes lazily as operations require them
- Each scope maps to single repo/package operation
- Future: can grant per-repo permissions
- Even though tokens may be same underlying value in vault, scope names are precise

**Org/Repo Extraction**:
From `git@github.com:alice/repo/my-tool.git`:
- Extract everything after `:` → `alice/repo/my-tool`
- Parse: `alice` (org), `repo/my-tool` (repo path)
- Or from URL: `https://github.com/alice/my-tool.git` → `alice`, `my-tool`

**Implementation**: PKG-9, LFORGE2-17

---

## 3. Hyperforge Config is Ground Truth

`.hyperforge/config.toml` is authoritative. Git config follows.

**Reconciliation**:
```bash
# Config says:
forges = ["github", "codeberg"]

# Git has:
git remote -v
# origin   git@github.com:...
# gitlab   git@gitlab.com:...

# hyperforge sync --path . will:
# 1. Warn: gitlab remote not in config, will be removed
# 2. Add codeberg remote (missing from git)
# 3. Remove gitlab remote (not in config)
# 4. Require --force to proceed if removing remotes
```

**Import from Git**:
```bash
# Import existing git config into hyperforge
hyperforge init --path . --import-from-git

# Detects existing remotes, builds config from them
```

**Implementation**: LFORGE2-5 (sync command)

---

## 4. Remote URL Construction

Build git remote URLs from `.hyperforge/config.toml`:

```toml
forges = ["github", "codeberg"]
org = "alice"
repo_name = "my-tool"

[ssh]
github = "~/.ssh/alice_github"
codeberg = "~/.ssh/alice_codeberg"
```

**Constructed remotes**:
```bash
# GitHub
git remote add github git@github.com:alice/my-tool.git
git config core.sshCommand "ssh -i ~/.ssh/alice_github"

# Codeberg
git remote add codeberg git@codeberg.org:alice/my-tool.git
git config core.sshCommand "ssh -i ~/.ssh/alice_codeberg"
```

**Forge URL patterns**:
- GitHub: `git@github.com:{org}/{repo}.git`
- Codeberg: `git@codeberg.org:{org}/{repo}.git`
- GitLab: `git@gitlab.com:{org}/{repo}.git`

**Implementation**: LFORGE2-4 (init command)

---

## 5. Lazy Scope Requests

Scopes requested on-demand, not upfront:

```rust
// When hyperforge needs a token:
async fn push_to_github(&self, repo: &str) -> Result<()> {
    // Build scope path
    let scope = format!("git/github/alice/repo/{}/token", repo);

    // This triggers auth request if scope not granted
    let token = self.auth.get_secret(&scope).await?;

    // Use token
    git::push_with_token(repo, &token).await
}
```

**Flow**:
1. Hyperforge requests `git/github/alice/repo/my-tool/token`
2. Auth provider checks if scope granted
3. If not: request approval (queue, timeout)
4. User approves in WorkOS UI or CLI
5. Token returned to hyperforge
6. Cached for session

**User can approve async**:
- Request goes into approval queue
- Hyperforge times out after 30s
- User approves in separate UI
- Next time hyperforge runs, scope is granted

**Implementation**: LFORGE2-17, PKG-9

---

## 6. Publishing: Build First, Then Publish

Validate packages build locally before publishing:

```rust
async fn publish(&self, path: &Path, bump: VersionBump) -> Result<()> {
    // 1. Bump version
    let new_version = self.bump_version(path, bump).await?;

    // 2. Build locally (cargo build, npm run build, etc.)
    self.build_locally(path).await?;

    // 3. Run tests
    self.run_tests(path).await?;

    // 4. If build/test fails, rollback version bump
    if build_failed || test_failed {
        self.rollback_version(path).await?;
        return Err(anyhow!("Build/test failed, rolled back"));
    }

    // 5. Publish to registry
    self.publish_to_registry(path).await?;

    // 6. Commit version bump
    self.commit_version(path, new_version).await?;

    // 7. Tag release
    self.tag_release(path, new_version).await?;

    Ok(())
}
```

**Why**:
- Catch build errors before publishing
- Don't publish broken packages
- Can't unpublish from most registries
- Local validation is fast

**Implementation**: PKG-10, PKG-11

---

## 7. Push: Stop on First Failure

When pushing to multiple forges, stop on first failure:

```rust
async fn push_to_all_forges(&self, path: &Path) -> Result<()> {
    let forges = self.config.forges()?;

    for forge in forges {
        // If this fails, don't try remaining forges
        self.push_to_forge(path, forge).await?;
    }

    Ok(())
}
```

**Why**:
- Can't easily resolve push failures automatically
- User needs to fix issue (auth, conflicts, etc.)
- Better to fail fast than continue with partial state
- User sees first error, fixes it, reruns

**Workspace push**:
Still stop on first repo failure (can't continue safely).

**Implementation**: LFORGE2-7 (push command)

---

## 8. Dry-Run for All Operations

Every command should support `--dry-run`:

```bash
# Preview without changes
hyperforge init --path . --forges github,codeberg --dry-run
hyperforge sync --path . --dry-run
hyperforge push --path . --dry-run
hyperforge publish --path . --bump patch --dry-run
hyperforge workspace push --path . --dry-run
```

**Dry-run behavior**:
- Show what would be done
- Don't modify files
- Don't call APIs
- Don't commit/push

**Implementation**: All command tickets

---

## 9. Workspace Defaults (Directory-Level Config)

`.hyperforge/defaults.toml` at any level provides defaults for child repos:

```
~/projects/
  ├── .hyperforge/
  │   └── defaults.toml       # Applies to all repos in ~/projects/
  ├── alice/
  │   ├── .hyperforge/
  │   │   └── defaults.toml   # Overrides parent, applies to alice repos
  │   ├── tool1/.hyperforge/config.toml  # Inherits from ../defaults.toml
  │   └── tool2/.hyperforge/config.toml
  └── bob/
      └── app/.hyperforge/config.toml    # Inherits from ~/projects/defaults.toml
```

**Inheritance chain**:
1. Load all `defaults.toml` from repo up to root
2. Apply in order: root → ... → parent
3. Repo `config.toml` overrides all defaults

**Example**:
```toml
# ~/projects/.hyperforge/defaults.toml
forges = ["github"]
visibility = "public"

# ~/projects/alice/.hyperforge/defaults.toml
org = "alice"
forges = ["github", "codeberg"]  # Overrides parent

# ~/projects/alice/tool1/.hyperforge/config.toml
repo_name = "tool1"
visibility = "private"  # Overrides defaults

# Effective config for tool1:
# org = "alice"
# repo_name = "tool1"
# forges = ["github", "codeberg"]
# visibility = "private"
```

**When importing**:
```bash
hyperforge remote github pull --org alice

# If no defaults.toml found, prompt:
# "No workspace defaults found. Create .hyperforge/defaults.toml? [Y/n]"
```

**Implementation**: LFORGE2-20 (remote import), LFORGE2-2 (config loading)

---

## 10. Migration Path from Old Hyperforge

**No automatic migration**. Fresh start approach:

**Step 1: Import from forges**
```bash
cd ~/projects/alice
hyperforge remote github pull --org alice --all
```

**Step 2: Workspace operations now work**
```bash
hyperforge workspace push --path .
hyperforge workspace publish --path . --bump patch
```

**Old hyperforge**:
- Keep as-is
- Users migrate repos one-by-one
- Or use `remote pull` to bulk import

**Implementation**: LFORGE2-20

---

## 11. WorkOS Vault Integration

**WorkOS provides**:
- Identity/authentication
- Scope management (permissions)
- Master token issuance
- Vault for secret storage

**Flow**:
```
1. User logs in → WorkOS returns master token
2. Hyperforge starts with master token
3. Needs git/github/alice/repo/tool/token
4. Requests scope from WorkOS (master token + scope)
5. WorkOS checks permissions
6. If granted: returns access token
7. Hyperforge uses access token to get secret from Vault
8. Vault returns actual GitHub token
9. Hyperforge pushes with token
```

**Multi-provider support**:
```toml
# Can route different scopes to different providers
[auth]
default = "workos"

[auth.providers.workos]
type = "workos"
vault_url = "https://vault.workos.com"

[auth.providers.local]
type = "keychain"

[auth.routing]
"git/github/acme-corp/*" = "workos"  # Company secrets
"git/github/alice/*" = "local"       # Personal secrets
```

**Implementation**: LFORGE2-17, PKG-9

---

## Summary of Key Changes to Plans

1. ✅ **LFORGE2-0**: New ticket for worktree setup
2. ✅ **LFORGE2-20**: New ticket for remote import
3. 🔄 **LFORGE2-2**: Update config schema with inheritance
4. 🔄 **LFORGE2-3**: Update git integration with core.sshCommand
5. 🔄 **LFORGE2-4**: Update remote construction logic
6. 🔄 **LFORGE2-5**: Update sync semantics (config is ground truth)
7. 🔄 **PKG-9**: Update auth with granular scopes
8. 🔄 **All**: Add --dry-run support

Next: Update existing tickets with these clarifications.
