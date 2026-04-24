#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)
TEMP_REPO="v5prov-7-${TS}"

# Create a fresh repo to delete.
hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${TEMP_REPO}.git\"}]" \
    --create_remote true --visibility private >/dev/null

# --- delete_remote true: remote gone + local entry dropped ---
out=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO" --delete_remote true)
echo "$out" | hf_assert_event '.type == "remote_deleted"'
echo "$out" | hf_assert_event '.type == "repo_deleted"'
set +e
gh repo view "${ORG}/${TEMP_REPO}" >/dev/null 2>&1
rc=$?
set -e
[[ $rc -ne 0 ]]

# --- delete_remote false (default): local only, no forge call ---
TEMP_REPO2="v5prov-7b-${TS}"
hf_cmd repos add --org "$ORG" --name "$TEMP_REPO2" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${TEMP_REPO2}.git\"}]" \
    --create_remote true --visibility private >/dev/null
out=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO2")
echo "$out" | hf_assert_no_event '.type == "remote_deleted"'
echo "$out" | hf_assert_event '.type == "repo_deleted"'
# Forge still has the repo.
gh repo view "${ORG}/${TEMP_REPO2}" --json name >/dev/null
# Clean up via gh since hyperforge no longer tracks it.
gh repo delete "${ORG}/${TEMP_REPO2}" --yes >/dev/null

# --- delete_remote true with blank token: auth error + local entry NOT dropped ---
TEMP_REPO3="v5prov-7c-${TS}"
hf_cmd repos add --org "$ORG" --name "$TEMP_REPO3" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${TEMP_REPO3}.git\"}]" \
    --create_remote true --visibility private >/dev/null
hf_put_secret "secrets://gh-token-blank" ""
sed -i 's|key: secrets://[^[:space:]]*|key: secrets://gh-token-blank|' "$HF_CONFIG/orgs/${ORG}.yaml"
set +e
err=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO3" --delete_remote true 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "auth" or .code == "auth" or (.message // "" | test("auth"; "i")))'
# Restore creds and clean up.
hf_use_test_config
hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO3" --delete_remote true >/dev/null

hf_teardown
echo "PASS"
