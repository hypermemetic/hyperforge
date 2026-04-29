#!/usr/bin/env bash
# tier: 1
# V5PARITY-23 acceptance: providerless `auth_requirements_for` +
# `auth_detect_external` (no org context required).
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- (1) auth_requirements_for surfaces static scopes ---
hf_spawn
out=$(hf_cmd auth_requirements_for --provider github)
echo "$out" | hf_assert_event '.type == "auth_requirements_for" and .provider == "github"'
echo "$out" | hf_assert_event '.required_scopes | index("repo")'
echo "$out" | hf_assert_event '.required_scopes | index("read:org")'
echo "$out" | hf_assert_event '.recommended_scopes | index("delete_repo")'

out=$(hf_cmd auth_requirements_for --provider codeberg)
echo "$out" | hf_assert_event '.type == "auth_requirements_for" and .provider == "codeberg"'
out=$(hf_cmd auth_requirements_for --provider gitlab)
echo "$out" | hf_assert_event '.type == "auth_requirements_for" and .provider == "gitlab"'

out=$(hf_cmd auth_requirements_for --provider zoid)
echo "$out" | hf_assert_event '.type == "error" and .code == "validation"'

# --- (2) auth_detect_external against a stub gh ---
TMP="$(mktemp -d -t v5prty23-XXXXXX)"
mkdir -p "$TMP/bin"
cat > "$TMP/bin/gh" <<'GHSTUB'
#!/usr/bin/env bash
case "$*" in
    "auth status")
        echo "github.com" >&2
        echo "  ✓ Logged in to github.com account stub-user (keyring)" >&2
        echo "  - Token scopes: 'repo', 'read:org'" >&2
        exit 0 ;;
    "auth token") echo "stub-token"; exit 0 ;;
    "api /user/orgs --jq .[].login")
        printf 'stub-org\nanother-org\n'; exit 0 ;;
    "--version") echo "gh stub"; exit 0 ;;
esac
exit 1
GHSTUB
chmod +x "$TMP/bin/gh"

hf_teardown
export PATH="$TMP/bin:$PATH"
hf_spawn
out=$(hf_cmd auth_detect_external --provider github)
echo "$out" | hf_assert_event '.type == "external_auth_detected" and .provider == "github" and .logged_in == true'
echo "$out" | hf_assert_event '.username == "stub-user"'
echo "$out" | hf_assert_event '.scopes | index("repo")'
echo "$out" | hf_assert_event '.accessible_orgs | index("stub-org")'
if echo "$out" | grep -q "stub-token"; then
    echo "FAIL: token leaked into auth_detect_external event payload" >&2
    exit 1
fi
echo "no-token-leak: confirmed"

rm -rf "$TMP"
hf_teardown
echo "PASS"
