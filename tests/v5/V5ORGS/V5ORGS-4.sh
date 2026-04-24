#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: create writes the file ---
hf_spawn
hf_load_fixture empty
captured_config="$HF_CONFIG"
out=$(hf_cmd orgs create name=hypermemetic provider=github)
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "hypermemetic" and .provider == "github" and .repo_count == 0'
test -f "$captured_config/orgs/hypermemetic.yaml"
hf_teardown

# --- round-trip: fresh daemon sees the created org ---
hf_spawn
hf_load_fixture empty
hf_cmd orgs create name=hypermemetic provider=github >/dev/null
saved_config="$HF_CONFIG"
# move config to a stable location, teardown, respawn pointing at it
persist=$(mktemp -d)
cp -a "$saved_config/." "$persist/"
hf_teardown

hf_spawn
rm -rf "$HF_CONFIG"
cp -a "$persist/." "$HF_CONFIG/"
out=$(hf_cmd orgs list)
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "hypermemetic"'
rm -rf "$persist"
hf_teardown

# --- dry_run: emits summary, no file written ---
hf_spawn
hf_load_fixture empty
out=$(hf_cmd orgs create name=hypermemetic provider=github dry_run=true)
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "hypermemetic"'
[[ ! -f "$HF_CONFIG/orgs/hypermemetic.yaml" ]]
hf_teardown

# --- already exists: typed error, file byte-identical ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs create name=demo provider=github 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("demo"))'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- invalid name: typed error, no file ---
hf_spawn
hf_load_fixture empty
set +e
out=$(hf_cmd orgs create name=bad/name provider=github 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
[[ ! -f "$HF_CONFIG/orgs/bad/name.yaml" ]]
[[ -z "$(ls "$HF_CONFIG/orgs" 2>/dev/null || true)" ]]
hf_teardown

# --- unknown provider: typed error, no file ---
hf_spawn
hf_load_fixture empty
set +e
out=$(hf_cmd orgs create name=hypermemetic provider=nonsense 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
[[ ! -f "$HF_CONFIG/orgs/hypermemetic.yaml" ]]
hf_teardown

echo "PASS"
