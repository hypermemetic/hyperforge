#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- empty fixture: zero org_summary events ---
hf_spawn
hf_load_fixture empty
hf_cmd orgs list | hf_assert_count '.type == "org_summary"' 0
hf_teardown

# --- minimal_org: exactly one demo summary ---
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd orgs list)
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "demo" and .provider == "github" and .repo_count == 0'
echo "$out" | hf_assert_count '.type == "org_summary"' 1
hf_teardown

# --- two_orgs: ordered ascending, acme before demo ---
hf_spawn
hf_load_fixture two_orgs
out=$(hf_cmd orgs list)
echo "$out" | hf_assert_count '.type == "org_summary"' 2
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "acme"'
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "demo"'
# ordering: acme index < demo index
names=$(echo "$out" | jq -r 'select(.type == "org_summary") | .name')
[[ "$(echo "$names" | head -n1)" == "acme" ]]
[[ "$(echo "$names" | sed -n 2p)" == "demo" ]]
hf_teardown

# --- redaction: seed a secret, list emits no plaintext ---
hf_spawn
hf_load_fixture org_with_credentials
hf_put_secret "secrets://gh-token" "ghp_leak_me_please"
out=$(hf_cmd orgs list)
if echo "$out" | grep -q 'ghp_leak_me_please'; then
  echo "REDACTION FAIL: orgs.list leaked secret" >&2
  exit 1
fi
hf_teardown

# --- determinism + round-trip: two successive lists equal ---
hf_spawn
hf_load_fixture two_orgs
a=$(hf_cmd orgs list)
b=$(hf_cmd orgs list)
[[ "$a" == "$b" ]]
hf_teardown

echo "PASS"
