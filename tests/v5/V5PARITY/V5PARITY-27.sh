#!/usr/bin/env bash
# tier: 1
# V5PARITY-27 acceptance: external-auth ops module is the single
# subprocess source for forge CLIs; tokens stay out of Debug/logs.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

cd "$(dirname "$0")/../../.."

# --- (1) module structure ---
for f in src/v5/ops/external_auth/mod.rs src/v5/ops/external_auth/gh.rs; do
    [[ -f "$f" ]] || { echo "FAIL: missing $f" >&2; exit 1; }
done
echo "module: external_auth/{mod,gh}.rs present"

# --- (2) DRY invariant — no `Command::new("gh")` outside ops/external_auth ---
hits=$(grep -RnE 'Command::new\("gh"\)' src/v5/ 2>/dev/null \
    | grep -vE '^src/v5/ops/external_auth/' \
    | grep -vE '^[^:]+:[0-9]+:\s*///' \
    || true)
if [[ -n "$hits" ]]; then
    echo "FAIL: Command::new(\"gh\") outside ops/external_auth/:" >&2
    echo "$hits" >&2
    exit 1
fi
echo "DRY: command-gh confined to ops/external_auth/"

# --- (3) Rust unit tests for the module pass ---
out=$(cargo test --lib --quiet ops::external_auth 2>&1 || true)
if echo "$out" | grep -qE "test result: ok\."; then
    echo "rust tests: ok"
else
    echo "FAIL: cargo test ops::external_auth did not pass" >&2
    echo "$out" | tail -20 >&2
    exit 1
fi

# --- (4) Provider Display works ---
grep -q "impl std::fmt::Display for ExternalAuthProvider" src/v5/ops/external_auth/mod.rs \
    || { echo "FAIL: missing Display impl" >&2; exit 1; }
echo "Display: ExternalAuthProvider has typed display"

echo "PASS"
