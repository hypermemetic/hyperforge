#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)
REPO="v5life-6-${TS}"

# Create a repo to soft-delete.
hf_cmd repos add --org "$ORG" --name "$REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${REPO}.git\"}]" \
    --create_remote true --visibility public --description "soft-delete target $TS" >/dev/null

# --- soft-delete on an active repo ---
out=$(hf_cmd repos delete --org "$ORG" --name "$REPO")
echo "$out" | hf_assert_event '.type == "forge_privatized" and .provider == "github"'
echo "$out" | hf_assert_event '.type == "repo_dismissed" and .already == false'
echo "$out" | hf_assert_event '.type == "repo_dismissed" and (.privatized_on | index("github"))'

# Verify on GitHub — repo still exists, now private.
gh repo view "${ORG}/${REPO}" --json visibility --jq '.visibility' | grep -qi private

# Verify locally — lifecycle is dismissed, privatized_on has github.
got=$(hf_cmd repos get --org "$ORG" --name "$REPO")
echo "$got" | hf_assert_event '(.metadata // {}).lifecycle == "dismissed"'
echo "$got" | hf_assert_event '((.metadata // {}).privatized_on // []) | index("github")'

# --- already-dismissed: idempotent ---
out=$(hf_cmd repos delete --org "$ORG" --name "$REPO")
echo "$out" | hf_assert_event '.type == "repo_dismissed" and .already == true'

# --- protected repo refuses delete ---
hf_cmd repos protect --org "$ORG" --name "$REPO" --protected true >/dev/null 2>&1 || true
# Un-dismiss it to test the active+protected path. (Implementer may also
# provide an undismiss method; for this test we hand-edit the metadata.)
sed -i 's/lifecycle: dismissed/lifecycle: active/' "$HF_CONFIG/orgs/${ORG}.yaml"
set +e
err=$(hf_cmd repos delete --org "$ORG" --name "$REPO" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.code == "protected" or (.message // "" | test("protect"; "i")))'

# Cleanup: un-protect, dismiss, leave for V5LIFECYCLE-7 to exercise purge.
hf_cmd repos protect --org "$ORG" --name "$REPO" --protected false >/dev/null 2>&1 || true
hf_cmd repos delete --org "$ORG" --name "$REPO" >/dev/null 2>&1 || true

hf_teardown
echo "PASS"
