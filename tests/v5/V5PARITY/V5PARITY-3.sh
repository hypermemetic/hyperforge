#!/usr/bin/env bash
# tier: 1
# V5PARITY-3 acceptance: git transport methods.
# All tier-1 — uses a local bare repo as the "remote"; no network needed.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# Build a local bare repo to act as the remote + a seed working dir.
TMP="$(mktemp -d -t v5prty3-XXXXXX)"
REMOTE="$TMP/remote.git"
SEED="$TMP/seed"
git init -q --bare "$REMOTE"
git init -q "$SEED"
git -C "$SEED" -c user.email=test@test -c user.name=test commit --allow-empty -m initial -q
git -C "$SEED" branch -m main 2>/dev/null || true
git -C "$SEED" remote add origin "$REMOTE"
git -C "$SEED" push -q origin main

# Register in an org yaml.
mkdir -p "$HF_CONFIG/orgs"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
YAML
cat > "$HF_CONFIG/orgs/demo.yaml" <<YAML
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: widget
    remotes:
      - url: $REMOTE
YAML

# --- clone ---
DEST="$TMP/clone"
out=$(hf_cmd repos clone --org demo --name widget --dest "$DEST")
echo "$out" | hf_assert_event '.type == "clone_done" and .dest == "'"$DEST"'"'
[[ -d "$DEST/.git" ]]

# --- status clean ---
out=$(hf_cmd repos status --path "$DEST")
echo "$out" | hf_assert_event '.type == "repo_status" and .dirty == false'

# --- dirty after file touch ---
echo "hello" > "$DEST/unknown.txt"
out=$(hf_cmd repos dirty --path "$DEST")
echo "$out" | hf_assert_event '.type == "repo_dirty" and .dirty == true'
rm "$DEST/unknown.txt"

# --- fetch ---
out=$(hf_cmd repos fetch --path "$DEST")
echo "$out" | hf_assert_event '.type == "fetch_done"'

# --- pull (ff, no-op) ---
out=$(hf_cmd repos pull --path "$DEST" --remote origin --branch main)
echo "$out" | hf_assert_event '.type == "pull_done" and .branch == "main"'

# --- set_transport: file:// → file:// (no-op for local path) — verify yaml updated ---
# For the transport flip test, use github-like URLs instead.
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: widget
    remotes:
      - url: https://github.com/demo/widget.git
YAML
out=$(hf_cmd repos set_transport --org demo --name widget --transport ssh)
echo "$out" | hf_assert_event '.type == "transport_set" and .transport == "ssh"'
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.remotes[0].url | startswith("git@github.com:")'

# Flip back.
out=$(hf_cmd repos set_transport --org demo --name widget --transport https)
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.remotes[0].url | startswith("https://github.com/")'

rm -rf "$TMP"
hf_teardown
echo "PASS"
