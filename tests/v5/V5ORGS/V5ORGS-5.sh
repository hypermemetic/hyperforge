#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: delete one of two orgs, sibling untouched ---
hf_spawn
hf_load_fixture two_orgs
acme_before=$(sha256sum "$HF_CONFIG/orgs/acme.yaml" | awk '{print $1}')
out=$(hf_cmd orgs delete org=demo)
echo "$out" | hf_assert_event '.name == "demo"'
[[ ! -f "$HF_CONFIG/orgs/demo.yaml" ]]
acme_after=$(sha256sum "$HF_CONFIG/orgs/acme.yaml" | awk '{print $1}')
[[ "$acme_before" == "$acme_after" ]]
hf_teardown

# --- round-trip: fresh daemon sees exactly one remaining org ---
hf_spawn
hf_load_fixture two_orgs
hf_cmd orgs delete org=demo >/dev/null
persist=$(mktemp -d)
cp -a "$HF_CONFIG/." "$persist/"
hf_teardown

hf_spawn
rm -rf "$HF_CONFIG"
cp -a "$persist/." "$HF_CONFIG/"
out=$(hf_cmd orgs list)
echo "$out" | hf_assert_count '.type == "org_summary"' 1
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "acme"'
rm -rf "$persist"
hf_teardown

# --- dry_run: emits deletion event, file still present byte-identical ---
hf_spawn
hf_load_fixture two_orgs
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
out=$(hf_cmd orgs delete org=demo dry_run=true)
echo "$out" | hf_assert_event '.name == "demo"'
test -f "$HF_CONFIG/orgs/demo.yaml"
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- not found: typed error, no mutation ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs delete org=nonexistent 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- workspace referencing the org: delete succeeds, workspace file untouched ---
hf_spawn
hf_load_fixture org_with_workspace_ref
ws_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml" | awk '{print $1}')
hf_cmd orgs delete org=demo >/dev/null
[[ ! -f "$HF_CONFIG/orgs/demo.yaml" ]]
ws_after=$(sha256sum "$HF_CONFIG/workspaces/main.yaml" | awk '{print $1}')
[[ "$ws_before" == "$ws_after" ]]
hf_teardown

echo "PASS"
