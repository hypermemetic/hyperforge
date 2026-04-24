#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- github.com → github ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github"'
hf_teardown

# --- order preserved: github, codeberg ---
hf_spawn
hf_load_fixture org_with_mirror_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github" and .remotes[1].provider == "codeberg"'
hf_teardown

# --- per-remote override wins ---
hf_spawn
hf_load_fixture org_with_custom_domain_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "gitlab"'
hf_teardown

# --- unknown domain, no override: derivation error ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos add --org demo --name widget --remotes '[{"url":"https://git.unknown.example/demo/widget.git"}]' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
# The error must name the URL or the extracted host.
echo "$out" | grep -qE 'unknown\.example|git\.unknown'
hf_teardown

# --- provider_map hot-swap: remove entry → error; add back → success (no restart) ---
hf_spawn
hf_load_fixture org_with_repo
# Remove github.com entry. Fixture shape is known; a targeted sed line
# removal is sufficient and avoids a yaml-parser dep.
sed -i '/^[[:space:]]*github\.com:[[:space:]]*github[[:space:]]*$/d' "$HF_CONFIG/config.yaml"
set +e
out=$(hf_cmd repos get --org demo --name widget 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
# Restore and expect success without a respawn.
hf_add_provider_map github.com github
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github"'
hf_teardown

echo "PASS"
