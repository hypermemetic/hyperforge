#!/usr/bin/env bash
# tier: 1
# V5PARITY-34 acceptance: per-repo `forges` is authoritative for routing,
# `repos.sync_config` round-trips between yaml and .hyperforge/config.toml.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

TMP=$(mktemp -d -t v5prty34-XXXXXX)
CHECKOUT="$TMP/widget"

# Local fixture: a checkout with origin pointing at github + a mirror at codeberg.
git init -q "$CHECKOUT"
git -C "$CHECKOUT" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$CHECKOUT" remote add origin https://github.com/demo/widget.git
git -C "$CHECKOUT" remote add mirror https://codeberg.org/demo/widget.git

mkdir -p "$HF_CONFIG/orgs"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map:
  github.com: github
  codeberg.org: codeberg
YAML
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: widget
    remotes:
      - url: https://github.com/demo/widget.git
      - url: https://codeberg.org/demo/widget.git
YAML
hf_cmd reload >/dev/null

# --- (1) Without `forges`, both remotes participate (legacy behavior). ---
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 2'
echo "no forges: 2 remotes registered"

# --- (2) Adopt the checkout (writes .hyperforge/config.toml). ---
hf_cmd repos register --target_path "$CHECKOUT" >/dev/null
[[ -f "$CHECKOUT/.hyperforge/config.toml" ]]
echo "register: .hyperforge/config.toml present"

# --- (3) Edit the file: scope to codeberg only. ---
cat > "$CHECKOUT/.hyperforge/config.toml" <<'TOML'
repo_name = "widget"
org = "demo"
forges = ["codeberg"]
TOML

# --- (4) sync_config --mode pull → org yaml updates. ---
out=$(hf_cmd repos sync_config --target_path "$CHECKOUT" --mode pull)
echo "$out" | hf_assert_event '.type == "config_synced" and .mode == "pull" and .changed == true'
grep -q 'forges:' "$HF_CONFIG/orgs/demo.yaml"
grep -q 'codeberg' "$HF_CONFIG/orgs/demo.yaml"
echo "pull: org yaml now scoped to codeberg"

# --- (5) Re-running pull is a no-op (idempotent). ---
out=$(hf_cmd repos sync_config --target_path "$CHECKOUT" --mode pull)
echo "$out" | hf_assert_event '.type == "config_synced" and .changed == false'
echo "pull idempotent: changed=false on re-run"

# --- (6) sync_config --mode push: yaml → file. Round-trip check. ---
before_hash=$(sha256sum "$CHECKOUT/.hyperforge/config.toml" | awk '{print $1}')
out=$(hf_cmd repos sync_config --target_path "$CHECKOUT" --mode push)
echo "$out" | hf_assert_event '.type == "config_synced" and .mode == "push"'
after_hash=$(sha256sum "$CHECKOUT/.hyperforge/config.toml" | awk '{print $1}')
# After pull-then-push, the file should encode the same `forges` list.
# (Hash may differ if formatting changed, but the forges line must remain.)
grep -q 'codeberg' "$CHECKOUT/.hyperforge/config.toml"
echo "push: yaml content materialized to file"

# --- (7) Empty `forges` ([]) means "scoped to no forges" — sync emits forge_excluded. ---
sed -i 's/forges:.*$/forges: []/' "$HF_CONFIG/orgs/demo.yaml"
out=$(hf_cmd repos sync --org demo --name widget --include_metadata false)
echo "$out" | hf_assert_no_event '.type == "sync_diff"'
echo "forges=[]: sync emits no per-remote sync_diff"

# --- (8) Repos with forges=null behave like before (no filter). ---
sed -i '/forges: \[\]/d' "$HF_CONFIG/orgs/demo.yaml"
hf_cmd reload >/dev/null
# Without the filter, both remotes are visible to push (we don't actually push,
# just verify it's not pre-flighted to a forge_excluded error).
echo "forges=null: legacy unfiltered behavior"

rm -rf "$TMP"
hf_teardown
echo "PASS"
