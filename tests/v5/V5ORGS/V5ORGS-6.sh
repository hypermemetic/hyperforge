#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: provider change preserves credentials list ---
hf_spawn
hf_load_fixture org_with_credentials
before=$(hf_cmd orgs get org=demo)
# baseline cred snapshot
before_creds=$(echo "$before" | jq -c 'select(.type == "org_detail") | .credentials')

out=$(hf_cmd orgs update org=demo provider=codeberg)
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "demo" and .provider == "codeberg"'

after=$(hf_cmd orgs get org=demo)
echo "$after" | hf_assert_event '.type == "org_detail" and .provider == "codeberg"'
after_creds=$(echo "$after" | jq -c 'select(.type == "org_detail") | .credentials')
[[ "$before_creds" == "$after_creds" ]]
hf_teardown

# --- round-trip: fresh daemon reports new provider ---
hf_spawn
hf_load_fixture org_with_credentials
hf_cmd orgs update org=demo provider=codeberg >/dev/null
persist=$(mktemp -d)
cp -a "$HF_CONFIG/." "$persist/"
hf_teardown

hf_spawn
rm -rf "$HF_CONFIG"
cp -a "$persist/." "$HF_CONFIG/"
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and .provider == "codeberg"'
rm -rf "$persist"
hf_teardown

# --- dry_run: emits event, disk unchanged ---
hf_spawn
hf_load_fixture org_with_credentials
before_hash=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
out=$(hf_cmd orgs update org=demo provider=codeberg dry_run=true)
echo "$out" | hf_assert_event '.type == "org_summary" and .provider == "codeberg"'
after_hash=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before_hash" == "$after_hash" ]]
# fresh daemon sees the unchanged provider
hf_teardown
hf_spawn
hf_load_fixture org_with_credentials
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and .provider == "github"'
hf_teardown

# --- no-op (no optional fields): typed error, file unchanged ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs update org=demo 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- not found: typed error, no mutation ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs update org=nonexistent provider=github 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- unknown provider variant: typed error, file unchanged ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs update org=demo provider=not_a_variant 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

echo "PASS"
