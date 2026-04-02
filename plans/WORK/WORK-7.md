# WORK-7: Workspace Discover

blocked_by: [WORK-3]
unlocks: []

## Scope

Scan a parent directory for potential workspaces and suggest or auto-register them.

## Method

`workspace discover` — params: `path` (String), `depth` (Option<u32>, default 2), `register` (Option<bool>, default false)

## Tests

### `test_discover_finds_workspace`
Create a tempdir structure:
```
/tmp/parent/
  workspace_a/
    repo1/.git/
    repo2/.git/
    repo3/.git/
  workspace_b/
    repo4/.git/
    repo5/.git/
    repo6/.git/
  single_repo/.git/
```
Call `workspace discover --path /tmp/parent`.
Assert events contain "workspace_a" (3 git repos) and "workspace_b" (3 git repos).
Assert events do NOT contain "single_repo" (only 1 repo, it IS a repo not a workspace).

### `test_discover_infers_org`
Create tempdir:
```
/tmp/parent/
  myws/
    repo1/.git/
    repo1/.hyperforge/config.toml  (contains org = "myorg")
    repo2/.git/
```
Call `workspace discover --path /tmp/parent`.
Assert the event for "myws" contains `org: myorg`.

### `test_discover_infers_forges`
Create tempdir:
```
/tmp/parent/
  myws/
    repo1/.git/config  (contains remote "origin" url = git@github.com:...)
    repo1/.git/config  (contains remote "codeberg" url = git@codeberg.org:...)
```
Call `workspace discover --path /tmp/parent`.
Assert the event for "myws" contains forges including "github".

### `test_discover_skips_registered`
Register workspace "myws" at "/tmp/parent/myws".
Create the same tempdir structure as test_discover_finds_workspace.
Call `workspace discover --path /tmp/parent`.
Assert events do NOT contain "myws" (already registered).
Assert events contain other unregistered workspaces.

### `test_discover_auto_register`
Create tempdir with 2 potential workspaces (3+ git repos each).
Call `workspace discover --path /tmp/parent --register true`.
Load registry. Assert both workspaces are now registered.

### `test_discover_depth_limit`
Create tempdir:
```
/tmp/parent/
  level1/
    level2/
      workspace_deep/
        repo1/.git/
        repo2/.git/
        repo3/.git/
```
Call `workspace discover --path /tmp/parent --depth 1`.
Assert "workspace_deep" is NOT found (it's at depth 2).
Call `workspace discover --path /tmp/parent --depth 2`.
Assert "workspace_deep" IS found.

### `test_discover_skips_git_repos`
Create tempdir:
```
/tmp/parent/
  not_a_workspace/.git/     (this IS a git repo itself)
    submodule/.git/
    other/.git/
```
Call `workspace discover --path /tmp/parent`.
Assert "not_a_workspace" is NOT suggested (it's a repo, not a workspace of repos).

### `test_discover_empty_dir`
Create empty tempdir. Call `workspace discover --path /tmp/empty`.
Assert single Info event: "No potential workspaces found."

### `test_discover_generates_register_commands`
Create tempdir with one potential workspace.
Call `workspace discover --path /tmp/parent`.
Assert one event contains a valid command string:
`workspace register --name workspace_a --path /tmp/parent/workspace_a --org inferred_org`
