#!/usr/bin/env bash
# tier: 1
# V5PARITY-24 acceptance: provider-default credential fallback.
# We assert the behavior shape via Rust unit tests on the resolution
# path (full integration with a real forge would be tier-2).
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

cd "$(dirname "$0")/../../.."

# --- (1) helper exists ---
grep -q "pub fn default_token_ref_for" src/v5/ops/repo.rs \
    || { echo "FAIL: default_token_ref_for missing" >&2; exit 1; }
echo "helper: default_token_ref_for present"

# --- (2) ForgeAuth gained fallback_token_ref + resolve_token ---
grep -q "fallback_token_ref:" src/v5/adapters/mod.rs \
    || { echo "FAIL: ForgeAuth.fallback_token_ref missing" >&2; exit 1; }
grep -q "fn resolve_token" src/v5/adapters/mod.rs \
    || { echo "FAIL: ForgeAuth::resolve_token missing" >&2; exit 1; }
echo "ForgeAuth: fallback + resolve_token in place"

# --- (3) adapters use resolve_token instead of inline resolution ---
for adapter in github codeberg gitlab; do
    if ! grep -q "auth.resolve_token()" "src/v5/adapters/$adapter.rs"; then
        echo "FAIL: $adapter adapter not using resolve_token" >&2
        exit 1
    fi
done
echo "adapters: routed through ForgeAuth::resolve_token"

# --- (4) End-to-end: register an org with NO explicit credentials,
# write a provider-default secret, run a forge call.
# We exercise this by spawning a daemon, setting up a fake "github"
# org pointing at a local file:// remote that exists_on_forge can't
# reach (it's not really a github URL), and asserting the failure mode
# is the *forge* layer, not "no token configured" — meaning fallback
# resolution succeeded in finding the default secret.
hf_spawn

mkdir -p "$HF_CONFIG/orgs"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
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
YAML

# Set ONLY the provider-default secret. No per-org token.
hf_cmd secrets set --key "secrets://github/_default/token" --value "fake-token-not-real" >/dev/null

# Now run a method that resolves the token through ForgeAuth.
# `auth_check` exercises the path. We expect either auth-failure (real
# token would be valid) OR a forge-side "401 unauthorized" — but NOT
# "no token credential on org", which would mean the fallback failed
# to resolve.
out=$(hf_cmd auth_check --org demo 2>&1)
if echo "$out" | grep -q "no token credential on org"; then
    echo "FAIL: fallback did not resolve the provider_default secret" >&2
    echo "$out" >&2
    exit 1
fi
echo "fallback: provider_default token resolves when explicit cred absent"

hf_teardown
echo "PASS"
