# Hyperforge Command Structure Review

**Date**: 2026-01-18
**Status**: Draft

## Current Command Hierarchy

```
hyperforge
├── org
│   ├── list                    # List all orgs
│   ├── import                  # Import repos from forges into config
│   └── <org_name>
│       ├── info                # Show org details
│       ├── repos
│       │   ├── list            # List repos in config
│       │   ├── create          # Stage a new repo
│       │   ├── update          # Update repo config
│       │   ├── remove          # Mark repo for deletion
│       │   ├── diff            # Show desired vs synced state
│       │   ├── sync            # Sync to forges (--yes to apply)
│       │   ├── add_forge       # Bulk add forge to all repos
│       │   ├── set_default_forges  # Set org default forges
│       │   ├── enforce_ssh     # Enforce SSH config on local repos
│       │   └── <repo_name>
│       │       └── show        # Show repo details
│       └── secrets
│           └── ...
└── workspace
    ├── list                    # List workspace bindings
    ├── show                    # Show workspace resolution
    ├── bind                    # Bind directory to org
    ├── unbind                  # Remove binding
    ├── diff                    # Preview changes for workspace
    ├── sync                    # Sync workspace to forges
    ├── import                  # Import from forges
    ├── clone_all               # Clone all repos
    ├── discover_uninitialized  # Find local repos not in config
    ├── create_uninitialized    # Stage uninitialized repos
    └── list_forge_repos        # Query repos on forges
```

## High-Level Workflows

### Workflow 1: Initial Setup

```bash
# 1. Bind workspace directory to an org
hyperforge workspace bind --path ~/dev/myorg --org_name myorg

# 2. Import existing repos from forges
hyperforge org import --org_name myorg --include_private true

# 3. Clone all repos locally
hyperforge workspace clone_all --path ~/dev/myorg
```

### Workflow 2: Add New Local Repo to Forges

```bash
# 1. Create git repo locally
cd ~/dev/myorg && mkdir new-project && cd new-project && git init

# 2. Discover it
hyperforge workspace discover_uninitialized --path ~/dev/myorg
# Shows: new-project

# 3. Stage it (dry-run)
hyperforge workspace create_uninitialized --path ~/dev/myorg
# Shows what would be staged

# 4. Stage it (apply)
hyperforge workspace create_uninitialized --path ~/dev/myorg --yes

# 5. Sync to forges (dry-run)
hyperforge workspace sync --path ~/dev/myorg
# Shows what would be created on GitHub/Codeberg

# 6. Sync to forges (apply)
hyperforge workspace sync --path ~/dev/myorg --yes
```

### Workflow 3: Check Forge State

```bash
# What's on the forges?
hyperforge workspace list_forge_repos --path ~/dev/myorg

# What's in config?
hyperforge org myorg repos list

# What's the diff?
hyperforge org myorg repos diff
```

### Workflow 4: Ensure SSH Config

```bash
# Check/enforce SSH config on all local repos
hyperforge org myorg repos enforce_ssh
```

### Workflow 5: Bulk Forge Management

```bash
# Add a forge to all repos
hyperforge org myorg repos add_forge --forge codeberg

# Set default forges for new repos
hyperforge org myorg repos set_default_forges --forges github,codeberg

# Sync changes
hyperforge org myorg repos sync --yes
```

---

## Idiosyncrasies Identified

### 1. Inconsistent Scoping (org vs workspace)

| Command | Level | But operates on... |
|---------|-------|-------------------|
| `repos sync` | org | workspace (looks up path from bindings) |
| `repos enforce_ssh` | org | workspace (looks up path from bindings) |
| `workspace sync` | workspace | requires `--path` |
| `create_uninitialized` | workspace | requires `--path` |

**Problem**: Some org commands implicitly operate on workspaces, while workspace commands require explicit paths. This is confusing.

### 2. Duplicate Functionality

- `org <org> repos sync` and `workspace sync --path` do similar things
- `org <org> repos diff` and `workspace diff --path` do similar things

**Problem**: Two ways to do the same thing with slightly different semantics.

### 3. Path vs Org Resolution

- Workspace commands: require `--path`, resolve org from bindings
- Org commands: require org name, sometimes look up workspace path

**Problem**: Mental model switches between "start from path" and "start from org".

### 4. Inconsistent --yes Semantics

| Command | Without --yes | With --yes |
|---------|---------------|------------|
| `sync` | dry-run preview | apply changes |
| `create_uninitialized` | dry-run preview | stage repos |
| `import` | N/A (always applies) | N/A |

**Problem**: `import` doesn't have dry-run mode.

### 5. discover vs create Split

```bash
# Two commands for related operations
hyperforge workspace discover_uninitialized --path ...
hyperforge workspace create_uninitialized --path ...
```

**Problem**: Could be one command with a flag.

### 6. Verb Inconsistency

- `create` - stages a repo (doesn't create on forge)
- `sync` - actually creates on forge
- `import` - discovers AND stages

**Problem**: "create" doesn't create, "sync" creates.

---

## Suggested Improvements

### Option A: Workspace-Centric Model

Make workspace the primary entry point, eliminate org-level workspace operations.

```
hyperforge workspace --path ~/dev/myorg
├── status              # Combined: local state, config state, forge state
├── init                # Stage uninitialized repos (--yes to apply)
├── sync                # Sync to forges (--yes to apply)
├── import              # Import from forges (--yes to apply)
├── clone               # Clone missing repos
├── enforce-ssh         # Enforce SSH config
└── repos
    ├── list            # List repos in config
    ├── add             # Stage new repo
    ├── remove          # Mark for deletion
    └── <name> show     # Show repo details

hyperforge org
├── list                # List orgs
└── <name> info         # Show org config
```

**Benefits**:
- Clear mental model: always start from workspace
- No duplicate commands
- Path is always explicit

### Option B: Smart Context Detection

Use current directory to determine context, like git.

```bash
cd ~/dev/myorg/some-repo

# These auto-detect workspace and org from cwd
hyperforge status          # Show workspace status
hyperforge sync --yes      # Sync workspace
hyperforge init --yes      # Stage uninitialized repos
```

**Benefits**:
- No `--path` needed when inside workspace
- Feels like git (context-aware)

### Option C: Unified Verbs

Standardize on consistent verbs:

| Action | Current | Proposed |
|--------|---------|----------|
| Add to config | `create`, `import` | `stage` |
| Push to forge | `sync` | `push` |
| Pull from forge | `import` | `pull` |
| Preview | (no flag) | `--dry-run` |
| Apply | `--yes` | (default) or `--apply` |

```bash
hyperforge workspace stage --path ~/dev/myorg      # Stage uninitialized
hyperforge workspace push --path ~/dev/myorg       # Push to forges
hyperforge workspace pull --path ~/dev/myorg       # Pull from forges
```

### Option D: Combine discover + create

```bash
# Current (two commands)
hyperforge workspace discover_uninitialized --path ...
hyperforge workspace create_uninitialized --path ...

# Proposed (one command)
hyperforge workspace init --path ~/dev/myorg           # dry-run
hyperforge workspace init --path ~/dev/myorg --yes     # apply
```

---

## Recommended Changes (Minimal)

1. **Rename `create_uninitialized` to `init`** - shorter, clearer intent

2. **Add `--dry-run` to `import`** - consistency with other commands

3. **Deprecate org-level sync in favor of workspace sync** - reduce confusion

4. **Add cwd detection** - if inside a bound workspace, `--path` becomes optional

5. **Consolidate discover + init** - `init` shows what would be staged (dry-run by default)

---

## Command Comparison: Current vs Proposed

### Current
```bash
hyperforge workspace discover_uninitialized --path ~/dev/myorg
hyperforge workspace create_uninitialized --path ~/dev/myorg --yes
hyperforge workspace sync --path ~/dev/myorg --yes
```

### Proposed
```bash
cd ~/dev/myorg
hyperforge init           # Shows uninitialized repos (dry-run)
hyperforge init --yes     # Stages them
hyperforge sync --yes     # Pushes to forges
```

Or with explicit path:
```bash
hyperforge init --path ~/dev/myorg --yes
hyperforge sync --path ~/dev/myorg --yes
```
