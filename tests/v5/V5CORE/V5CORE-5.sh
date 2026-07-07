#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# status returns an event tagged "status" with non-empty version and an
# absolute, well-formed config_dir.
out=$(hf_cmd status)

echo "$out" | hf_assert_event '.type == "status"'
echo "$out" | hf_assert_event '.type == "status" and (.version | type == "string") and (.version | length > 0)'
echo "$out" | hf_assert_event '.type == "status" and (.config_dir | startswith("/"))'
echo "$out" | hf_assert_event '.type == "status" and (.config_dir | contains("..") | not)'
echo "$out" | hf_assert_event '.type == "status" and (.config_dir | endswith("/") | not)'

# config_dir matches the harness-allocated HF_CONFIG (absolute form).
hf_config_abs=$(readlink -f "$HF_CONFIG")
echo "$out" | hf_assert_event ".type == \"status\" and .config_dir == \"$hf_config_abs\""

# Schema: the root hub exposes `status`. (V5CORE-5's original baseline
# asserted exactly one method; later tickets legitimately added root
# methods — auth_check, reload, config_*, begin — so assert the shape,
# not a count that goes stale with every root-method addition.)
schema=$(hf_cmd __schema__ 2>/dev/null || hf_cmd)
echo "$schema" | hf_assert_count '.activation == "HyperforgeHub" and (.methods | index("status")) != null' 1

hf_teardown
echo "PASS"
