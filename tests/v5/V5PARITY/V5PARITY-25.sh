#!/usr/bin/env bash
# tier: 1
# V5PARITY-25 acceptance: repos.register adopts an existing checkout.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

TMP="$(mktemp -d -t v5prty25-XXXXXX)"
CHECKOUT="$TMP/widget"

# Fixture: a real checkout with origin pointing at a github URL.
git init -q "$CHECKOUT"
git -C "$CHECKOUT" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$CHECKOUT" remote add origin https://github.com/demo/widget.git
git -C "$CHECKOUT" remote add mirror https://codeberg.org/demo/widget.git

# Provider map + an existing demo org.
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
repos: []
YAML
hf_cmd reload >/dev/null

# --- (1) Auto-derive org + name from origin URL. ---
out=$(hf_cmd repos register --target_path "$CHECKOUT")
echo "$out" | hf_assert_event '.type == "repo_registered" and .ref.org == "demo" and .ref.name == "widget"'
echo "$out" | hf_assert_event '.init_done == true'
[[ -f "$CHECKOUT/.hyperforge/config.toml" ]]
grep -q 'demo' "$HF_CONFIG/orgs/demo.yaml"
grep -q 'widget' "$HF_CONFIG/orgs/demo.yaml"
# Both remotes captured (origin + mirror).
grep -q 'github.com/demo/widget.git' "$HF_CONFIG/orgs/demo.yaml"
grep -q 'codeberg.org/demo/widget.git' "$HF_CONFIG/orgs/demo.yaml"

# --- (2) Re-register is idempotent. ---
out=$(hf_cmd repos register --target_path "$CHECKOUT")
echo "$out" | hf_assert_event '.type == "repo_registered"'

# --- (3) --init false skips .hyperforge/config.toml. ---
rm -rf "$CHECKOUT/.hyperforge"
out=$(hf_cmd repos register --target_path "$CHECKOUT" --init false)
echo "$out" | hf_assert_event '.type == "repo_registered" and .init_done == false'
[[ ! -f "$CHECKOUT/.hyperforge/config.toml" ]]

# --- (4) Conflict: same repo name, different remotes. ---
CHECKOUT2="$TMP/widget2"
git init -q "$CHECKOUT2"
git -C "$CHECKOUT2" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$CHECKOUT2" remote add origin https://github.com/demo/widget.git
git -C "$CHECKOUT2" remote add upstream https://github.com/different-fork/widget.git
out=$(hf_cmd repos register --target_path "$CHECKOUT2")
echo "$out" | hf_assert_event '.type == "repo_conflict" and .ref.name == "widget"'

# --- (5) No matching provider_map → validation error. ---
CHECKOUT3="$TMP/orphan"
git init -q "$CHECKOUT3"
git -C "$CHECKOUT3" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$CHECKOUT3" remote add origin https://unknown.example.com/foo/orphan.git
out=$(hf_cmd repos register --target_path "$CHECKOUT3")
echo "$out" | hf_assert_event '.type == "error" and .code == "validation"'

# --- (6) Org override. ---
CHECKOUT4="$TMP/widget-fork"
git init -q "$CHECKOUT4"
git -C "$CHECKOUT4" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$CHECKOUT4" remote add origin https://github.com/demo/widget-fork.git
out=$(hf_cmd repos register --target_path "$CHECKOUT4" --repo_name custom-name)
echo "$out" | hf_assert_event '.type == "repo_registered" and .ref.name == "custom-name"'

rm -rf "$TMP"
hf_teardown
echo "PASS"
