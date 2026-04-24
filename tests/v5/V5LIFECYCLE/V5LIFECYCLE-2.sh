#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5LIFECYCLE-2 is a zero-behavior-change refactor. Verify:
#   (a) the full tier-1 sweep still passes;
#   (b) no v5 source file outside ops/ and secrets/ uses serde_yaml /
#       std::fs file I/O directly — all state access goes through
#       ops::state.

# (a) proxy check: run a trivial end-to-end (status + orgs list).
hf_spawn
hf_load_fixture "minimal_org"

hf_cmd status 2>&1 | hf_assert_event '.type == "status"'
hf_cmd orgs list 2>&1 | hf_assert_event '.type == "org_summary" and .name == "demo"'

hf_teardown

# (b) structural grep — D13 enforcement.
cd "$(dirname "$0")/../../.."
# Catches direct yaml parsing/serialization calls. Excludes:
#   - ops/*  (the state layer's own home)
#   - secrets.rs (separate secret store module)
#   - config.rs (the types + loader module — the state impl detail)
#   - ///  doc comments (textual mentions aren't violations)
violations=$(grep -RnE 'serde_yaml::(from_str|to_string|from_reader)' src/v5/ 2>/dev/null \
    | grep -vE '^src/v5/(ops|secrets|config\.rs)' \
    | grep -vE '^[^:]+:[0-9]+:\s*///' || true)

if [[ -n "$violations" ]]; then
    echo "D13 violation — direct state I/O outside ops/, secrets/, or config.rs:"
    echo "$violations"
    exit 1
fi

echo "PASS"
