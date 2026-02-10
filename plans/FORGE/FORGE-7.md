# FORGE-7: Verify — compile, test, smoke test

blocked_by: [FORGE-6]
unlocks: []

## Scope

Verify the full refactor compiles, passes tests, and works end-to-end.

## Steps

1. `cargo check` — compiles clean, no warnings from changed code
2. `cargo test` — all tests pass
3. Rebuild substrate, restart
4. `synapse substrate hyperforge repos_import --org hypermemetic --forge codeberg` — merges codeberg into existing repos
5. `synapse substrate hyperforge workspace_diff --org hypermemetic` — shows `remote_only` instead of `delete`
6. `synapse substrate hyperforge repos_list --org hypermemetic` — shows `forges: [github, codeberg]` instead of `origin`/`mirrors`
7. Existing `repos.yaml` with old format loads correctly (backward compat)
8. Create `~/.config/hyperforge/orgs/hypermemetic/defaults.yaml` with `forges: [github, codeberg]`
9. `synapse substrate hyperforge repos_create --org hypermemetic --name test-repo` — uses org defaults

## Acceptance criteria

- Zero compiler errors
- All existing tests pass
- Old `repos.yaml` format auto-migrates on load
- New repos created without `--forges` use org defaults
- `workspace_diff` labels are `remote_only`, not `delete`
