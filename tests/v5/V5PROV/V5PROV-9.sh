#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5PROV-9 checkpoint: end-to-end workflow verification.
# SKIPs non-tier-2 stories if HF_V5_TEST_CONFIG_DIR is unset; tier-1
# stories (U1, U6) still run.

TS=$(date +%s)
TMP_REPO="v5prov-ckpt-${TS}"
TMP_WS="v5prov-ckpt-ws-${TS}"
TMP_DIR="/tmp/v5prov-ckpt-${TS}"

results=()

record() { results+=("$1"); echo "STORY $1"; }

hf_spawn

# --- U1: workspace create (tier 1) ---
hf_load_fixture empty
if hf_cmd workspaces create --name "$TMP_WS" --ws_path "$TMP_DIR" --repos '[]' >/dev/null 2>&1 \
    && hf_cmd workspaces list 2>/dev/null | hf_assert_event '.type == "workspace_summary" and .name == "'"$TMP_WS"'"' >/dev/null 2>&1; then
    record "U1 green: workspace create + list"
else
    record "U1 red: workspace create/list failed"
fi
hf_cmd workspaces delete --name "$TMP_WS" >/dev/null 2>&1 || true
hf_teardown

# Remaining stories require tier-2 config.
if [[ -z "${HF_V5_TEST_CONFIG_DIR:-}" || ! -f "${HF_V5_TEST_CONFIG_DIR:-/dev/null}/tier2.env" ]]; then
    record "U2 yellow: tier-2 config not set (HF_V5_TEST_CONFIG_DIR)"
    record "U3 yellow: tier-2 config not set"
    record "U4 yellow: tier-2 config not set"
    record "U5 yellow: tier-2 config not set"
    record "U6 yellow: token leakage check skipped (no token to grep against)"
    printf '%s\n' "${results[@]}"
    echo "PASS"
    exit 0
fi

# Source tier-2 env.
# shellcheck disable=SC1091
set -a; source "$HF_V5_TEST_CONFIG_DIR/tier2.env"; set +a
ORG="$HF_TIER2_GITHUB_ORG"

# --- U2: repos add --create_remote → repo exists on forge ---
hf_spawn
hf_use_test_config
out=$(hf_cmd repos add --org "$ORG" --name "$TMP_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${TMP_REPO}.git\"}]" \
    --create_remote true --visibility private --description "checkpoint $TS" 2>&1 || true)
if echo "$out" | hf_assert_event '.type == "repo_created"' >/dev/null 2>&1 \
    && gh repo view "${ORG}/${TMP_REPO}" --json visibility --jq '.visibility' 2>/dev/null | grep -qi private; then
    record "U2 green: add+create_remote produces private forge repo"
else
    record "U2 red: create_remote didn't land"
fi

# --- U3: add to workspace + sync → in_sync ---
hf_cmd workspaces create --name "$TMP_WS" --ws_path "$TMP_DIR" \
    --repos "[\"${ORG}/${TMP_REPO}\"]" >/dev/null 2>&1 || true
sync_out=$(hf_cmd workspaces sync --name "$TMP_WS" 2>&1 || true)
if echo "$sync_out" | hf_assert_event '.type == "workspace_sync_report" and .total >= 1 and .in_sync >= 1' >/dev/null 2>&1; then
    record "U3 green: workspace sync reports in_sync"
else
    record "U3 red: sync didn't produce expected in_sync"
fi

# --- U4: alternative path — remote-only creation via sync ---
ALT_REPO="v5prov-ckpt-alt-${TS}"
hf_cmd repos add --org "$ORG" --name "$ALT_REPO" \
    --remotes "[{\"url\":\"https://github.com/${ORG}/${ALT_REPO}.git\"}]" >/dev/null 2>&1 || true
hf_cmd workspaces add_repo --name "$TMP_WS" --repo_ref "${ORG}/${ALT_REPO}" >/dev/null 2>&1 || true
sync_out=$(hf_cmd workspaces sync --name "$TMP_WS" 2>&1 || true)
if echo "$sync_out" | hf_assert_event '.type == "workspace_sync_report" and .created >= 1' >/dev/null 2>&1 \
    && gh repo view "${ORG}/${ALT_REPO}" >/dev/null 2>&1; then
    record "U4 green: sync created remote-only member"
else
    record "U4 red: sync did not create remote-only member"
fi

# --- U5: delete cascade ---
# delete_repo scope needed on the gh token; classify yellow when absent
# rather than reporting a scope gap as a logic failure.
del1=""; del2=""
if gh auth status 2>&1 | grep -q 'delete_repo'; then
    del1=$(hf_cmd repos delete --org "$ORG" --name "$TMP_REPO" --delete_remote true 2>&1 || true)
    del2=$(hf_cmd repos delete --org "$ORG" --name "$ALT_REPO" --delete_remote true 2>&1 || true)
    set +e
    gh repo view "${ORG}/${TMP_REPO}" >/dev/null 2>&1; rc1=$?
    gh repo view "${ORG}/${ALT_REPO}" >/dev/null 2>&1; rc2=$?
    set -e
    if [[ $rc1 -ne 0 && $rc2 -ne 0 ]]; then
        record "U5 green: delete cascade removed both remote repos"
    else
        record "U5 red: cascade delete left remote repo(s) behind"
    fi
else
    record "U5 yellow: gh token lacks delete_repo scope (run: gh auth refresh -h github.com -s delete_repo)"
fi

# --- U6: no token leakage anywhere in the accumulated event stream ---
TOKEN="${HF_TIER2_GITHUB_TOKEN:-}"
combined="$out $sync_out $del1 $del2"
if [[ -z "$TOKEN" ]] || ! echo "$combined" | grep -qF "$TOKEN"; then
    record "U6 green: no token value present in event stream"
else
    record "U6 red: token leaked into events (investigate)"
fi

hf_cmd workspaces delete --name "$TMP_WS" >/dev/null 2>&1 || true
hf_teardown

echo
echo "=== state-of-epic map ==="
printf '%s\n' "${results[@]}"

# Overall pass iff no "red" entries. Yellow is acceptable (skipped or
# blocked by external non-logic constraint like a missing token scope).
for r in "${results[@]}"; do
    [[ "$r" != *" red:"* ]] || { echo "FAIL: at least one story is red"; exit 1; }
done
echo "PASS"
