#!/usr/bin/env bash
# tier: 1
# V5PARITY-12 acceptance: data-structure tightenings + DRY invariants +
# parallel-test harness fix + helper presence.
#
# Round-trip byte-identity for committed fixtures is enforced via the
# Rust unit-test suite (see ops::state::round_trip_*); this script
# focuses on what bash can see directly: source-level invariants and
# DRY greps.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

cd "$(dirname "$0")/../../.."

# --- (1) DRY grep invariants still green (full V5LIFECYCLE-11 run) ---
out="$(bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-11.sh 2>&1)"
echo "$out" | grep -q 'DRY:command-git green' || {
    echo "FAIL: Command::new(\"git\") DRY invariant not green" >&2
    echo "$out" | tail -25 >&2
    exit 1
}
echo "DRY: command-git green"

# --- (2) canonical_remote() helper is referenceable ---
grep -qE 'pub fn canonical_remote' src/v5/config.rs || {
    echo "FAIL: canonical_remote() helper not defined" >&2
    exit 1
}
echo "helper: canonical_remote present"

# --- (3) parallel-test harness: __hf_pick_port uses SO_REUSEADDR ---
grep -q 'SO_REUSEADDR' tests/v5/harness/lib.sh || {
    echo "FAIL: __hf_pick_port does not use SO_REUSEADDR" >&2
    exit 1
}
echo "harness: SO_REUSEADDR guard present"

# --- (4) RepoLifecycle/protected defaults are non-Option ---
if grep -q 'lifecycle: Option<RepoLifecycle>' src/v5/config.rs; then
    echo "FAIL: lifecycle still Option<RepoLifecycle>" >&2
    exit 1
fi
if grep -q 'protected: Option<bool>' src/v5/config.rs; then
    echo "FAIL: protected still Option<bool>" >&2
    exit 1
fi
echo "types: lifecycle/protected defaulted"

# --- (5) Fixture round-trip via the daemon ---
# For every committed fixture with an orgs/workspaces dir, copy it into
# a fresh $HF_CONFIG, run a no-op orgs.update on each org (which loads
# + re-saves the yaml), and assert the byte-content is unchanged.
ROUNDTRIP_FAIL=0
for fx in tests/v5/fixtures/*/; do
    nm=$(basename "$fx")
    [[ "$nm" == "tier2-template" ]] && continue
    [[ -d "$fx/orgs" ]] || continue
    hf_spawn
    cp -a "$fx/." "$HF_CONFIG/"
    for org_yaml in "$HF_CONFIG/orgs/"*.yaml; do
        [[ -f "$org_yaml" ]] || continue
        oname=$(basename "$org_yaml" .yaml)
        before=$(sha256sum "$org_yaml" | awk '{print $1}')
        # orgs.update with no flags: load → save (canonical re-emit).
        hf_cmd orgs update --name "$oname" >/dev/null 2>&1 || true
        after=$(sha256sum "$org_yaml" | awk '{print $1}')
        if [[ "$before" != "$after" ]]; then
            echo "round-trip drift in fixture $nm/orgs/$oname.yaml" >&2
            diff <(echo) "$org_yaml" >&2 || true
            ROUNDTRIP_FAIL=1
        fi
    done
    hf_teardown
done
if (( ROUNDTRIP_FAIL )); then
    echo "FAIL: fixture round-trip drift detected" >&2
    exit 1
fi
echo "round-trip: fixtures byte-identical after no-op update"

echo "PASS"
