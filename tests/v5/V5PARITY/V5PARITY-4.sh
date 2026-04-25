#!/usr/bin/env bash
# tier: 1
# V5PARITY-4 acceptance: repo.{size, loc, large_files, dirty}
#   + workspace.{repo_sizes, loc, large_files, dirty}
# Tier-1 — uses a fixture checkout with known sizes.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# Build a checkout dir with known file sizes.
TMP="$(mktemp -d -t v5prty4-XXXXXX)"
CHECKOUT="$TMP/checkout"
mkdir -p "$CHECKOUT"
git init -q "$CHECKOUT"
# Known content: 10-byte text, 150KB binary file, and a few Rust/TOML files.
printf '0123456789' > "$CHECKOUT/small.txt"
head -c 153600 /dev/urandom > "$CHECKOUT/big.bin"
printf 'fn main() {}\n' > "$CHECKOUT/main.rs"
printf 'pub fn a() {}\npub fn b() {}\n' > "$CHECKOUT/lib.rs"
printf '[package]\nname = "x"\n' > "$CHECKOUT/Cargo.toml"

EXPECTED_FILES=5
EXPECTED_BYTES=$(find "$CHECKOUT" -type f -not -path '*/\.git/*' -printf '%s\n' | awk '{s+=$1} END {print s}')

# --- repos.size ---
out=$(hf_cmd repos size --path "$CHECKOUT")
echo "$out" | hf_assert_event ".type == \"repo_size_summary\" and .file_count == $EXPECTED_FILES"
echo "$out" | hf_assert_event ".type == \"repo_size_summary\" and .bytes == $EXPECTED_BYTES"

# --- repos.loc ---
out=$(hf_cmd repos loc --path "$CHECKOUT")
echo "$out" | hf_assert_event '.type == "repo_loc_summary" and (.by_language.rust // 0) == 3'
echo "$out" | hf_assert_event '.type == "repo_loc_summary" and (.by_language.toml // 0) == 2'
echo "$out" | hf_assert_event '.type == "repo_loc_summary" and .total >= 5'

# --- repos.large_files ---
# Default threshold 100 KB; only big.bin (150KB) should match.
out=$(hf_cmd repos large_files --path "$CHECKOUT")
echo "$out" | hf_assert_event '.type == "large_file" and (.path | endswith("big.bin"))'
echo "$out" | hf_assert_event '.type == "large_files_summary" and .count == 1'

# Higher threshold excludes it.
out=$(hf_cmd repos large_files --path "$CHECKOUT" --threshold 200)
echo "$out" | hf_assert_event '.type == "large_files_summary" and .count == 0'
echo "$out" | hf_assert_no_event '.type == "large_file"'

# --- repos.dirty (alias for V5PARITY-3 method; same callsite) ---
out=$(hf_cmd repos dirty --path "$CHECKOUT")
echo "$out" | hf_assert_event '.type == "repo_dirty"'

# --- workspace aggregates ---
# Register a demo org + workspace pointing at the checkout's parent dir.
mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
YAML
cat > "$HF_CONFIG/orgs/demo.yaml" <<YAML
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: checkout
    remotes:
      - url: "file://$CHECKOUT"
YAML
cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: $TMP
repos:
  - demo/checkout
YAML

# workspaces.repo_sizes
out=$(hf_cmd workspaces repo_sizes --name main)
echo "$out" | hf_assert_event '.type == "member_analytics" and .metric == "size" and .status == "ok"'
echo "$out" | hf_assert_event '.type == "workspace_analytics_summary" and .metric == "size" and .total == 1 and .ok == 1'

# workspaces.loc
out=$(hf_cmd workspaces loc --name main)
echo "$out" | hf_assert_event '.type == "workspace_analytics_summary" and .metric == "loc" and .total_loc >= 5'

# workspaces.large_files
out=$(hf_cmd workspaces large_files --name main)
echo "$out" | hf_assert_event '.type == "workspace_analytics_summary" and .metric == "large_files" and .total_large_files == 1'

# workspaces.dirty
out=$(hf_cmd workspaces dirty --name main)
echo "$out" | hf_assert_event '.type == "workspace_analytics_summary" and .metric == "dirty"'

rm -rf "$TMP"
hf_teardown
echo "PASS"
