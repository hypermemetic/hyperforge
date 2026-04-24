#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: remove the only credential, list becomes empty ---
hf_spawn
hf_load_fixture org_with_credentials
hf_cmd orgs remove_credential org=demo key=secrets://gh-token >/dev/null
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and (.credentials | length == 0) and .provider == "github"'
hf_teardown

# --- round-trip: fresh daemon still sees zero credentials ---
hf_spawn
hf_load_fixture org_with_credentials
hf_cmd orgs remove_credential org=demo key=secrets://gh-token >/dev/null
persist=$(mktemp -d)
cp -a "$HF_CONFIG/." "$persist/"
hf_teardown

hf_spawn
rm -rf "$HF_CONFIG"
cp -a "$persist/." "$HF_CONFIG/"
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and (.credentials | length == 0)'
rm -rf "$persist"
hf_teardown

# --- dry_run: event emitted, disk unchanged, credential still present ---
hf_spawn
hf_load_fixture org_with_credentials
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
hf_cmd orgs remove_credential org=demo key=secrets://gh-token dry_run=true >/dev/null
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and (.credentials | length == 1) and .credentials[0].key == "secrets://gh-token"'
hf_teardown

# --- key not found: typed error distinguishable from org not found ---
hf_spawn
hf_load_fixture org_with_credentials
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
key_err=$(hf_cmd orgs remove_credential org=demo key=secrets://nonexistent 2>&1)
set -e
echo "$key_err" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- org not found: typed error, distinguishable from key-not-found ---
hf_spawn
hf_load_fixture org_with_credentials
set +e
org_err=$(hf_cmd orgs remove_credential org=nonexistent key=secrets://x 2>&1)
set -e
echo "$org_err" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
# The two errors must differ in some observable way (message text or code).
set +e
key_err=$(hf_cmd orgs remove_credential org=demo key=secrets://missing 2>&1)
set -e
[[ "$org_err" != "$key_err" ]]
hf_teardown

# --- secret store untouched: removing credential does NOT delete secrets.yaml entry ---
hf_spawn
hf_load_fixture org_with_credentials
hf_put_secret "secrets://gh-token" "ghp_stays_in_store"
secrets_before=$(sha256sum "$HF_CONFIG/secrets.yaml" | awk '{print $1}')
hf_cmd orgs remove_credential org=demo key=secrets://gh-token >/dev/null
secrets_after=$(sha256sum "$HF_CONFIG/secrets.yaml" | awk '{print $1}')
[[ "$secrets_before" == "$secrets_after" ]]
hf_teardown

echo "PASS"
