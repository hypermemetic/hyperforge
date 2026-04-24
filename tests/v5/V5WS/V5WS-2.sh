#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- empty workspace dir => zero summaries ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces list)
echo "$out" | hf_assert_count '.type == "workspace_summary"' 0
hf_teardown

# --- one workspace with one repo => exactly one summary ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
snapshot_before=$(cd "$HF_CONFIG" && find . -type f | sort | xargs sha256sum)
out=$(hf_cmd workspaces list)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main" and .repo_count == 1'
echo "$out" | hf_assert_event '.type == "workspace_summary" and .path == "/tmp/hf-v5-ws-with-one-repo"'
echo "$out" | hf_assert_count '.type == "workspace_summary"' 1
snapshot_after=$(cd "$HF_CONFIG" && find . -type f | sort | xargs sha256sum)
[[ "$snapshot_before" == "$snapshot_after" ]]
hf_teardown

# --- cross-org workspace set => two summaries ascending ---
hf_spawn
hf_load_fixture "ws_cross_org"
# Add a second workspace to exercise ordering
mkdir -p "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/workspaces/alpha.yaml" <<'YAML'
name: alpha
path: /tmp/hf-v5-ws-alpha
repos: []
YAML
out=$(hf_cmd workspaces list)
echo "$out" | hf_assert_count '.type == "workspace_summary"' 2
# alpha precedes multi in ascending order — first summary event is alpha
first_name=$(echo "$out" | jq -r 'select(.type == "workspace_summary") | .name' | head -n1)
[[ "$first_name" == "alpha" ]]
hf_teardown

echo "PASS"
