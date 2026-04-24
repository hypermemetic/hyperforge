#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 gitlab

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITLAB_ORG"
TS=$(date +%s)
TEMP_REPO="v5prov-5-${TS}"

# Create + verify via GitLab API + delete round-trip.
out=$(hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"https://gitlab.com/${ORG}/${TEMP_REPO}.git\"}]" \
    --create_remote true --visibility private --description "V5PROV-5 smoke $TS")
echo "$out" | hf_assert_event '.type == "repo_created"'

TOKEN="${HF_TIER2_GITLAB_TOKEN:-}"
if [[ -n "$TOKEN" ]]; then
    # URL-encode slash.
    curl -sf -H "PRIVATE-TOKEN: $TOKEN" \
        "https://gitlab.com/api/v4/projects/${ORG}%2F${TEMP_REPO}" | jq -e '.visibility == "private"' >/dev/null
fi

out=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO" --delete_remote true)
echo "$out" | hf_assert_event '.type == "remote_deleted" or .type == "repo_deleted"'

# Unique to GitLab: internal visibility is SUPPORTED.
INT_REPO="v5prov-5-int-${TS}"
out=$(hf_cmd repos add --org "$ORG" --name "$INT_REPO" \
    --remotes "[{\"url\":\"https://gitlab.com/${ORG}/${INT_REPO}.git\"}]" \
    --create_remote true --visibility internal)
echo "$out" | hf_assert_event '.type == "repo_created"'
# Clean up the internal-visibility test repo.
hf_cmd repos delete --org "$ORG" --name "$INT_REPO" --delete_remote true >/dev/null || true

hf_teardown
echo "PASS"
