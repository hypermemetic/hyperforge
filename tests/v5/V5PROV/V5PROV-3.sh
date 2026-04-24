#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
# Use a timestamped name so the test is idempotent across runs.
TS=$(date +%s)
TEMP_REPO="v5prov-3-${TS}"

# --- create_repo: success creates remote + repo_exists reflects it ---
cat >> "$HF_CONFIG/orgs/${ORG}.yaml" <<YAML
  - name: $TEMP_REPO
    remotes:
      - url: https://github.com/${ORG}/${TEMP_REPO}.git
YAML

out=$(hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${TEMP_REPO}.git\"}]" \
    --create_remote true --visibility private --description "V5PROV-3 smoke $TS")
echo "$out" | hf_assert_event '.type == "repo_created" and .ref.name == "'"$TEMP_REPO"'"'

# --- verify via gh (external check, not via hyperforge) ---
gh repo view "${ORG}/${TEMP_REPO}" --json visibility --jq '.visibility' | grep -qi private

# --- repo_exists returns true; delete_repo removes it ---
out=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO" --delete_remote true)
echo "$out" | hf_assert_event '.type == "remote_deleted" or .type == "repo_deleted"'

# --- gh confirms deletion ---
set +e
gh repo view "${ORG}/${TEMP_REPO}" >/dev/null 2>&1
rc=$?
set -e
[[ $rc -ne 0 ]]

# --- create with visibility=internal on github.com is rejected ---
REJ_REPO="v5prov-3-rej-${TS}"
out=$(hf_cmd repos add --org "$ORG" --name "$REJ_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${REJ_REPO}.git\"}]" \
    --create_remote true --visibility internal 2>&1 || true)
echo "$out" | hf_assert_event '.type == "error" and (.code == "unsupported_visibility" or (.message // "" | test("unsupported|internal"; "i")))'

# --- conflict on already-existing repo name ---
out=$(hf_cmd repos add --org "$ORG" --name "$HF_TIER2_GITHUB_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${HF_TIER2_GITHUB_REPO}.git\"}]" \
    --create_remote true --visibility private 2>&1 || true)
echo "$out" | hf_assert_event '.type == "error" and (.code == "conflict" or (.message // "" | test("already"; "i")))'

hf_teardown
echo "PASS"
