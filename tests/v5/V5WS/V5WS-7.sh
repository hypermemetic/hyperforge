#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

orgs_snapshot () {
  (cd "$HF_CONFIG/orgs" && find . -type f | sort | xargs sha256sum)
}

# --- happy path: remove one of two refs ---
hf_spawn
hf_load_fixture "ws_cross_org"
orgs_before=$(orgs_snapshot)
out=$(hf_cmd workspaces remove_repo name=multi repo_ref=acme/tool)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "multi" and .repo_count == 1'
out2=$(hf_cmd workspaces get name=multi)
echo "$out2" | hf_assert_event '.type == "workspace_detail" and (.repos | length == 1)'
echo "$out2" | hf_assert_no_event '.type == "workspace_detail" and (any(.repos[]; . == "acme/tool"))'
orgs_after=$(orgs_snapshot)
[[ "$orgs_before" == "$orgs_after" ]]
hf_teardown

# --- dry_run yields same event, yaml byte-identical ---
hf_spawn
hf_load_fixture "ws_cross_org"
before=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
out=$(hf_cmd workspaces remove_repo name=multi repo_ref=acme/tool dry_run=true)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .repo_count == 1'
after=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- object-form entry matched by ref, dropped ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
cat > "$HF_CONFIG/workspaces/main.yaml" <<'YAML'
name: main
path: /tmp/hf-v5-ws-rm-obj
repos:
  - ref: demo/widget
    dir: widget-local
YAML
hf_cmd workspaces remove_repo name=main repo_ref=demo/widget >/dev/null
out=$(hf_cmd workspaces get name=main)
echo "$out" | hf_assert_event '.type == "workspace_detail" and (.repos == [])'
hf_teardown

# --- ref not a member ---
hf_spawn
hf_load_fixture "ws_cross_org"
before=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
out=$(hf_cmd workspaces remove_repo name=multi repo_ref=ghost/none || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost/none")'
after=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- workspace not found ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
out=$(hf_cmd workspaces remove_repo name=ghost repo_ref=demo/widget || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost")'
hf_teardown

# --- dry_run + delete_remote: cascade + summary events; no file or forge touch ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
mkdir -p /tmp/hf-v5-ws-with-one-repo
ws_before=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum 2>/dev/null || echo "")
orgs_before=$(orgs_snapshot)
ws_yaml_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces remove_repo name=main repo_ref=demo/widget dry_run=true delete_remote=true)
echo "$out" | hf_assert_event '.ref.org == "demo" and .ref.name == "widget"'
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main"'
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$ws_yaml_before" ]]
[[ "$(orgs_snapshot)" == "$orgs_before" ]]
ws_after=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum 2>/dev/null || echo "")
[[ "$ws_before" == "$ws_after" ]]
rm -rf /tmp/hf-v5-ws-with-one-repo
hf_teardown

# --- workspace path tree untouched on non-dry remove ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
mkdir -p /tmp/hf-v5-ws-with-one-repo/widget
echo "sentinel" > /tmp/hf-v5-ws-with-one-repo/widget/marker.txt
ws_before=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum)
hf_cmd workspaces remove_repo name=main repo_ref=demo/widget >/dev/null
ws_after=$(cd /tmp/hf-v5-ws-with-one-repo && find . -type f | sort | xargs sha256sum)
[[ "$ws_before" == "$ws_after" ]]
rm -rf /tmp/hf-v5-ws-with-one-repo
hf_teardown

echo "PASS"
