# WORK-6: Auto-Registration from begin and repo init

blocked_by: [WORK-2]
unlocks: []

## Scope

Automatically register workspaces when `begin` or `repo init` creates them, so users don't need a separate registration step.

## Tests

### `test_begin_registers_workspace`
Call `begin(org="neworg", forges="github,codeberg", workspace_path="/tmp/newws")`.
Load the workspace registry afterward.
Assert registry contains an entry named "neworg" with:
- path = "/tmp/newws"
- org = "neworg"
- forges = [GitHub, Codeberg]

### `test_begin_does_not_duplicate`
Register workspace "myorg" at path "/tmp/myorg".
Call `begin(org="myorg", forges="github", workspace_path="/tmp/myorg")`.
Load the registry. Assert only 1 entry for "myorg" (not 2).
Assert forges updated to [GitHub] (upsert, not duplicate).

### `test_begin_preserves_existing_workspaces`
Register workspace "other" at "/tmp/other".
Call `begin(org="neworg", forges="github", workspace_path="/tmp/neworg")`.
Assert registry has 2 entries: "other" and "neworg".

### `test_init_suggests_registration`
Create a repo at `/tmp/unregistered_ws/myrepo/`. No workspace registered for `/tmp/unregistered_ws/`.
Call `repo init(org="myorg", path="/tmp/unregistered_ws/myrepo", ...)`.
Collect events. Assert one Info event contains "not in a registered workspace" and contains a `workspace register` command suggestion.

### `test_init_no_suggestion_when_registered`
Register workspace "myws" at `/tmp/myws/`.
Call `repo init(org="myorg", path="/tmp/myws/newrepo", ...)`.
Collect events. Assert NO event contains "not in a registered workspace".

### `test_sync_suggests_registration`
Don't register any workspace. Call `workspace sync --path /tmp/unregistered`.
Collect events. Assert the final event suggests registering:
"Tip: Register this workspace with `workspace register --name ...`"

### `test_sync_no_suggestion_when_registered`
Register workspace at `/tmp/registered`. Call `workspace sync --path /tmp/registered --dry_run true`.
Collect events. Assert no registration suggestion.
