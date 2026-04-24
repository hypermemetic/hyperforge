#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_require_tier2 github

hf_spawn
hf_use_test_config

ORG="$HF_TIER2_GITHUB_ORG"
TS=$(date +%s)

# --- dismissed member is skipped in sync by default ---
ACT_REPO="v5life-10a-${TS}"
DIS_REPO="v5life-10b-${TS}"
WS="v5life-10-ws-${TS}"

hf_cmd repos add --org "$ORG" --name "$ACT_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${ACT_REPO}.git\"}]" \
    --create_remote true --visibility private >/dev/null
hf_cmd repos add --org "$ORG" --name "$DIS_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${DIS_REPO}.git\"}]" \
    --create_remote true --visibility private >/dev/null
hf_cmd repos delete --org "$ORG" --name "$DIS_REPO" >/dev/null   # soft-delete

hf_cmd workspaces create --name "$WS" --ws_path "/tmp/$WS" \
    --repos "[\"${ORG}/${ACT_REPO}\",\"${ORG}/${DIS_REPO}\"]" >/dev/null

out=$(hf_cmd workspaces sync --name "$WS")
echo "$out" | hf_assert_event '.type == "sync_skipped" and .ref.name == "'"$DIS_REPO"'" and .reason == "dismissed"'
echo "$out" | hf_assert_event '.type == "sync_diff" and .ref.name == "'"$ACT_REPO"'"'
echo "$out" | hf_assert_event '.type == "workspace_sync_report" and .total == 2 and .skipped == 1'

# --- include_dismissed reaches the dismissed member too ---
out2=$(hf_cmd workspaces sync --name "$WS" --include_dismissed true)
echo "$out2" | hf_assert_count '.type == "sync_diff"' 2
echo "$out2" | hf_assert_event '.type == "workspace_sync_report" and .skipped == 0'

# --- config_drift: a disk dir with .hyperforge/config.toml declaring a
#     different identity than the workspace member. ---
DRIFT_DIR="/tmp/$WS/$DIS_REPO"
mkdir -p "$DRIFT_DIR/.hyperforge"
cat > "$DRIFT_DIR/.hyperforge/config.toml" <<TOML
repo_name = "somebody-else"
org = "other-org"
forges = ["github"]
TOML
git -C "$DRIFT_DIR" init -q 2>/dev/null || true
git -C "$DRIFT_DIR" remote add origin "https://github.com/${ORG}/${DIS_REPO}.git" 2>/dev/null || true

rc_out=$(hf_cmd workspaces reconcile --name "$WS")
echo "$rc_out" | hf_assert_event '.kind == "config_drift" or .type == "config_drift"'

# Cleanup: un-dismiss via direct yaml so cleanup can cascade (purge if scope available).
if gh auth status 2>&1 | grep -q 'delete_repo'; then
    hf_cmd repos purge --org "$ORG" --name "$ACT_REPO" >/dev/null 2>&1 || true
    hf_cmd repos purge --org "$ORG" --name "$DIS_REPO" >/dev/null 2>&1 || true
fi
hf_cmd workspaces delete --name "$WS" >/dev/null 2>&1 || true

hf_teardown
echo "PASS"
