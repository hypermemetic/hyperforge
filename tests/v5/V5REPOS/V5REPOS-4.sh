#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- org_with_repo: one RepoDetail with one remote, provider derived as github ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_count '.type == "repo_detail"' 1
echo "$out" | hf_assert_event '.type == "repo_detail" and .ref.org == "demo" and .ref.name == "widget" and (.remotes | length) == 1 and .remotes[0].provider == "github"'
hf_teardown

# --- org_with_mirror_repo: two remotes, github then codeberg ---
hf_spawn
hf_load_fixture org_with_mirror_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 2 and .remotes[0].provider == "github" and .remotes[1].provider == "codeberg"'
hf_teardown

# --- org_with_custom_domain_repo: per-remote override wins ---
hf_spawn
hf_load_fixture org_with_custom_domain_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "gitlab"'
hf_teardown

# --- nonexistent org ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos get --org nonexistent --name widget 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
echo "$out" | hf_assert_no_event '.type == "repo_detail"'
hf_teardown

# --- nonexistent repo ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos get --org demo --name nonexistent 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
hf_teardown

# --- missing required params ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos get --org demo 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
set +e
out=$(hf_cmd repos get --name widget 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

echo "PASS"
