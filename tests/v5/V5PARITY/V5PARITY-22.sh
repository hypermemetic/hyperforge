#!/usr/bin/env bash
# tier: 1
# V5PARITY-22 acceptance: workspaces.from_org clones every (filtered)
# repo into a workspace path, registers them, partial-failure tolerant.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

TMP="$(mktemp -d -t v5prty22-XXXXXX)"

# Three local bare repos to act as remotes.
declare -A REMOTES
for r in alpha beta gamma; do
    REMOTES[$r]="$TMP/$r.git"
    git init -q --bare "${REMOTES[$r]}"
    git --git-dir="${REMOTES[$r]}" symbolic-ref HEAD refs/heads/main
    seed="$TMP/${r}-seed"
    git init -q "$seed"
    git -C "$seed" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
    git -C "$seed" branch -m main 2>/dev/null || true
    git -C "$seed" remote add origin "${REMOTES[$r]}"
    git -C "$seed" -c user.email=t@t -c user.name=t push -q origin main
done

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
  - name: alpha
    remotes:
      - url: ${REMOTES[alpha]}
  - name: beta
    remotes:
      - url: ${REMOTES[beta]}
  - name: gamma
    remotes:
      - url: ${REMOTES[gamma]}
YAML
hf_cmd reload >/dev/null

# --- (1) Full one-shot: workspace + clone all 3. ---
WS_PATH="$TMP/code"
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS_PATH")
echo "$out" | hf_assert_event ".type == \"workspace_created\" and .name == \"demo\" and .path == \"$WS_PATH\""
echo "$out" | hf_assert_event '.type == "member_added" and .ref.name == "alpha"'
echo "$out" | hf_assert_event '.type == "member_added" and .ref.name == "beta"'
echo "$out" | hf_assert_event '.type == "member_added" and .ref.name == "gamma"'
echo "$out" | hf_assert_event '.type == "workspace_git_summary" and .op == "from_org" and .total == 3 and .ok == 3'
[[ -d "$WS_PATH/alpha/.git" ]]
[[ -d "$WS_PATH/beta/.git" ]]
[[ -d "$WS_PATH/gamma/.git" ]]
[[ -f "$HF_CONFIG/workspaces/demo.yaml" ]]

# --- (2) Glob filter. ---
WS2="$TMP/code2"
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS2" --name selected --filter "alpha,gamma")
echo "$out" | hf_assert_event '.type == "workspace_git_summary" and .total == 2 and .ok == 2'
[[ -d "$WS2/alpha/.git" ]]
[[ -d "$WS2/gamma/.git" ]]
[[ ! -d "$WS2/beta" ]]

# --- (3) Wildcard filter. ---
WS3="$TMP/code3"
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS3" --name wild --filter "a*")
echo "$out" | hf_assert_event '.type == "workspace_git_summary" and .total == 1 and .ok == 1'
[[ -d "$WS3/alpha/.git" ]]

# --- (4) --clone false: workspace yaml + members but no checkouts. ---
WS4="$TMP/code4"
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS4" --name no-clone --clone false)
echo "$out" | hf_assert_event '.type == "workspace_created"'
echo "$out" | hf_assert_event '.type == "workspace_git_summary" and .ok == 0 and .errored == 0'
[[ -f "$HF_CONFIG/workspaces/no-clone.yaml" ]]
[[ ! -d "$WS4/alpha/.git" ]]

# --- (5) Re-run on existing path: idempotent (members not re-cloned). ---
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS_PATH")
echo "$out" | hf_assert_event '.type == "member_added" and .already_present == true'

# --- (6) Filter that matches zero → validation error. ---
WS6="$TMP/code6"
out=$(hf_cmd workspaces from_org --org demo --target_path "$WS6" --filter "nope-*")
echo "$out" | hf_assert_event '.type == "error" and .code == "validation"'

# --- (7) Unknown org → not_found. ---
out=$(hf_cmd workspaces from_org --org missing --target_path "$TMP/missing")
echo "$out" | hf_assert_event '.type == "error" and .code == "not_found"'

rm -rf "$TMP"
hf_teardown
echo "PASS"
