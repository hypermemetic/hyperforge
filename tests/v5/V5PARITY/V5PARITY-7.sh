#!/usr/bin/env bash
# tier: 1
# V5PARITY-7 acceptance: secrets.{set,list_refs,delete} + auth_check + auth_requirements.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn
hf_load_fixture minimal_org

# --- secrets.set: writes to disk, masks value in event ---
out=$(hf_cmd secrets set --key "secrets://github/test/token" --value "abc123token")
echo "$out" | hf_assert_event '.type == "secret_set" and .key == "secrets://github/test/token" and .value_length == 11'
# Value MUST NOT appear in the event payload.
! echo "$out" | grep -q "abc123token"

# Verify on disk.
grep -q "github/test/token: abc123token" "$HF_CONFIG/secrets.yaml"

# --- secrets.list_refs: keys only, no values ---
out=$(hf_cmd secrets list_refs)
echo "$out" | hf_assert_event '.type == "secret_ref" and .key == "secrets://github/test/token" and .type_hint == "token"'
! echo "$out" | grep -q "abc123token"

# --- secrets.delete: removes the entry; existed=true ---
out=$(hf_cmd secrets delete --key "secrets://github/test/token")
echo "$out" | hf_assert_event '.type == "secret_deleted" and .key == "secrets://github/test/token" and .existed == true'
# Subsequent list_refs is empty.
out=$(hf_cmd secrets list_refs)
echo "$out" | hf_assert_no_event '.type == "secret_ref" and .key == "secrets://github/test/token"'

# --- secrets.delete on missing: existed=false (idempotent) ---
out=$(hf_cmd secrets delete --key "secrets://github/test/token")
echo "$out" | hf_assert_event '.type == "secret_deleted" and .existed == false'

# --- secrets.set with invalid ref errors ---
set +e
err=$(hf_cmd secrets set --key "not-a-secret-ref" --value "x" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.code == "invalid_ref" or (.message // "" | test("invalid"; "i")))'

# --- auth_requirements: lists creds + their presence ---
# Set a credential, then check requirements.
hf_cmd secrets set --key "secrets://gh-token" --value "real-token-value" >/dev/null
# Update the demo org to have a credential pointing at our key.
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials:
    - key: secrets://gh-token
      type: token
repos: []
YAML

out=$(hf_cmd auth_requirements)
echo "$out" | hf_assert_event '.type == "auth_requirement" and .org == "demo" and .provider == "github" and .key == "secrets://gh-token" and .cred_type == "token" and .present == true'

# --- auth_requirements with org filter ---
out=$(hf_cmd auth_requirements --org demo)
echo "$out" | hf_assert_count '.type == "auth_requirement"' 1

# --- auth_requirements when secret missing ---
hf_cmd secrets delete --key "secrets://gh-token" >/dev/null
out=$(hf_cmd auth_requirements)
echo "$out" | hf_assert_event '.type == "auth_requirement" and .present == false'

# --- auth_check: skipped without tier-2 (would hit real GitHub API) ---
if [[ -n "${HF_V5_TEST_CONFIG_DIR:-}" && -f "${HF_V5_TEST_CONFIG_DIR}/tier2.env" ]]; then
    hf_teardown
    hf_spawn
    hf_use_test_config
    out=$(hf_cmd auth_check)
    echo "$out" | hf_assert_event '.type == "auth_check_result" and .valid == true'
fi

hf_teardown
echo "PASS"
