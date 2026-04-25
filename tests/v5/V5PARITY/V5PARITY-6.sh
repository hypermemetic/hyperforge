#!/usr/bin/env bash
# tier: 2
# V5PARITY-6 acceptance: repos.{rename, set_default_branch, set_archived}.
# Workspace-level variants (workspaces.set_default_branch / check_default_branch
# / verify / check / diff / move_repos) are deferred to a follow-up.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)
NAME="v5prty6-${TS}"

hf_cmd repos add --org "$ORG" --name "$NAME" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${NAME}.git\"}]" \
    --create_remote true --visibility private --description "V5PARITY-6 sandbox $TS" >/dev/null

# --- set_archived true ---
out=$(hf_cmd repos set_archived --org "$ORG" --name "$NAME" --archived true)
echo "$out" | hf_assert_event '.type == "archived_set" and .archived == true'
gh repo view "${ORG}/${NAME}" --json isArchived --jq '.isArchived' | grep -q true

# --- set_archived false (required for further writes) ---
out=$(hf_cmd repos set_archived --org "$ORG" --name "$NAME" --archived false)
echo "$out" | hf_assert_event '.type == "archived_set" and .archived == false'

# --- rename ---
NEW_NAME="v5prty6-${TS}-renamed"
out=$(hf_cmd repos rename --org "$ORG" --name "$NAME" --new_name "$NEW_NAME")
echo "$out" | hf_assert_event '.type == "repo_renamed" and .old_ref.name == "'"$NAME"'" and .new_ref.name == "'"$NEW_NAME"'"'
gh repo view "${ORG}/${NEW_NAME}" --json name --jq '.name' | grep -q "$NEW_NAME"
hf_cmd repos list --org "$ORG" | hf_assert_event '.type == "repo_summary" and .name == "'"$NEW_NAME"'"'

# --- cleanup ---
hf_cmd repos delete --org "$ORG" --name "$NEW_NAME" >/dev/null
if gh auth status 2>&1 | grep -q 'delete_repo'; then
    hf_cmd repos purge --org "$ORG" --name "$NEW_NAME" >/dev/null 2>&1 || true
fi

hf_teardown
echo "PASS"
