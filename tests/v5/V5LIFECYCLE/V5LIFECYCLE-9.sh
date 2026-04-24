#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn
hf_load_fixture "empty"

TMP="$(mktemp -d -t v5life-9-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# --- first init ---
out=$(hf_cmd repos init --target_path "$TMP" --org demo --repo_name widget \
    --forges '["github"]' --visibility private --description "hello")
echo "$out" | hf_assert_event '.type == "hyperforge_config_written" and .repo_name == "widget"'

[[ -f "$TMP/.hyperforge/config.toml" ]]

# Contents parse to expected fields.
grep -q '^repo_name' "$TMP/.hyperforge/config.toml"
grep -q '^org' "$TMP/.hyperforge/config.toml"
grep -q '^forges' "$TMP/.hyperforge/config.toml"
grep -q 'github' "$TMP/.hyperforge/config.toml"

# --- second init without --force fails ---
set +e
err=$(hf_cmd repos init --target_path "$TMP" --org demo --repo_name widget \
    --forges '["github"]' 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.code == "already_exists" or (.message // "" | test("force"; "i")))'

# --- with --force overwrites ---
out=$(hf_cmd repos init --target_path "$TMP" --org demo --repo_name widget2 \
    --forges '["github"]' --force true)
echo "$out" | hf_assert_event '.type == "hyperforge_config_written" and .repo_name == "widget2"'
grep -q 'widget2' "$TMP/.hyperforge/config.toml"

# --- dry_run on a new path does NOT create the file ---
TMP2="$(mktemp -d -t v5life-9b-XXXXXX)"
trap 'rm -rf "$TMP" "$TMP2"' EXIT
out=$(hf_cmd repos init --target_path "$TMP2" --org demo --repo_name dry \
    --forges '["github"]' --dry_run true)
echo "$out" | hf_assert_event '.type == "hyperforge_config_written"'
[[ ! -e "$TMP2/.hyperforge/config.toml" ]]

hf_teardown
echo "PASS"
