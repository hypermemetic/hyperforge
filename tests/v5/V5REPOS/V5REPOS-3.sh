#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- minimal_org: empty repo list ---
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd repos list --org demo)
echo "$out" | hf_assert_count '.type == "repo_summary"' 0
hf_teardown

# --- org_with_repo: exactly one summary with remote_count 1 ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos list --org demo)
echo "$out" | hf_assert_count '.type == "repo_summary"' 1
echo "$out" | hf_assert_event '.type == "repo_summary" and .org == "demo" and .name == "widget" and .remote_count == 1'
hf_teardown

# --- org_with_mirror_repo: one summary, remote_count 2 ---
hf_spawn
hf_load_fixture org_with_mirror_repo
out=$(hf_cmd repos list --org demo)
echo "$out" | hf_assert_count '.type == "repo_summary"' 1
echo "$out" | hf_assert_event '.type == "repo_summary" and .remote_count == 2'
hf_teardown

# --- nonexistent org: error, no summary ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos list --org nonexistent 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
echo "$out" | hf_assert_no_event '.type == "repo_summary"'
hf_teardown

# --- missing org param ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos list 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- determinism: two calls yield equal streams ---
hf_spawn
hf_load_fixture org_with_mirror_repo
a=$(hf_cmd repos list --org demo)
b=$(hf_cmd repos list --org demo)
[[ "$a" == "$b" ]]
hf_teardown

echo "PASS"
