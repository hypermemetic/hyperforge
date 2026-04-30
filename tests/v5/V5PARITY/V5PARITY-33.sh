#!/usr/bin/env bash
# tier: 1 (source-shape) + tier 2 (real github with private repos).
# V5PARITY-33: list_repos uses /user/repos when target is the authed user.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

cd "$(dirname "$0")/../../.."

# --- (1) source-shape: github adapter calls /user/repos with affiliation=owner. ---
grep -q '/user/repos?affiliation=owner' src/v5/adapters/github.rs \
    || { echo "FAIL: github adapter missing /user/repos?affiliation=owner path" >&2; exit 1; }
grep -q "owner_filter" src/v5/adapters/github.rs \
    || { echo "FAIL: missing owner.login filter helper" >&2; exit 1; }
echo "source: /user/repos path + owner_filter present"

# --- (2) tier-2: import returns the private+public union. ---
if [[ -z "${HF_V5_TEST_CONFIG_DIR:-}" || ! -f "${HF_V5_TEST_CONFIG_DIR}/tier2.env" ]]; then
    echo "SKIP tier-2: HF_V5_TEST_CONFIG_DIR not set; source-shape passed"
    echo "PASS"
    exit 0
fi
set -a; source "$HF_V5_TEST_CONFIG_DIR/tier2.env"; set +a
ORG="$HF_TIER2_GITHUB_ORG"

hf_spawn
hf_use_test_config
out=$(hf_cmd repos import --org "$ORG" --forge github)
v5_total=$(echo "$out" | jq -r 'select(.type=="import_summary") | .total' | head -1)
gh_total=$(gh api "/user/repos?affiliation=owner&per_page=100" --jq 'length' 2>/dev/null || echo "?")
[[ "$v5_total" == "$gh_total" ]] \
    || { echo "FAIL: v5 imported $v5_total, gh sees $gh_total" >&2; exit 1; }
echo "tier-2: $v5_total imported = $gh_total visible to gh"
hf_teardown
echo "PASS"
