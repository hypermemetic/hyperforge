#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

: "${HF_TEST_GITLAB_HOST:=gitlab.com}"
hf_require_tier2 gitlab
TS=$(date +%s)
STAMP="hyperforge-v5-repos-11 $TS"

hf_spawn
hf_use_test_config

# --- read: four-key shape ---
out=$(hf_cmd repos sync --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO")
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ((.remote // .snapshot // {}) | keys | sort) == ["archived","default_branch","description","visibility"]'

# --- visibility ∈ {public, internal, private} ---
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ([(.remote // .snapshot // {}).visibility] | inside(["public","internal","private"]))'

original=$(echo "$out" | jq -r 'select(.type == "forge_metadata" or .type == "sync_diff") | (.remote // .snapshot // {}).description' | head -n1)

# --- write round-trip + restore ---
hf_cmd repos push --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO" --fields "{\"description\":\"$STAMP\"}" >/dev/null
verify=$(hf_cmd repos sync --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO")
echo "$verify" | grep -q "$STAMP"
hf_cmd repos push --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO" --fields "{\"description\":\"$original\"}" >/dev/null

# --- auth error when token blank ---
hf_put_secret "secrets://gl-token" ""
set +e
err=$(hf_cmd repos sync --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "auth" or (.message // "" | test("auth"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'
hf_put_secret "secrets://gl-token" "$HF_TEST_GITLAB_TOKEN"

# --- not_found ---
BOGUS="nonexistent-$TS"
python3 - "$HF_CONFIG/orgs/${HF_TIER2_GITLAB_ORG}.yaml" "$HF_TEST_GITLAB_HOST" "$HF_TIER2_GITLAB_ORG" "$BOGUS" <<'PY'
import sys, yaml
p, host, org, bogus = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
d = yaml.safe_load(open(p))
d["repos"].append({"name": bogus, "remotes": [{"url": f"https://{host}/{org}/{bogus}.git"}]})
open(p, "w").write(yaml.safe_dump(d))
PY
set +e
err=$(hf_cmd repos sync --org "$HF_TIER2_GITLAB_ORG" --name "$BOGUS" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "not_found" or (.message // "" | test("not.?found|404"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'

# --- no token leakage ---
full=$(hf_cmd repos sync --org "$HF_TIER2_GITLAB_ORG" --name "$HF_TIER2_GITLAB_REPO" 2>&1)
! echo "$full" | grep -q "$HF_TEST_GITLAB_TOKEN"

hf_teardown
echo "PASS"
