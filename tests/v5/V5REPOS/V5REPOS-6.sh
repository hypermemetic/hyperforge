#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- default: local-only remove succeeds ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos remove --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_removed" or (.type == "repo_detail" and .ref.name == "widget")'
listed=$(hf_cmd repos list --org demo)
echo "$listed" | hf_assert_count '.type == "repo_summary"' 0
hf_teardown

# --- dry_run: same events, file unchanged ---
hf_spawn
hf_load_fixture org_with_repo
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
out=$(hf_cmd repos remove --org demo --name widget --dry_run true)
echo "$out" | hf_assert_event '.type == "repo_removed" or .type == "repo_detail"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
# Still queryable.
hf_cmd repos get --org demo --name widget | hf_assert_event '.type == "repo_detail"'
hf_teardown

# --- nonexistent name: error, no file change ---
hf_spawn
hf_load_fixture org_with_repo
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
set +e
out=$(hf_cmd repos remove --org demo --name nonexistent 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- delete_remote=true with no credential: adapter fails, entry preserved ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos remove --org demo --name widget --delete_remote true 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
# Local entry must still exist.
hf_cmd repos get --org demo --name widget | hf_assert_event '.type == "repo_detail"'
hf_teardown

# --- restart parity ---
hf_spawn
hf_load_fixture org_with_repo
hf_cmd repos remove --org demo --name widget >/dev/null
saved="$(mktemp -d -t hfv5-save-XXXXXX)"
cp -r "$HF_CONFIG"/. "$saved"/
hf_teardown
hf_spawn
cp -r "$saved"/. "$HF_CONFIG"/
listed=$(hf_cmd repos list --org demo)
echo "$listed" | hf_assert_count '.type == "repo_summary"' 0
rm -rf "$saved"
hf_teardown

# --- no plaintext secret leakage ---
hf_spawn
hf_load_fixture org_with_repo
hf_put_secret "secrets://gh-token" "ghp_leak_me_please"
out=$(hf_cmd repos remove --org demo --name widget 2>&1)
! echo "$out" | grep -q 'ghp_leak_me_please'
hf_teardown

echo "PASS"
