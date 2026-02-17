# Workspace Sync Guide

How to push everything in a workspace to remote forges, create repos that don't exist, and ensure local state is fully synced.

## TL;DR

```bash
# Preview what would happen
synapse substrate hyperforge workspace sync \
  --path /path/to/workspace --org myorg --dry_run true

# Do it for real
synapse substrate hyperforge workspace sync \
  --path /path/to/workspace --org myorg
```

`workspace sync` is a single command that discovers repos, registers them, creates missing repos on forges, updates metadata, and pushes git content.

## The 8-Phase Pipeline

`workspace sync` runs 8 phases in order:

### Phase 1: Discover

Scans the workspace directory for repositories. Classifies each subdirectory:

| Type | Has `.git`? | Has `.hyperforge/config.toml`? | Action |
|------|-------------|-------------------------------|--------|
| Configured | Yes | Yes | Included as-is |
| Unconfigured | Yes | No | Initialized in Phase 2 |
| Skipped | No | No | Reported, skipped |

### Phase 2: Init Unconfigured

Creates `.hyperforge/config.toml` and adds git remotes for repos that have `.git` but no hyperforge config.

- If all discovered repos share one org, it's inferred automatically
- If multiple orgs exist, you must pass `--org`
- Forges are inferred from existing configured repos, or pass `--forges`

### Phase 3: Re-discover

Rescans the workspace to pick up repos that were just initialized in Phase 2. Only runs if any inits happened.

### Phase 4: Register in LocalForge

Adds every discovered repo to LocalForge (`~/.config/hyperforge/orgs/{org}/repos.yaml`). This is the local registry that drives sync.

- Reads each repo's `.hyperforge/config.toml` to determine origin, mirrors, visibility
- First forge listed = origin, rest = mirrors
- Idempotent: skips repos already registered

### Phase 5: Import Remote-Only

Queries each configured forge for repos that exist remotely but aren't in LocalForge. Imports them so the diff in Phase 6 is complete.

### Phase 6: Diff

Computes what needs to change on each forge:

| Operation | Meaning |
|-----------|---------|
| **Create** | In LocalForge but not on remote |
| **Update** | On both, but description or visibility differs |
| **Delete** | On remote but not in LocalForge (skipped by sync) |
| **InSync** | Matches on both sides |

### Phase 7: Apply Creates & Updates

Calls forge APIs to create or update repos:

- **Create**: `POST /orgs/{org}/repos` on GitHub/Codeberg/GitLab
- **Update**: `PATCH` endpoint to update description/visibility
- **Delete**: Explicitly skipped. Sync never deletes remote repos.

### Phase 8: Push

Runs `git push` to all configured remotes for every repo in the workspace. Skip with `--no_push`.

## Dry Run

Always preview first:

```bash
synapse substrate hyperforge workspace sync \
  --path /path/to/workspace --org myorg --dry_run true
```

Dry run populates the in-memory LocalForge so the diff output is accurate, but writes nothing to disk and makes no API calls.

## Diff Only

To see what would change without any side effects:

```bash
synapse substrate hyperforge workspace diff \
  --path /path/to/workspace --org myorg
```

Returns counts (`to_create`, `to_update`, `to_delete`, `in_sync`) and individual operations per repo.

## What Sync Compares

The diff considers only two fields:

| Field | Compared? |
|-------|-----------|
| `description` | Yes |
| `visibility` | Yes |
| `origin` | No |
| `mirrors` | No |
| `protected` | No |

If you change a repo's origin or mirrors locally, sync won't detect a difference. Those fields control *where* sync pushes, not *what* it diffs.

## Safety Guarantees

- **No deletes**: Sync skips `SyncOp::Delete`. Repos on remotes that aren't in LocalForge are left alone.
- **Idempotent**: Running sync twice produces the same result.
- **Crash-safe**: Phases 1-6 are read-only. Phase 7 (API calls) and Phase 8 (git push) are the only phases with side effects.

## Common Workflows

### Fresh workspace, no repos on forges yet

```bash
# 1. Each repo needs at least a .git directory
# 2. Run sync — it handles everything else
synapse substrate hyperforge workspace sync \
  --path ~/code/myorg --org myorg --forges "github,codeberg"
```

This will: init configs, register in LocalForge, create repos on both forges, push all branches.

### Repos exist on GitHub, mirror to Codeberg

```bash
# 1. Import from GitHub
synapse substrate hyperforge repo import --forge github --org myorg

# 2. Sync to Codeberg (creates missing repos there)
synapse substrate hyperforge workspace sync --org myorg
```

### Check sync state across all forges

```bash
synapse substrate hyperforge workspace diff --org myorg --forge github
synapse substrate hyperforge workspace diff --org myorg --forge codeberg
```

### Push git content only (no repo creation)

```bash
synapse substrate hyperforge workspace sync \
  --path ~/code/myorg --org myorg --no_push false

# Or for a single repo:
synapse substrate hyperforge repo push --path ~/code/myorg/my-repo
```

## Limitations

- **Repos must have `.git`**: Bare directories without git init are skipped entirely.
- **No bulk `repos_create`**: If you want to register repos in LocalForge without the full sync pipeline, you must call `repo create` for each one individually.
- **Mirrors are push-only**: Sync creates and updates repos on mirrors, but doesn't pull content from them.
