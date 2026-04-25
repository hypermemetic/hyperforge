#!/usr/bin/env bash
# tier: 1
# V5PARITY-8 acceptance: reload, config_show, config_set_ssh_key,
# config_show_ssh_key, begin.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- begin on a fresh empty config dir ---
hf_spawn
out=$(hf_cmd begin)
echo "$out" | hf_assert_event '.type == "begin_next_step" and .action == "orgs.create"'
echo "$out" | hf_assert_event '.type == "begin_next_step" and .action == "secrets.set"'
echo "$out" | hf_assert_event '.type == "begin_next_step" and .action == "repos.import"'
[[ -f "$HF_CONFIG/config.yaml" ]]
grep -q 'github.com: github' "$HF_CONFIG/config.yaml"

# --- begin re-run is a no-op (config.yaml unchanged) ---
before=$(sha256sum "$HF_CONFIG/config.yaml")
hf_cmd begin >/dev/null
[[ "$(sha256sum "$HF_CONFIG/config.yaml")" == "$before" ]]
hf_teardown

# --- config_show + reload + ssh_key against a fixture ---
hf_spawn
hf_load_fixture minimal_org

# config_show emits the provider_map.
out=$(hf_cmd config_show)
echo "$out" | hf_assert_event '.type == "config_show" and (.provider_map | has("github.com"))'

# reload returns counts.
out=$(hf_cmd reload)
echo "$out" | hf_assert_event '.type == "reload_done" and .orgs >= 0 and .workspaces >= 0'

# config_set_ssh_key sets a key on the demo org.
out=$(hf_cmd config_set_ssh_key --org demo --forge github --key "/tmp/dummy_key")
echo "$out" | hf_assert_event '.type == "ssh_key_set" and .org == "demo" and .path == "/tmp/dummy_key"'

# config_show_ssh_key reads it back.
out=$(hf_cmd config_show_ssh_key --org demo --forge github)
echo "$out" | hf_assert_event '.type == "ssh_key_show" and .org == "demo" and .path == "/tmp/dummy_key"'

# config_show_ssh_key with a wrong forge filter returns null path.
out=$(hf_cmd config_show_ssh_key --org demo --forge codeberg)
echo "$out" | hf_assert_event '.type == "ssh_key_show" and .org == "demo" and ((.path // null) == null)'

hf_teardown
echo "PASS"
