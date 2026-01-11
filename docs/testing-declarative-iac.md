# Testing Hyperforge Declarative IAC

This document describes how to test the declarative infrastructure-as-code features with the `hypermemetic` organization using both GitHub and Codeberg.

## Prerequisites

### 1. Ensure Tokens Are Set

Tokens are stored in macOS Keychain. Verify they exist:

```bash
# Check GitHub token
synapse plexus hyperforge org hypermemetic secrets list

# If missing, acquire from gh CLI
synapse plexus hyperforge org hypermemetic secrets acquire --forge github

# Or set manually
synapse plexus hyperforge org hypermemetic secrets set --key github-token --value "ghp_xxx"
synapse plexus hyperforge org hypermemetic secrets set --key codeberg-token --value "xxx"
```

### 2. Verify Org Configuration

```bash
# Check org exists and has both forges configured
synapse plexus hyperforge org show --org-name hypermemetic
```

Expected output should show:
- `forges: [github, codeberg]`
- `owner: hypermemetic`

### 3. Check Current State

```bash
# List current repos in local config
synapse plexus hyperforge org hypermemetic repos list

# List staged changes (if any)
synapse plexus hyperforge org hypermemetic repos list --staged true
```

---

## Test 1: repos.refresh()

**Purpose**: Query GitHub and Codeberg APIs to discover what repos exist remotely.

```bash
synapse plexus hyperforge org hypermemetic repos refresh
```

**Expected Events**:
```
RefreshStarted: org_name=hypermemetic, forges=[github, codeberg]
RefreshProgress: forge=github, repos_found=N
RefreshProgress: forge=codeberg, repos_found=M
RefreshComplete: discovered=X, matched=Y, untracked=Z
```

**What to verify**:
- `repos_found` counts match actual repos on each forge
- `matched` = repos in local config that exist on forges
- `untracked` = repos on forges not in local config

---

## Test 2: repos.diff()

**Purpose**: Compare local desired state vs synced state without calling Pulumi.

```bash
synapse plexus hyperforge org hypermemetic repos diff
```

**Expected Events**:
```
RepoDiff: repo=substrate, status=in_sync, details=[...]
RepoDiff: repo=synapse, status=in_sync, details=[...]
...
DiffSummary: to_create=0, to_update=0, to_delete=0, in_sync=N, untracked=0
```

**Status meanings**:
- `in_sync` - Repo is synced on all desired forges
- `to_create` - Repo in local config but not yet on forges
- `to_update` - Repo exists but forge list differs
- `to_delete` - Repo marked with `_delete: true`
- `untracked` - Repo on forges but not in local config

---

## Test 3: org.import() (Dry Run)

**Purpose**: See what would be imported from forges without writing config.

```bash
synapse plexus hyperforge org import --org-name hypermemetic --dry-run true
```

**Expected Events**:
```
ImportStarted: org_name=hypermemetic, forges=[github, codeberg]
RepoImported: repo_name=substrate, forges=[github, codeberg], visibility=public
RepoImported: repo_name=synapse, forges=[github, codeberg], visibility=public
...
ImportComplete: imported_count=N, skipped_count=M
```

**What to verify**:
- `skipped_count` should equal repos already in local config
- `imported_count` should equal new repos discovered
- Private repos excluded unless `--include-private true`

---

## Test 4: repos.converge() (Dry Run)

**Purpose**: Preview the full convergence process without making changes.

```bash
synapse plexus hyperforge org hypermemetic repos converge --dry-run true
```

**Expected Events**:
```
ConvergeStarted: phases=[refresh, diff, apply, capture, verify]
ConvergePhase: phase=refresh, status="discovered N repos"
ConvergePhase: phase=diff, status="create=0, update=0, delete=0"
ConvergePhase: phase=apply, status="skipped (dry run)"
ConvergeComplete: success=true, changes_applied=0, converged=true
```

**If already converged**: `changes_applied=0, converged=true`
**If changes needed**: Shows what would be created/updated/deleted

---

## Test 5: Create New Repo and Sync

**Purpose**: Test the full create → sync → capture flow.

### Step 1: Stage a new repo

```bash
synapse plexus hyperforge org hypermemetic repos create \
  --repo-name test-declarative-iac \
  --description "Testing declarative IAC" \
  --visibility public
```

**Expected**: `Staged: repo_name=test-declarative-iac`

### Step 2: Check diff

```bash
synapse plexus hyperforge org hypermemetic repos diff
```

**Expected**:
```
RepoDiff: repo=test-declarative-iac, status=to_create, details=["Will create on: [github, codeberg]"]
```

### Step 3: Preview sync

```bash
synapse plexus hyperforge org hypermemetic repos sync --dry-run true
```

### Step 4: Apply sync

```bash
synapse plexus hyperforge org hypermemetic repos sync --yes true
```

**Expected Events**:
```
SyncStarted: repo_count=N
SyncProgress: repo_name=test-declarative-iac, stage=pulumi
OutputsCaptured: repo=test-declarative-iac, forge=github, url=https://github.com/...
OutputsCaptured: repo=test-declarative-iac, forge=codeberg, url=https://codeberg.org/...
SyncComplete: success=true, synced_count=1
```

### Step 5: Verify convergence

```bash
synapse plexus hyperforge org hypermemetic repos diff
```

**Expected**: `test-declarative-iac` now shows `status=in_sync`

### Step 6: Check repos.yaml has _synced

```bash
cat ~/.config/hyperforge/orgs/hypermemetic/repos.yaml | grep -A 10 test-declarative-iac
```

**Expected**:
```yaml
test-declarative-iac:
  description: "Testing declarative IAC"
  visibility: public
  forges: [github, codeberg]
  _synced:
    github:
      url: "https://github.com/hypermemetic/test-declarative-iac"
      synced_at: "2025-01-08T..."
    codeberg:
      url: "https://codeberg.org/hypermemetic/test-declarative-iac"
      synced_at: "2025-01-08T..."
```

---

## Test 6: Full Converge Workflow

**Purpose**: Test the complete bidirectional sync.

```bash
synapse plexus hyperforge org hypermemetic repos converge --yes true
```

**Expected Phases**:
1. **REFRESH** - Queries GitHub and Codeberg APIs
2. **DIFF** - Compares local vs remote state
3. **APPLY** - Runs Pulumi to create/update/delete repos
4. **CAPTURE** - Saves URLs and IDs to `_synced`
5. **VERIFY** - Re-diffs to confirm convergence

**Success Criteria**:
```
ConvergeComplete: success=true, converged=true, drift_detected=false
```

---

## Test 7: Protection Check

**Purpose**: Verify protected repos can't be deleted without `--force`.

### Step 1: Set a repo as protected

Edit `~/.config/hyperforge/orgs/hypermemetic/repos.yaml`:
```yaml
substrate:
  protected: true
  # ... rest of config
```

### Step 2: Try to delete (should fail)

```bash
synapse plexus hyperforge org hypermemetic repos remove --repo-name substrate
```

**Expected**: `ProtectionError: "Repository is protected. Use --force true to delete."`

### Step 3: Force delete (should succeed)

```bash
synapse plexus hyperforge org hypermemetic repos remove --repo-name substrate --force true
```

**Expected**: `MarkedForDeletion: repo_name=substrate`

(Don't actually sync this - reset the staged file)

---

## Test 8: Single Repo Sync

**Purpose**: Verify `--repo-name` filter works.

```bash
# Sync only substrate
synapse plexus hyperforge org hypermemetic repos sync \
  --repo-name substrate \
  --dry-run true
```

**Expected**: Only `substrate` appears in progress events, `repo_count=1`

---

## Test 9: Import from Scratch

**Purpose**: Test initializing a new org from existing forges.

### Step 1: Create test org

```bash
synapse plexus hyperforge org create \
  --org-name test-import \
  --owner hypermemetic \
  --ssh-key id_ed25519 \
  --origin github \
  --forges "github,codeberg"
```

### Step 2: Set tokens (or copy from hypermemetic)

```bash
synapse plexus hyperforge org test-import secrets acquire --forge github
```

### Step 3: Import existing repos

```bash
synapse plexus hyperforge org import --org-name test-import
```

### Step 4: Verify imported state

```bash
synapse plexus hyperforge org test-import repos list
cat ~/.config/hyperforge/orgs/test-import/repos.yaml
```

### Step 5: Confirm convergence

```bash
synapse plexus hyperforge org test-import repos converge --dry-run true
```

**Expected**: `changes_applied=0, converged=true` (imported state matches remote)

---

## Cleanup

After testing, remove test artifacts:

```bash
# Remove test repo from staging (if staged)
rm ~/.config/hyperforge/orgs/hypermemetic/staged-repos.yaml

# Remove test org
synapse plexus hyperforge org remove --org-name test-import

# Delete test repo on forges (if created)
gh repo delete hypermemetic/test-declarative-iac --yes
# (manually delete on Codeberg)
```

---

## Troubleshooting

### "No token for github/codeberg"
```bash
synapse plexus hyperforge org hypermemetic secrets acquire --forge github
```

### "Pulumi stack not found"
The sync will auto-create the stack. If issues persist:
```bash
cd ~/.hypermemetic-infra/projects/forge-pulumi
pulumi stack init hypermemetic
```

### "Organization not found"
Check org exists:
```bash
synapse plexus hyperforge org list
synapse plexus hyperforge org show --org-name hypermemetic
```

### Viewing Raw Pulumi Output
For debugging, check Pulumi directly:
```bash
cd ~/.hypermemetic-infra/projects/forge-pulumi
HYPERFORGE_ORG=hypermemetic pulumi preview
```

---

## Success Criteria Summary

The declarative IAC implementation is working correctly when:

1. `org.import` creates local config from existing forge repos
2. `repos.refresh` queries forges and reports discovery stats
3. `repos.diff` correctly identifies create/update/delete/in_sync status
4. `repos.sync` creates repos on both GitHub and Codeberg
5. `_synced` state is captured with URLs after apply
6. `repos.converge --dry-run` shows "0 changes" when converged
7. Re-running converge is idempotent (no unnecessary changes)
8. Protected repos require `--force` to delete
9. Single repo sync filter works with `--repo-name`
