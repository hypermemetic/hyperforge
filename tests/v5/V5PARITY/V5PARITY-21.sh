#!/usr/bin/env bash
# tier: 1
# V5PARITY-21 acceptance: orgs.bootstrap composes secrets.set + create
# + set_credential + repos.import in one RPC. Token forms: raw,
# env://VAR, gh-token:// (latter exercised against a stub gh on PATH).
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# --- (1) Raw token, --import false → secret + org + cred + done. ---
out=$(hf_cmd orgs bootstrap \
    --name foo --provider github \
    --token "raw-token-1234" \
    --import false)
echo "$out" | hf_assert_event '.type == "secret_set" and .key == "secrets://github/foo/token"'
echo "$out" | hf_assert_event '.type == "org_summary" and .name == "foo" and .provider == "github"'
echo "$out" | hf_assert_event '.type == "credential_added"'
echo "$out" | hf_assert_event '.type == "bootstrap_done" and .org == "foo" and .repos_added == 0'
[[ -f "$HF_CONFIG/orgs/foo.yaml" ]]
grep -q 'secrets://github/foo/token' "$HF_CONFIG/orgs/foo.yaml"

# --- (2) --use_default_token true → cred references the _default path. ---
out=$(hf_cmd orgs bootstrap \
    --name bar --provider github \
    --token "shared-token" \
    --use_default_token true \
    --import false)
echo "$out" | hf_assert_event '.type == "secret_set" and .key == "secrets://github/_default/token"'
grep -q 'secrets://github/_default/token' "$HF_CONFIG/orgs/bar.yaml"

# --- (3) env:// form. The daemon resolves env vars in its own
# process, so we need to spawn it with the var pre-set.
hf_teardown
HF_TEST_TOKEN_FOR_BOOTSTRAP="from-env-var" hf_spawn
out=$(hf_cmd orgs bootstrap \
    --name baz --provider github \
    --token "env://HF_TEST_TOKEN_FOR_BOOTSTRAP" \
    --import false)
echo "$out" | hf_assert_event '.type == "bootstrap_done" and .org == "baz"'

# --- (4) gh-token:// against a stub gh. ---
TMP="$(mktemp -d -t v5prty21-stub-XXXXXX)"
mkdir -p "$TMP/bin"
cat > "$TMP/bin/gh" <<'GHSTUB'
#!/usr/bin/env bash
# Match on "$*" so single-arg invocations like `gh --version` work
# without trailing-space quirks.
case "$*" in
    "auth status") echo "github.com" >&2; echo "  ✓ Logged in to github.com account stub-user (keyring)" >&2; echo "  - Token scopes: 'repo'" >&2; exit 0 ;;
    "auth token")  echo "stub-token-from-gh"; exit 0 ;;
    "--version")   echo "gh version 999.0.0"; exit 0 ;;
    *) echo "stub: unhandled $*" >&2; exit 1 ;;
esac
GHSTUB
chmod +x "$TMP/bin/gh"

# Spawn a fresh daemon with the stub PATH exported so the daemon
# inherits it (bash functions don't preserve `VAR=val func` env).
hf_teardown
export PATH="$TMP/bin:$PATH"
hf_spawn
out=$(hf_cmd orgs bootstrap \
    --name qux --provider github \
    --token "gh-token://" \
    --import false)
echo "$out" | hf_assert_event '.type == "secret_set" and .value_length == 19'  # len("stub-token-from-gh")
echo "$out" | hf_assert_event '.type == "bootstrap_done" and .org == "qux"'

# --- (5) gh-token:// when gh is missing → bootstrap_failed { stage: token_resolve }. ---
# Skipped when the host has a real gh installed — that's the normal
# dev environment. If you want to exercise this path, run the test
# from a PATH that excludes gh.
if ! type -P gh >/dev/null; then
    hf_teardown
    hf_spawn
    out=$(hf_cmd orgs bootstrap \
        --name nogh --provider github \
        --token "gh-token://" \
        --import false 2>&1)
    echo "$out" | hf_assert_event '.type == "bootstrap_failed" and .stage == "token_resolve"'
    echo "no-gh path: bootstrap_failed { stage: token_resolve }"
else
    echo "no-gh path: skipped (real gh installed)"
fi

# --- (6) Re-running on existing org: idempotent. ---
out=$(hf_cmd orgs bootstrap \
    --name foo --provider github \
    --token "raw-token-1234" \
    --import false)
echo "$out" | hf_assert_event '.type == "bootstrap_done" and .org == "foo"'

# --- (7) --dry_run true: no state mutation. ---
before_hash=$(sha256sum "$HF_CONFIG/orgs/foo.yaml" | awk '{print $1}')
out=$(hf_cmd orgs bootstrap \
    --name foo --provider github \
    --token "different-token" \
    --import false \
    --dry_run true)
after_hash=$(sha256sum "$HF_CONFIG/orgs/foo.yaml" | awk '{print $1}')
[[ "$before_hash" == "$after_hash" ]] || { echo "FAIL: dry_run mutated state" >&2; exit 1; }
echo "dry_run: state unchanged"

rm -rf "$TMP"
hf_teardown
echo "PASS"
