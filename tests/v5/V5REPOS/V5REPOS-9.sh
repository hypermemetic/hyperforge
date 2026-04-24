#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

: "${HF_TEST_GITHUB_ORG:=}"
: "${HF_TEST_GITHUB_REPO:=}"
: "${HF_TEST_GITHUB_TOKEN:=}"

if [[ -z "$HF_TEST_GITHUB_ORG" || -z "$HF_TEST_GITHUB_REPO" || -z "$HF_TEST_GITHUB_TOKEN" ]]; then
  echo "SKIP: tier 2 env not set (HF_TEST_GITHUB_ORG/_REPO/_TOKEN)"
  exit 0
fi

TS=$(date +%s)
STAMP="hyperforge-v5-repos-9 $TS"

hf_spawn

# Build a per-test fixture pointing at the live repo.
mkdir -p "$HF_CONFIG/orgs"
cat > "$HF_CONFIG/config.yaml" <<YAML
provider_map:
  github.com: github
YAML
cat > "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" <<YAML
name: $HF_TEST_GITHUB_ORG
forge:
  provider: github
  credentials:
    - key: secrets://gh-token
      type: token
repos:
  - name: $HF_TEST_GITHUB_REPO
    remotes:
      - url: https://github.com/${HF_TEST_GITHUB_ORG}/${HF_TEST_GITHUB_REPO}.git
YAML
hf_put_secret "secrets://gh-token" "$HF_TEST_GITHUB_TOKEN"

# --- read capability: exact four-field shape ---
out=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
# Exactly the four DriftFieldKind keys must appear (via sync's remote snapshot event
# or a dedicated capability method — whichever the implementer surfaces).
echo "$out" | hf_assert_event '(.type == "forge_metadata" or .type == "sync_diff")
  and ((.remote // .snapshot // {}) | keys | sort) == ["archived","default_branch","description","visibility"]'

# Capture original description for later restoration.
original=$(echo "$out" | jq -r 'select(.type == "forge_metadata" or .type == "sync_diff") | (.remote // .snapshot // {}).description' | head -n1)

# --- write capability: round-trip through adapter, then restore ---
out=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --fields "{\"description\":\"$STAMP\"}")
echo "$out" | hf_assert_event '.type == "error"' || \
  echo "$out" | hf_assert_event '.type == "push_remote_ok" or .type == "forge_metadata"'

verify=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$verify" | grep -q "$STAMP"

# Restore.
hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --fields "{\"description\":\"$original\"}" >/dev/null

# --- auth error when token blank ---
hf_put_secret "secrets://gh-token" ""
set +e
err=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "auth" or (.message // "" | test("auth"; "i")))'
hf_put_secret "secrets://gh-token" "$HF_TEST_GITHUB_TOKEN"

# --- not_found for a bogus repo name ---
BOGUS="definitely-does-not-exist-$TS"
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" "$HF_TEST_GITHUB_ORG" "$BOGUS" <<'PY'
import sys, yaml
p, org, bogus = sys.argv[1], sys.argv[2], sys.argv[3]
d = yaml.safe_load(open(p))
d["repos"].append({"name": bogus, "remotes": [{"url": f"https://github.com/{org}/{bogus}.git"}]})
open(p, "w").write(yaml.safe_dump(d))
PY
set +e
err=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$BOGUS" 2>&1)
set -e
echo "$err" | hf_assert_event '.type == "error" and (.error_class == "not_found" or (.message // "" | test("not.?found|404"; "i")))' || \
  echo "$err" | hf_assert_event '.type == "sync_diff" and .status == "errored"'

# --- no token leakage anywhere ---
full=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" 2>&1; \
       hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --fields "{\"description\":\"$original\"}" 2>&1 || true)
! echo "$full" | grep -q "$HF_TEST_GITHUB_TOKEN"

hf_teardown
echo "PASS"
