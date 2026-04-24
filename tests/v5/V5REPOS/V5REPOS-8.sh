#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- success: drop codeberg remote, github remains ---
hf_spawn
hf_load_fixture org_with_mirror_repo
out=$(hf_cmd repos remove_remote --org demo --name widget --url "https://codeberg.org/demo/widget.git")
echo "$out" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 1 and .remotes[0].provider == "github"'
hf_teardown

# --- dry_run: events match, file unchanged ---
hf_spawn
hf_load_fixture org_with_mirror_repo
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
out=$(hf_cmd repos remove_remote --org demo --name widget --url "https://codeberg.org/demo/widget.git" --dry_run true)
echo "$out" | hf_assert_event '.type == "repo_detail"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- last-remote rejected ---
hf_spawn
hf_load_fixture org_with_repo
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
set +e
out=$(hf_cmd repos remove_remote --org demo --name widget --url "https://github.com/demo/widget.git" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- url not present: error ---
hf_spawn
hf_load_fixture org_with_mirror_repo
set +e
out=$(hf_cmd repos remove_remote --org demo --name widget --url "https://nope.example/demo/widget.git" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- missing org / name: error ---
hf_spawn
hf_load_fixture org_with_mirror_repo
set +e
out=$(hf_cmd repos remove_remote --org nonexistent --name widget --url "https://codeberg.org/demo/widget.git" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
set +e
out=$(hf_cmd repos remove_remote --org demo --name nonexistent --url "https://codeberg.org/demo/widget.git" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- restart parity ---
hf_spawn
hf_load_fixture org_with_mirror_repo
hf_cmd repos remove_remote --org demo --name widget --url "https://codeberg.org/demo/widget.git" >/dev/null
saved="$(mktemp -d -t hfv5-save-XXXXXX)"
cp -r "$HF_CONFIG"/. "$saved"/
hf_teardown
hf_spawn
cp -r "$saved"/. "$HF_CONFIG"/
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 1'
rm -rf "$saved"
hf_teardown

# --- no plaintext secret leakage ---
hf_spawn
hf_load_fixture org_with_mirror_repo
hf_put_secret "secrets://gh-token" "ghp_leak_me_please"
out=$(hf_cmd repos remove_remote --org demo --name widget --url "https://codeberg.org/demo/widget.git" 2>&1)
! echo "$out" | grep -q 'ghp_leak_me_please'
hf_teardown

echo "PASS"
