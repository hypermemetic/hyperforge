#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

REMOTE='{"url":"https://github.com/demo/widget.git"}'

# --- success: add then get confirms ---
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]")
echo "$out" | hf_assert_event '.type == "repo_detail" and .ref.name == "widget"'
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 1 and .remotes[0].provider == "github"'
hf_teardown

# --- dry_run: events match, file byte-identical ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
out=$(hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]" --dry_run true)
echo "$out" | hf_assert_event '.type == "repo_detail" and .ref.name == "widget"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- restart parity: daemon state == disk state ---
hf_spawn
hf_load_fixture minimal_org
hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]" >/dev/null
saved="$HF_CONFIG"
hf_teardown
hf_spawn
cp -r "$saved"/* "$HF_CONFIG"/
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.type == "repo_detail" and .ref.name == "widget"'
rm -rf "$saved"
hf_teardown

# --- empty remotes rejected ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos add --org demo --name widget --remotes '[]' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- duplicate name rejected on second add ---
hf_spawn
hf_load_fixture minimal_org
hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]" >/dev/null
set +e
out=$(hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- unknown domain rejected ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos add --org demo --name widget --remotes '[{"url":"https://git.unknown.example/demo/widget.git"}]' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- no plaintext secret leakage ---
hf_spawn
hf_load_fixture minimal_org
hf_put_secret "secrets://gh-token" "ghp_leak_me_please"
out=$(hf_cmd repos add --org demo --name widget --remotes "[$REMOTE]" 2>&1)
! echo "$out" | grep -q 'ghp_leak_me_please'
hf_teardown

echo "PASS"
