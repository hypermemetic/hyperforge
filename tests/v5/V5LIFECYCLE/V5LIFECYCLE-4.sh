#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5LIFECYCLE-4: ops::repo::{exists,create,delete}_on_forge are the
# only code paths that reach adapter lifecycle methods. Verify:
#   (a) structural grep — no callsite outside ops/;
#   (b) V5PROV-6 and V5PROV-8 still pass end-to-end when tier-2 config
#       is available (they SKIP otherwise — still green).

# (a) structural grep
cd "$(dirname "$0")/../../.."
violations=$(grep -RE 'adapter\.(create_repo|delete_repo|repo_exists)|for_provider\(' src/v5/ 2>/dev/null \
    | grep -vE '^src/v5/(ops|adapters)/' || true)
if [[ -n "$violations" ]]; then
    echo "DRY violation — direct adapter lifecycle calls or for_provider outside ops/:"
    echo "$violations"
    exit 1
fi

# (b) delegate to V5PROV-6 / V5PROV-8 as regression suites.
if [[ -n "${HF_V5_TEST_CONFIG_DIR:-}" && -f "${HF_V5_TEST_CONFIG_DIR}/tier2.env" ]]; then
    bash "$(dirname "$0")/../V5PROV/V5PROV-6.sh"
    bash "$(dirname "$0")/../V5PROV/V5PROV-8.sh"
else
    echo "SKIP (end-to-end regression): HF_V5_TEST_CONFIG_DIR not set; grep invariant still passed"
fi

echo "PASS"
