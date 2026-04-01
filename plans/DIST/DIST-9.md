# DIST-9: Workspace-Wide Release

blocked_by: [DIST-6]
unlocks: []

## Scope

Release all packages in a workspace in dependency order, respecting the existing publish-order DAG. Extends `build release` to workspace scale with parallel cross-compilation.

## Method

`build release_all` — release every binary-producing package in the workspace.

### Params
- `path` — workspace path
- `tag` — release tag (applied uniformly, or per-package via version)
- `targets` — target triples
- `include` / `exclude` — repo filters
- `forge` — target forges
- `dry_run`

### Flow

1. Discover workspace, detect build systems
2. Filter to repos that produce binaries (via DIST-3)
3. Compute publish order (existing `workspace_publish_order`)
4. For each repo in order:
   - Cross-compile for all targets (parallel across targets)
   - Package archives
   - Create release + upload assets
5. Emit per-repo and final summary

### Concurrency

- Cross-compilation across targets for a single repo: parallel via `run_batch_blocking`
- Repos are processed sequentially in publish order (dependencies first)
- Upload to multiple forges per repo: parallel

## Haskell Considerations

For Haskell packages (synapse, synapse-cc, plexus-protocol):
- Only native target (no cross-compilation)
- Static linking attempted on Linux (`--enable-executable-static`)
- Formula generation (DIST-7) runs after upload

## Acceptance Criteria

- [ ] Releases all binary-producing repos in workspace
- [ ] Respects dependency order
- [ ] Parallel cross-compilation per repo
- [ ] Summary shows total repos/targets/assets
- [ ] Works for mixed Rust + Haskell workspace
