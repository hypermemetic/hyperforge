#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)
TEMP_REPO="v5prov-6-${TS}"
URL="https://github.com/${ORG}/${TEMP_REPO}.git"

# --- create_remote false → backward compat (no forge call, no repo_created event) ---
out=$(hf_cmd repos add --org "$ORG" --name "nocreate-${TS}" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/nocreate-${TS}.git\"}]")
echo "$out" | hf_assert_no_event '.type == "repo_created"'
# Verify forge has NOT created it.
set +e
gh repo view "${ORG}/nocreate-${TS}" >/dev/null 2>&1
rc=$?
set -e
[[ $rc -ne 0 ]]
# Clean up local entry.
hf_cmd repos remove --org "$ORG" --name "nocreate-${TS}" >/dev/null

# --- create_remote true → repo_created emitted + remote exists + private ---
out=$(hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"$URL\"}]" \
    --create_remote true --visibility private --description "V5PROV-6 $TS")
echo "$out" | hf_assert_event '.type == "repo_created"'
echo "$out" | hf_assert_event '.type == "repo_added"'
gh repo view "${ORG}/${TEMP_REPO}" --json visibility --jq '.visibility' | grep -qi private

# --- conflict path: rollback verified (local entry removed after conflict) ---
out=$(hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"$URL\"}]" \
    --create_remote true --visibility private 2>&1 || true)
echo "$out" | hf_assert_event '.type == "error" and (.code == "conflict" or (.message // "" | test("exist"; "i")))'
# Local entry count for this repo must be 1 (the original, not a duplicate).
list=$(hf_cmd repos list --org "$ORG")
count=$(echo "$list" | jq -r 'select(.type == "repo_summary" and .name == "'"$TEMP_REPO"'") | .name' | wc -l)
[[ "$count" == "1" ]]

# --- dry_run + create_remote: events emitted, no forge call ---
DRY_REPO="v5prov-6-dry-${TS}"
out=$(hf_cmd repos add --org "$ORG" --name "$DRY_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${DRY_REPO}.git\"}]" \
    --create_remote true --visibility private --dry_run true)
echo "$out" | hf_assert_event '.type == "repo_created"'
set +e
gh repo view "${ORG}/${DRY_REPO}" >/dev/null 2>&1
rc=$?
set -e
[[ $rc -ne 0 ]]   # NOT on the forge — dry_run
# And not in the local yaml either.
echo "$(hf_cmd repos list --org "$ORG")" | hf_assert_no_event '.type == "repo_summary" and .name == "'"$DRY_REPO"'"'

# --- cleanup: remove the real repo we created ---
hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO" --delete_remote true >/dev/null

hf_teardown
echo "PASS"
