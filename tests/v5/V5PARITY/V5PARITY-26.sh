#!/usr/bin/env bash
# tier: 1
# V5PARITY-26 acceptance: status surfaces an onboarding_hint when the
# config dir is empty; absent once orgs/workspaces exist.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- empty-config status carries the hint ---
hf_spawn
out=$(hf_cmd status)
echo "$out" | hf_assert_event '.type == "status" and (.onboarding_hint | type == "string")'
echo "empty-config: hint present"

# --- after orgs.create, hint goes away ---
hf_cmd orgs create --name foo --provider github >/dev/null
out=$(hf_cmd status)
echo "$out" | hf_assert_no_event '.onboarding_hint != null'
echo "post-create: hint absent"

hf_teardown
echo "PASS"
