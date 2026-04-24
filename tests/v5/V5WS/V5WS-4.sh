#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: create with empty repos ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces create name=main ws_path=/tmp/hf-v5-test-dev repos='[]')
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main" and .repo_count == 0'
test -f "$HF_CONFIG/workspaces/main.yaml"
hf_teardown

# --- state is on disk, not memory: respawn + list ---
hf_spawn
hf_load_fixture "ws_empty"
hf_cmd workspaces create name=main ws_path=/tmp/hf-v5-test-dev repos='[]' >/dev/null
captured_config="$HF_CONFIG"
hf_teardown

# Respawn, pointing at the same config dir
hf_spawn
# Copy the disk state we just wrote into the fresh HF_CONFIG
cp -r "$captured_config/." "$HF_CONFIG/" 2>/dev/null || true
out=$(hf_cmd workspaces list)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main"'
hf_teardown

# --- valid ref gets recorded in string form ---
hf_spawn
hf_load_fixture "ws_empty"
hf_cmd workspaces create name=main ws_path=/tmp/hf-v5-test-dev repos='["demo/widget"]' >/dev/null
out=$(hf_cmd workspaces get name=main)
first_ref=$(echo "$out" | jq -r 'select(.type == "workspace_detail") | .repos[0]')
[[ "$first_ref" == "demo/widget" ]]
hf_teardown

# --- dry_run emits event but no file on disk ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces create name=main ws_path=/tmp/hf-v5-test-dev repos='[]' dry_run=true)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main"'
[[ ! -f "$HF_CONFIG/workspaces/main.yaml" ]]
hf_teardown

# --- already exists => error, file untouched ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces create name=main ws_path=/tmp/hf-v5-other repos='[]' || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("main")'
after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- unknown repo ref => error, no file written ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces create name=fresh ws_path=/tmp/hf-v5-test-dev repos='["ghost/nothing"]' || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost/nothing")'
[[ ! -f "$HF_CONFIG/workspaces/fresh.yaml" ]]
hf_teardown

# --- invalid WorkspaceName => error ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces create name=bad/name ws_path=/tmp/hf-v5-test-dev repos='[]' || true)
echo "$out" | hf_assert_event '.type | test("error")'
[[ ! -f "$HF_CONFIG/workspaces/bad/name.yaml" ]]
hf_teardown

# --- non-absolute FsPath => error ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces create name=main ws_path=relative/path repos='[]' || true)
echo "$out" | hf_assert_event '.type | test("error")'
[[ ! -f "$HF_CONFIG/workspaces/main.yaml" ]]
hf_teardown

echo "PASS"
