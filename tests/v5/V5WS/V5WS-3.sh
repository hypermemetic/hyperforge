#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- single-repo workspace round-trips shape ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
out=$(hf_cmd workspaces get name=main)
echo "$out" | hf_assert_event '.type == "workspace_detail" and .name == "main" and .path == "/tmp/hf-v5-ws-with-one-repo"'
echo "$out" | hf_assert_event '.type == "workspace_detail" and (.repos | length == 1)'
echo "$out" | hf_assert_count '.type == "workspace_detail"' 1
hf_teardown

# --- cross-org ordering preserved ---
hf_spawn
hf_load_fixture "ws_cross_org"
out=$(hf_cmd workspaces get name=multi)
# repos[0] is demo/widget (string form), repos[1] is acme/tool (string form) per fixture
first=$(echo "$out" | jq -r 'select(.type == "workspace_detail") | .repos[0]')
second=$(echo "$out" | jq -r 'select(.type == "workspace_detail") | .repos[1]')
[[ "$first" == "demo/widget" ]]
[[ "$second" == "acme/tool" ]]
hf_teardown

# --- mixed string/object forms round-trip ---
hf_spawn
hf_load_fixture "ws_cross_org"
cat > "$HF_CONFIG/workspaces/mixed.yaml" <<'YAML'
name: mixed
path: /tmp/hf-v5-ws-mixed
repos:
  - demo/widget
  - ref: acme/tool
    dir: tool-local
YAML
out=$(hf_cmd workspaces get name=mixed)
# First entry is a bare string; second is an object with dir
is_string=$(echo "$out" | jq -r 'select(.type == "workspace_detail") | .repos[0] | type')
[[ "$is_string" == "string" ]]
echo "$out" | hf_assert_event '.type == "workspace_detail" and (.repos[1].dir == "tool-local")'
hf_teardown

# --- not found ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
out=$(hf_cmd workspaces get name=nonexistent || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("nonexistent")'
echo "$out" | hf_assert_count '.type == "workspace_detail"' 0
hf_teardown

# --- missing required parameter ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
out=$(hf_cmd workspaces get || true)
echo "$out" | hf_assert_event '.type | test("error")'
hf_teardown

echo "PASS"
