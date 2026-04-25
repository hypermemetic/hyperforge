#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5LIFECYCLE-11: checkpoint. User-story matrix + DRY grep invariant.
# Tier 1 portion (grep + .hyperforge init) runs always.
# Tier 2 portion (soft-delete/purge round-trip) gated on HF_V5_TEST_CONFIG_DIR.

results=()
record() { results+=("$1"); echo "STORY $1"; }

# --- DRY grep invariants (always tier 1) ---
cd "$(dirname "$0")/../../.."

grep_violation() {
    local label="$1"
    local pattern="$2"
    local exclude="$3"
    local hits
    # Always also exclude doc-comment lines (`///`) — textual mentions
    # of adapter methods in documentation aren't actual calls.
    hits=$(grep -RnE "$pattern" src/v5/ 2>/dev/null \
        | grep -vE "$exclude" \
        | grep -vE '^[^:]+:[0-9]+:\s*///' \
        | grep -vE '^[^:]+:[0-9]+:.*\s*//[^/]' \
        || true)
    if [[ -z "$hits" ]]; then
        record "DRY:$label green"
    else
        record "DRY:$label red — $(echo "$hits" | head -n1)"
    fi
}

# DRY invariants — tightened to match V5LIFECYCLE-{2,3,4}. See those
# scripts for the reasoning behind each exclusion.
#   workspaces.rs has 5 inline workspace-yaml loads that are a known
#   follow-up migration; excluded from yaml-io.
#   config.rs owns the yaml type defs + loader impls.
#   secrets.rs is the YAML-backed secret store (separate from orgs/ws state).
#   /// doc comments can mention adapter methods without it being a call.
grep_violation "yaml-io"      'serde_yaml::(from_str|to_string|from_reader)' '^src/v5/(ops|secrets|config\.rs)'
grep_violation "adapter-meta" '[^/]adapter\.(read_metadata|write_metadata)' '^src/v5/ops/'
grep_violation "adapter-life" '[^/]adapter\.(create_repo|delete_repo|repo_exists|update_repo)' '^src/v5/ops/'
grep_violation "for_provider" '[^a-z:]for_provider\(' '^src/v5/(ops|adapters)/'
grep_violation "compute_drift" '[^a-z_]compute_drift\(' '^src/v5/ops/'
# V5PARITY-12 new invariant: `Command::new("git")` lives in ops/git/* only.
# V5PARITY-15 widened the path scope from a single file to the dir after
# the module split into mod.rs / subprocess.rs / local.rs.
grep_violation "command-git"  'Command::new\("git"\)' '^src/v5/ops/git/'

# --- U5: .hyperforge init (always tier 1) ---
TMP="$(mktemp -d -t v5life-ckpt-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
hf_spawn
hf_load_fixture "empty"
if hf_cmd repos init --target_path "$TMP" --org demo --repo_name checkpoint \
    --forges '["github"]' --visibility private >/dev/null 2>&1 \
    && [[ -f "$TMP/.hyperforge/config.toml" ]]; then
    record "U5 green: repos.init writes .hyperforge/config.toml"
else
    record "U5 red: repos.init failed"
fi
hf_teardown

# --- Tier-2 stories ---
if [[ -z "${HF_V5_TEST_CONFIG_DIR:-}" || ! -f "${HF_V5_TEST_CONFIG_DIR}/tier2.env" ]]; then
    for s in U1 U2 U3 U4 U6 U7; do
        record "$s yellow: tier-2 config not set"
    done
else
    set -a; source "$HF_V5_TEST_CONFIG_DIR/tier2.env"; set +a
    ORG="$HF_TIER2_GITHUB_ORG"
    TS=$(date +%s)
    CKPT_REPO="v5life-ckpt-${TS}"

    hf_spawn
    hf_use_test_config

    # U1: soft-delete
    hf_cmd repos add --org "$ORG" --name "$CKPT_REPO" \
        --remotes "[{\"url\":\"https://github.com/${ORG}/${CKPT_REPO}.git\"}]" \
        --create_remote true --visibility public >/dev/null 2>&1 || true
    if hf_cmd repos delete --org "$ORG" --name "$CKPT_REPO" 2>/dev/null | \
        hf_assert_event '.type == "repo_dismissed"' >/dev/null 2>&1 \
       && gh repo view "${ORG}/${CKPT_REPO}" --json visibility --jq '.visibility' 2>/dev/null | grep -qi private; then
        record "U1 green: soft-delete privatizes + dismisses"
    else
        record "U1 red: soft-delete failed"
    fi

    # U2: dismissed still listable
    if hf_cmd repos list --org "$ORG" 2>/dev/null | \
        hf_assert_event '.name == "'"$CKPT_REPO"'"' >/dev/null 2>&1; then
        record "U2 green: dismissed repo still listable"
    else
        record "U2 red: dismissed repo missing from list"
    fi

    # U3: purge (requires delete_repo scope)
    if gh auth status 2>&1 | grep -q 'delete_repo'; then
        if hf_cmd repos purge --org "$ORG" --name "$CKPT_REPO" 2>/dev/null | \
            hf_assert_event '.type == "repo_purged"' >/dev/null 2>&1 \
           && ! gh repo view "${ORG}/${CKPT_REPO}" >/dev/null 2>&1; then
            record "U3 green: purge cascades to forge 404"
        else
            record "U3 red: purge didn't remove remote"
        fi
    else
        record "U3 yellow: gh token lacks delete_repo scope"
    fi

    # U4: protection blocks delete + purge
    PROT_REPO="v5life-ckpt-prot-${TS}"
    hf_cmd repos add --org "$ORG" --name "$PROT_REPO" \
        --remotes "[{\"url\":\"https://github.com/${ORG}/${PROT_REPO}.git\"}]" \
        --create_remote true --visibility private >/dev/null 2>&1 || true
    hf_cmd repos protect --org "$ORG" --name "$PROT_REPO" --protected true >/dev/null 2>&1 || true
    err_del=$(hf_cmd repos delete --org "$ORG" --name "$PROT_REPO" 2>&1 || true)
    err_prg=$(hf_cmd repos purge --org "$ORG" --name "$PROT_REPO" 2>&1 || true)
    if echo "$err_del" | grep -qi protect && echo "$err_prg" | grep -qi protect; then
        record "U4 green: protection blocks delete and purge"
    else
        record "U4 red: protection did not block both paths"
    fi
    hf_cmd repos protect --org "$ORG" --name "$PROT_REPO" --protected false >/dev/null 2>&1 || true
    hf_cmd repos delete --org "$ORG" --name "$PROT_REPO" >/dev/null 2>&1 || true
    if gh auth status 2>&1 | grep -q 'delete_repo'; then
        hf_cmd repos purge --org "$ORG" --name "$PROT_REPO" >/dev/null 2>&1 || true
    fi

    # U6 + U7: workspace config_drift + sync skips dismissed
    # Delegate to V5LIFECYCLE-10.sh if available, or assert inline.
    if bash "$(dirname "$0")/V5LIFECYCLE-10.sh" >/dev/null 2>&1; then
        record "U6 green: config_drift surfaced"
        record "U7 green: dismissed skipped in sync"
    else
        record "U6 red: config_drift / sync-skip assertion failed"
        record "U7 red: (see U6)"
    fi

    hf_teardown
fi

echo
echo "=== state-of-epic map ==="
printf '%s\n' "${results[@]}"

for r in "${results[@]}"; do
    [[ "$r" != *" red"* ]] || { echo "FAIL: at least one story is red"; exit 1; }
done
echo "PASS"
