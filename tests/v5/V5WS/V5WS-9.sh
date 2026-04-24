#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# Credential injection relies on V5ORGS-7-equivalent onboarding done by a
# sibling ticket; this script assumes the fixture orgs carry credentials
# and a secret is seeded via hf_put_secret before the call.
#
# If tier-2 credentials are unavailable in CI, V5WS-10 classifies U5 yellow.

orgs_snapshot () {
  (cd "$HF_CONFIG/orgs" && find . -type f | sort | xargs sha256sum)
}

# --- 1 member, 1 SyncDiff + 1 report ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
# Populate credentials on the org (expected tier-2 setup; secret keyed by fixture).
hf_put_secret "secrets://gh-token" "${HF_V5_GH_TOKEN:-missing}" || true
out=$(hf_cmd workspaces sync name=main)
echo "$out" | hf_assert_count '.type == "sync_diff"' 1
echo "$out" | hf_assert_count '.type == "workspace_sync_report"' 1
echo "$out" | hf_assert_event '.type == "workspace_sync_report" and .name == "main" and .total == 1 and ((.in_sync + .drifted + .errored) == 1) and (.per_repo | length == 1)'
hf_teardown

# --- cross-org workspace: SyncDiff events preserve yaml order ---
hf_spawn
hf_load_fixture "ws_cross_org"
hf_put_secret "secrets://gh-token"  "${HF_V5_GH_TOKEN:-missing}" || true
hf_put_secret "secrets://cb-token" "${HF_V5_CB_TOKEN:-missing}" || true
out=$(hf_cmd workspaces sync name=multi)
# First SyncDiff ref matches yaml order: demo/widget then acme/tool
first_ref_org=$(echo "$out" | jq -r 'select(.type == "sync_diff") | "\(.ref.org)/\(.ref.name)"' | head -n1)
second_ref_org=$(echo "$out" | jq -r 'select(.type == "sync_diff") | "\(.ref.org)/\(.ref.name)"' | sed -n '2p')
[[ "$first_ref_org" == "demo/widget" ]]
[[ "$second_ref_org" == "acme/tool" ]]
echo "$out" | hf_assert_event '.type == "workspace_sync_report" and .total == 2'
hf_teardown

# --- one org missing creds: that member errored, batch continues ---
hf_spawn
hf_load_fixture "ws_cross_org"
# Seed only one of two tokens
hf_put_secret "secrets://gh-token" "${HF_V5_GH_TOKEN:-missing}" || true
out=$(hf_cmd workspaces sync name=multi)
echo "$out" | hf_assert_event '.type == "workspace_sync_report" and .total == 2 and .errored >= 1'
echo "$out" | hf_assert_count '.type == "sync_diff"' 2
# RPC exit was success despite errored member — we got a report
hf_teardown

# --- zero-member workspace ---
hf_spawn
hf_load_fixture "ws_empty"
mkdir -p "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/workspaces/empty-ws.yaml" <<'YAML'
name: empty-ws
path: /tmp/hf-v5-empty-ws
repos: []
YAML
out=$(hf_cmd workspaces sync name=empty-ws)
echo "$out" | hf_assert_count '.type == "sync_diff"' 0
echo "$out" | hf_assert_event '.type == "workspace_sync_report" and .name == "empty-ws" and .total == 0 and .in_sync == 0 and .drifted == 0 and .errored == 0 and (.per_repo | length == 0)'
hf_teardown

# --- workspace not found ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces sync name=nonexistent || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("nonexistent")'
echo "$out" | hf_assert_count '.type == "workspace_sync_report"' 0
hf_teardown

# --- nothing on disk is modified by sync ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
hf_put_secret "secrets://gh-token" "${HF_V5_GH_TOKEN:-missing}" || true
mkdir -p /tmp/hf-v5-ws-with-one-repo
echo "sentinel" > /tmp/hf-v5-ws-with-one-repo/marker.txt
ws_yaml_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
orgs_before=$(orgs_snapshot)
fs_before=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum)
hf_cmd workspaces sync name=main >/dev/null
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$ws_yaml_before" ]]
[[ "$(orgs_snapshot)" == "$orgs_before" ]]
fs_after=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum)
[[ "$fs_before" == "$fs_after" ]]
rm -rf /tmp/hf-v5-ws-with-one-repo
hf_teardown

echo "PASS"
