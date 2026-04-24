#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- add second ref to single-repo workspace ---
hf_spawn
hf_load_fixture "ws_empty"
# Add a second org fixture inline
cat > "$HF_CONFIG/orgs/acme.yaml" <<'YAML'
name: acme
forge:
  provider: codeberg
  credentials: []
repos:
  - name: tool
    remotes:
      - url: https://codeberg.org/acme/tool.git
YAML
mkdir -p "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/workspaces/main.yaml" <<'YAML'
name: main
path: /tmp/hf-v5-ws-add
repos:
  - demo/widget
YAML
out=$(hf_cmd workspaces add_repo name=main repo_ref=acme/tool)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .name == "main" and .repo_count == 2'
out2=$(hf_cmd workspaces get name=main)
echo "$out2" | hf_assert_event '.type == "workspace_detail" and (.repos | length == 2)'
echo "$out2" | hf_assert_event '.type == "workspace_detail" and (any(.repos[]; . == "acme/tool"))'
echo "$out2" | hf_assert_event '.type == "workspace_detail" and (any(.repos[]; . == "demo/widget"))'
hf_teardown

# --- dry_run yields same event, yaml byte-identical ---
hf_spawn
hf_load_fixture "ws_empty"
cat > "$HF_CONFIG/orgs/acme.yaml" <<'YAML'
name: acme
forge:
  provider: codeberg
  credentials: []
repos:
  - name: tool
    remotes:
      - url: https://codeberg.org/acme/tool.git
YAML
mkdir -p "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/workspaces/main.yaml" <<'YAML'
name: main
path: /tmp/hf-v5-ws-add
repos:
  - demo/widget
YAML
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces add_repo name=main repo_ref=acme/tool dry_run=true)
echo "$out" | hf_assert_event '.type == "workspace_summary" and .repo_count == 2'
after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- already a member ---
hf_spawn
hf_load_fixture "ws_cross_org"
before=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
out=$(hf_cmd workspaces add_repo name=multi repo_ref=acme/tool || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("acme/tool")'
after=$(sha256sum "$HF_CONFIG/workspaces/multi.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- workspace not found ---
hf_spawn
hf_load_fixture "ws_empty"
out=$(hf_cmd workspaces add_repo name=ghost repo_ref=demo/widget || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost")'
hf_teardown

# --- org not found ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces add_repo name=main repo_ref=ghost/widget || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("ghost")'
after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- repo not found in its org ---
hf_spawn
hf_load_fixture "ws_with_one_repo"
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces add_repo name=main repo_ref=demo/nothing || true)
echo "$out" | hf_assert_event '.type | test("error")'
echo "$out" | hf_assert_event '. | tostring | test("demo/nothing")'
after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
[[ "$before" == "$after" ]]
hf_teardown

echo "PASS"
