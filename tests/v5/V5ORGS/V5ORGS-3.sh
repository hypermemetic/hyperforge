#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- minimal_org: one org_detail, zero credentials ---
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and .name == "demo" and .provider == "github" and (.credentials | length == 0) and (.repos | length == 0)'
hf_teardown

# --- org_with_credentials: one CredentialEntry, key + type only ---
hf_spawn
hf_load_fixture org_with_credentials
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and .name == "demo" and (.credentials | length == 1) and .credentials[0].key == "secrets://gh-token" and .credentials[0].type == "token"'
hf_teardown

# --- redaction: resolved plaintext never appears in orgs.get output ---
hf_spawn
hf_load_fixture org_with_credentials
hf_put_secret "secrets://gh-token" "ghp_leak_me_please"
out=$(hf_cmd orgs get org=demo)
if echo "$out" | grep -q 'ghp_leak_me_please'; then
  echo "REDACTION FAIL: orgs.get leaked secret" >&2
  exit 1
fi
# still returned the reference
echo "$out" | hf_assert_event '.credentials[0].key == "secrets://gh-token"'
hf_teardown

# --- unknown org: typed error naming it, no org_detail ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd orgs get org=nonexistent 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
echo "$out" | hf_assert_no_event '.type == "org_detail"'
hf_teardown

# --- missing required param: typed error ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd orgs get 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

echo "PASS"
