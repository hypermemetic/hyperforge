#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 codeberg
TS=$(date +%s)
STAMP="hyperforge-v5-repos-10 $TS"

hf_spawn
hf_use_test_config

# --- read: exact four-key shape ---
out=$(hf_cmd repos sync --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO")
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ((.remote // .snapshot // {}) | keys | sort) == ["archived","default_branch","description","visibility"]'

# --- visibility string variants ---
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ((.remote // .snapshot // {}).visibility == "public" or (.remote // .snapshot // {}).visibility == "private")'

original=$(echo "$out" | jq -r 'select(.type == "forge_metadata" or .type == "sync_diff") | (.remote // .snapshot // {}).description' | head -n1)

# --- write round-trip + restore ---
hf_cmd repos push --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO" --fields "{\"description\":\"$STAMP\"}" >/dev/null
verify=$(hf_cmd repos sync --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO")
echo "$verify" | grep -q "$STAMP"
hf_cmd repos push --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO" --fields "{\"description\":\"$original\"}" >/dev/null

# --- auth error when token blank ---
hf_put_secret "secrets://cb-token" ""
set +e
err=$(hf_cmd repos sync --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "auth" or (.message // "" | test("auth"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'
hf_put_secret "secrets://cb-token" "$HF_TEST_CODEBERG_TOKEN"

# --- not_found for a bogus repo ---
BOGUS="nonexistent-$TS"
python3 - "$HF_CONFIG/orgs/${HF_TIER2_CODEBERG_ORG}.yaml" "$HF_TIER2_CODEBERG_ORG" "$BOGUS" <<'PY'
import sys, yaml
p, org, bogus = sys.argv[1], sys.argv[2], sys.argv[3]
d = yaml.safe_load(open(p))
d["repos"].append({"name": bogus, "remotes": [{"url": f"https://codeberg.org/{org}/{bogus}.git"}]})
open(p, "w").write(yaml.safe_dump(d))
PY
set +e
err=$(hf_cmd repos sync --org "$HF_TIER2_CODEBERG_ORG" --name "$BOGUS" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "not_found" or (.message // "" | test("not.?found|404"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'

# --- no token leakage ---
full=$(hf_cmd repos sync --org "$HF_TIER2_CODEBERG_ORG" --name "$HF_TIER2_CODEBERG_REPO" 2>&1)
! echo "$full" | grep -q "$HF_TEST_CODEBERG_TOKEN"

hf_teardown
echo "PASS"
