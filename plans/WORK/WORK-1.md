# WORK-1: Named Workspaces — Multi-Workspace Orchestration

## Goal

Make workspaces first-class named entities so every workspace command operates across all workspaces by default, with filtering by workspace name and repo name. Eliminate `--path` as the primary workspace identifier.

## Context

Today every workspace command requires `--path`:
```bash
synapse lforge hyperforge workspace sync --path /Users/shmendez/dev/controlflow/hypermemetic
synapse lforge hyperforge build dirty --path /Users/shmendez/dev/controlflow/hypermemetic
```

This means:
- You can only operate on one workspace at a time
- Paths are long, error-prone, not portable
- No way to say "sync everything" or "what's dirty across all my workspaces"
- The workspace concept is implicit (it's just a directory)

## Design

### Workspace Registry

A new config file `~/.config/hyperforge/workspaces.toml`:

```toml
[hypermemetic]
path = "/Users/shmendez/dev/controlflow/hypermemetic"
org = "hypermemetic"
forges = ["github", "codeberg"]

[onebigmediaco]
path = "/Users/shmendez/dev/hyperforge/workspaces/sshmendez/orgs/OneBigMediaCo"
org = "OneBigMediaCo"
forges = ["github"]

[juggernautlabs]
path = "/Users/shmendez/dev/controlflow/juggernautlabs"
org = "juggernautlabs"
forges = ["github"]
```

Each workspace has a **name** (the TOML key), a **path**, an **org**, and **forges**. The org+forges link back to OrgConfig for SSH keys and tokens.

### Command Changes

**All workspace/build commands gain:**
- `--workspace` / `-w` — filter to a specific workspace by name (optional)
- `--path` becomes optional — if omitted, operates on all registered workspaces
- `--include` / `--exclude` — filter repos across workspaces (already exists)

**New commands:**
- `workspace register --name x --path /path --org y` — register a workspace
- `workspace unregister --name x` — remove a workspace from registry
- `workspace list` — list all registered workspaces
- `workspace discover --path /parent` — scan a directory for git repos and suggest workspace registration

**Before (single workspace):**
```bash
synapse lforge hyperforge workspace sync --path /Users/shmendez/dev/controlflow/hypermemetic
synapse lforge hyperforge build dirty --path /Users/shmendez/dev/controlflow/hypermemetic
```

**After (all workspaces):**
```bash
synapse lforge hyperforge workspace sync
synapse lforge hyperforge build dirty
```

**After (filtered):**
```bash
synapse lforge hyperforge workspace sync --workspace hypermemetic
synapse lforge hyperforge build dirty --workspace onebigmediaco --include "Form*"
```

### Auto-Registration

`begin` already knows the workspace_path. When `begin --workspace_path X` is called, it should also register the workspace automatically.

## Dependency DAG

```
WORK-2 (Workspace registry — config file + types)
  │
  ├──► WORK-3 (workspace register/unregister/list commands)
  │
  ├──► WORK-4 (Multi-workspace resolution — resolve --workspace/--path to workspace list)
  │      │
  │      └──► WORK-5 (Retrofit all commands — make --path optional, add --workspace)
  │
  └──► WORK-6 (Auto-registration from begin + repo init)

WORK-7 (workspace discover — scan directory for repos) ◄── WORK-3
```

## Tickets

| Ticket | Description | Depends on |
|--------|-------------|-----------|
| WORK-2 | Workspace registry: config file, WorkspaceEntry type, load/save | — |
| WORK-3 | workspace register/unregister/list commands | WORK-2 |
| WORK-4 | Multi-workspace resolution: resolve names/paths to workspace list | WORK-2 |
| WORK-5 | Retrofit all commands: --path optional, add --workspace filter | WORK-4 |
| WORK-6 | Auto-registration from begin and repo init | WORK-2 |
| WORK-7 | workspace discover: scan directory, suggest registrations | WORK-3 |

## Phases

### Phase 1: Foundation (WORK-2, WORK-3) — parallelizable
- Workspace registry type and persistence
- Basic CRUD commands (register, unregister, list)

### Phase 2: Resolution (WORK-4)
- Given --workspace name or --path or neither, resolve to a list of WorkspaceEntry
- This is the core plumbing that all commands use

### Phase 3: Retrofit (WORK-5) — biggest ticket
- Every command that takes --path gains --workspace
- --path becomes optional (falls back to all workspaces)
- Build commands (dirty, activity, loc, release, etc.) aggregate across workspaces

### Phase 4: Quality of Life (WORK-6, WORK-7)
- Auto-register workspaces during begin
- Discover command for bootstrapping

## Success Criteria

- `synapse lforge hyperforge workspace sync` syncs all workspaces
- `synapse lforge hyperforge build dirty` shows dirty repos across all workspaces
- `synapse lforge hyperforge build dirty --workspace hypermemetic` filters to one
- `synapse lforge hyperforge workspace list` shows all registered workspaces
- Existing `--path` flag still works (backward compat)
