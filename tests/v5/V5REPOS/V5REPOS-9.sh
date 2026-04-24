#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

TS=$(date +%s)
STAMP="hyperforge-v5-repos-9 $TS"

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
REPO="$HF_TIER2_GITHUB_REPO"

# --- read capability: exact four-field shape ---
out=$(hf_cmd repos sync --org "$ORG" --name "$REPO")
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ((.remote // .snapshot // {}) | keys | sort) == ["archived","default_branch","description","visibility"]'

original=$(echo "$out" | jq -r 'select(.type == "forge_metadata" or .type == "sync_diff") | (.remote // .snapshot // {}).description' | head -n1)

# --- write capability: round-trip through adapter, then restore ---
out=$(hf_cmd repos push --org "$ORG" --name "$REPO" --fields "{\"description\":\"$STAMP\"}")
echo "$out" | hf_assert_event '.type == "error"' || \
  echo "$out" | hf_assert_event '.type == "push_remote_ok" or .type == "forge_metadata"'

verify=$(hf_cmd repos sync --org "$ORG" --name "$REPO")
echo "$verify" | grep -q "$STAMP"

hf_cmd repos push --org "$ORG" --name "$REPO" --fields "{\"description\":\"$original\"}" >/dev/null

# --- auth error: point the org cred ref at a blank secret ---
hf_put_secret "secrets://gh-token-blank" ""
sed -i 's|key: secrets://[^[:space:]]*|key: secrets://gh-token-blank|' "$HF_CONFIG/orgs/${ORG}.yaml"
set +e
err=$(hf_cmd repos sync --org "$ORG" --name "$REPO" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "auth" or (.message // "" | test("auth"; "i")))'
hf_use_test_config   # restore original config

# --- not_found for a bogus repo (append a bogus entry under the existing repos: list) ---
BOGUS="definitely-does-not-exist-$TS"
cat >> "$HF_CONFIG/orgs/${ORG}.yaml" <<YAML
  - name: $BOGUS
    remotes:
      - url: https://github.com/${ORG}/${BOGUS}.git
YAML
set +e
err=$(hf_cmd repos sync --org "$ORG" --name "$BOGUS" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "not_found" or (.message // "" | test("not.?found|404"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'

# --- no token leakage in outputs (compare against the value sourced from user's secrets.yaml) ---
token_val=$(grep -E '^[[:space:]]*[^#[:space:]].*token.*:' "$HF_V5_TEST_CONFIG_DIR/secrets.yaml" | head -n1 | sed 's/^[^:]*:[[:space:]]*//' | tr -d '"' || echo "")
full=$(hf_cmd repos sync --org "$ORG" --name "$REPO" 2>&1; \
       hf_cmd repos push --org "$ORG" --name "$REPO" --fields "{\"description\":\"$original\"}" 2>&1 || true)
if [[ -n "$token_val" ]]; then
    ! echo "$full" | grep -q "$token_val"
fi

hf_teardown
echo "PASS"
