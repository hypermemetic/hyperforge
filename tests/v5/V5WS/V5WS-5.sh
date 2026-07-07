#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# Per-process workspace dir: V5WS-5 and V5WS-7 both used the fixture's
# literal /tmp/hf-v5-ws-with-one-repo and rm -rf'd it — under parallel
# `cargo test` the two scripts raced and clobbered each other's tree.
WS_DIR="/tmp/hf-v5-ws5-$$"

orgs_snapshot () {
  (cd "$HF_CONFIG/orgs" && find . -type f | sort | xargs sha256sum)
}

# --- happy path: delete yaml, orgs untouched ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
orgs_before=$(orgs_snapshot)
out=$(hf_cmd workspaces delete name=main)
echo "$out" | hf_assert_event '.type == "workspace_deleted" and .name == "main"'
[[ ! -f "$HF_CONFIG/workspaces/main.yaml" ]]
orgs_after=$(orgs_snapshot)
[[ "$orgs_before" == "$orgs_after" ]]
hf_teardown

# --- respawn: list is empty ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
hf_cmd workspaces delete name=main >/dev/null
captured="$HF_CONFIG"
hf_teardown
hf_spawn
cp -r "$captured/." "$HF_CONFIG/" 2>/dev/null || true
out=$(hf_cmd workspaces list)
echo "$out" | hf_assert_count '.type == "workspace_summary"' 0
hf_teardown

# --- dry_run: event but file byte-identical ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces delete name=main dry_run=true)
echo "$out" | hf_assert_event '.type == "workspace_deleted" and .name == "main"'
after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- not found ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces delete name=ghost || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost")'
hf_teardown

# --- dry_run + delete_remote: cascade events emitted, no filesystem or forge touch ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
sed -i.sedbak "s|path: /tmp/hf-v5-ws-with-one-repo|path: $WS_DIR|" "$HF_CONFIG/workspaces/main.yaml" && rm -f "$HF_CONFIG/workspaces/main.yaml.sedbak"
# Create the workspace path so we can assert it's byte-identical after
mkdir -p "$WS_DIR"
ws_before=$(cd "$WS_DIR" && find . -type f | sort | xargs sha256sum 2>/dev/null || echo "")
orgs_before=$(orgs_snapshot)
ws_yaml_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces delete name=main dry_run=true delete_remote=true)
echo "$out" | hf_assert_event '.type == "workspace_deleted" and .name == "main"'
# One cascade event per member (ws_with_one_repo has 1 member: demo/widget)
echo "$out" | hf_assert_event '.ref.org == "demo" and .ref.name == "widget"'
# No file changed
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$ws_yaml_before" ]]
[[ "$(orgs_snapshot)" == "$orgs_before" ]]
ws_after=$(cd "$WS_DIR" && find . -type f | sort | xargs sha256sum 2>/dev/null || echo "")
[[ "$ws_before" == "$ws_after" ]]
rm -rf "$WS_DIR"
hf_teardown

# --- workspace path tree untouched on non-dry real delete ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
sed -i.sedbak "s|path: /tmp/hf-v5-ws-with-one-repo|path: $WS_DIR|" "$HF_CONFIG/workspaces/main.yaml" && rm -f "$HF_CONFIG/workspaces/main.yaml.sedbak"
mkdir -p "$WS_DIR/widget"
echo "sentinel" > "$WS_DIR/widget/marker.txt"
ws_before=$(cd "$WS_DIR" && find . -type f | sort | xargs sha256sum)
hf_cmd workspaces delete name=main >/dev/null
ws_after=$(cd "$WS_DIR" && find . -type f | sort | xargs sha256sum)
[[ "$ws_before" == "$ws_after" ]]
rm -rf "$WS_DIR"
hf_teardown

echo "PASS"
