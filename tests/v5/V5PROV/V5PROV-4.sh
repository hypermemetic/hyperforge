#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 codeberg

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_CODEBERG_ORG"
TS=$(date +%s)
TEMP_REPO="v5prov-4-${TS}"

# Core lifecycle round-trip: create → exists-via-gh-like API → delete → absent.
out=$(hf_cmd repos add --org "$ORG" --name "$TEMP_REPO" \
    --remotes "[{\"url\":\"https://codeberg.org/${ORG}/${TEMP_REPO}.git\"}]" \
    --create_remote true --visibility private --description "V5PROV-4 smoke $TS")
echo "$out" | hf_assert_event '.type == "repo_created" and .ref.name == "'"$TEMP_REPO"'"'

# Verify directly via Codeberg API (no hyperforge).
TOKEN="${HF_TIER2_CODEBERG_TOKEN:-}"
if [[ -n "$TOKEN" ]]; then
    curl -sf -H "Authorization: token $TOKEN" \
        "https://codeberg.org/api/v1/repos/${ORG}/${TEMP_REPO}" | jq -e '.private == true' >/dev/null
fi

# Delete with cascade.
out=$(hf_cmd repos delete --org "$ORG" --name "$TEMP_REPO" --delete_remote true)
echo "$out" | hf_assert_event '.type == "remote_deleted" or .type == "repo_deleted"'

# internal visibility → unsupported_visibility (Gitea doesn't do internal).
REJ_REPO="v5prov-4-rej-${TS}"
out=$(hf_cmd repos add --org "$ORG" --name "$REJ_REPO" \
    --remotes "[{\"url\":\"https://codeberg.org/${ORG}/${REJ_REPO}.git\"}]" \
    --create_remote true --visibility internal 2>&1 || true)
echo "$out" | hf_assert_event '.type == "error" and (.code == "unsupported_visibility" or (.message // "" | test("unsupported|internal"; "i")))'

hf_teardown
echo "PASS"
