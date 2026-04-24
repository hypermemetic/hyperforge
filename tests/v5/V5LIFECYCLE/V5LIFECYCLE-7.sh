#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

# Also requires delete_repo scope on the gh token — this script
# exercises the hard-delete path. Classify SKIP (not FAIL) if absent.
if ! gh auth status 2>&1 | grep -q 'delete_repo'; then
    echo "SKIP: gh token lacks delete_repo scope (run: gh auth refresh -h github.com -s delete_repo)"
    exit 0
fi

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)
REPO="v5life-7-${TS}"

# Create + dismiss → then purge.
hf_cmd repos add --org "$ORG" --name "$REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${REPO}.git\"}]" \
    --create_remote true --visibility private --description "purge target $TS" >/dev/null
hf_cmd repos delete --org "$ORG" --name "$REPO" >/dev/null

# --- purge dismissed non-protected ---
out=$(hf_cmd repos purge --org "$ORG" --name "$REPO")
echo "$out" | hf_assert_event '.type == "forge_deleted" and .provider == "github"'
echo "$out" | hf_assert_event '.type == "repo_purged"'

# Forge is 404, local record gone.
set +e
gh repo view "${ORG}/${REPO}" >/dev/null 2>&1; rc=$?
set -e
[[ $rc -ne 0 ]]
hf_cmd repos list --org "$ORG" | hf_assert_no_event '.type == "repo_summary" and .name == "'"$REPO"'"'

# --- purge active repo refuses ---
REPO2="v5life-7b-${TS}"
hf_cmd repos add --org "$ORG" --name "$REPO2" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${REPO2}.git\"}]" \
    --create_remote true --visibility private >/dev/null
set +e
err=$(hf_cmd repos purge --org "$ORG" --name "$REPO2" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.code == "not_dismissed" or (.message // "" | test("dismiss"; "i")))'

# --- purge protected+dismissed refuses ---
hf_cmd repos delete --org "$ORG" --name "$REPO2" >/dev/null
hf_cmd repos protect --org "$ORG" --name "$REPO2" --protected true >/dev/null
set +e
err=$(hf_cmd repos purge --org "$ORG" --name "$REPO2" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.code == "protected" or (.message // "" | test("protect"; "i")))'

# Cleanup.
hf_cmd repos protect --org "$ORG" --name "$REPO2" --protected false >/dev/null
hf_cmd repos purge --org "$ORG" --name "$REPO2" >/dev/null

hf_teardown
echo "PASS"
