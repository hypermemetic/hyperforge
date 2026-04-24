#!/usr/bin/env bash
# tier: 2
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

: "${HF_TEST_GITHUB_ORG:=}"
: "${HF_TEST_GITHUB_REPO:=}"
: "${HF_TEST_GITHUB_TOKEN:=}"

if [[ -z "$HF_TEST_GITHUB_ORG" || -z "$HF_TEST_GITHUB_REPO" || -z "$HF_TEST_GITHUB_TOKEN" ]]; then
  echo "SKIP: tier 2 env not set (HF_TEST_GITHUB_* required)"
  exit 0
fi

TS=$(date +%s)
STAMP="hyperforge-v5-repos-14 $TS"
url="https://github.com/${HF_TEST_GITHUB_ORG}/${HF_TEST_GITHUB_REPO}.git"

hf_spawn
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
    metadata:
      description: "$STAMP"
    remotes:
      - url: $url
YAML
hf_put_secret "secrets://gh-token" "$HF_TEST_GITHUB_TOKEN"

# Capture original description for restoration.
snap=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
original=$(echo "$snap" | jq -r 'select(.type == "sync_diff") | (.drift[]? | select(.field == "description") | .remote) // empty' | head -n1)
if [[ -z "$original" ]]; then
  original=$(echo "$snap" | jq -r 'select(.type == "forge_metadata") | .description' | head -n1)
fi

# --- full push: per-remote success + summary; subsequent sync is in_sync ---
out=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$out" | hf_assert_event '(.type == "push_remote_ok" or .type == "push_summary") and (.url == "'$url'" or (.results // [] | map(.url) | index("'$url'")))'
verify=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$verify" | hf_assert_event '.type == "sync_diff" and .status == "in_sync"'

# --- dry_run: forge unchanged (drift still shown if local ≠ forge after we perturb) ---
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" "dry-$STAMP" <<'PY'
import sys, yaml
p, v = sys.argv[1], sys.argv[2]
d = yaml.safe_load(open(p))
d["repos"][0].setdefault("metadata", {})["description"] = v
open(p, "w").write(yaml.safe_dump(d))
PY
out=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --dry_run true)
echo "$out" | hf_assert_event '(.type == "push_remote_ok" or .type == "push_summary")'
after=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$after" | hf_assert_event '.type == "sync_diff" and .status == "drifted"'

# --- first-fail-aborts: add a second remote whose adapter will fail (bad creds via missing secret override) ---
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
d = yaml.safe_load(open(p))
d["repos"][0]["remotes"].append({"url": "https://github.com/definitely-no-such-owner-xyz/nope.git", "provider": "github"})
open(p, "w").write(yaml.safe_dump(d))
PY
set +e
out=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" 2>&1)
rc=$?
set -e
# Per D4: remote 1 succeeds, remote 2 fails, abort. Overall non-zero.
echo "$out" | hf_assert_event '(.type == "push_remote_ok" or .type == "push_summary") and (.url == "'$url'" or (.results // [] | map(.url) | index("'$url'")))'
echo "$out" | hf_assert_event '.type == "error" or .type == "push_remote_error" or (.type == "push_summary" and ((.errored // []) | length) >= 1)'
[[ $rc -ne 0 ]]

# --- --remote scopes to exactly one remote ---
# Remove the bad remote first, then use --remote to target only the good one.
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
d = yaml.safe_load(open(p))
d["repos"][0]["remotes"] = [r for r in d["repos"][0]["remotes"] if "definitely-no-such" not in r["url"]]
d["repos"][0].setdefault("metadata", {})["description"] = "scoped-only"
open(p, "w").write(yaml.safe_dump(d))
PY
out=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --remote "$url")
echo "$out" | hf_assert_count '(.type == "push_remote_ok" or .type == "push_remote_error")' 1

# --- restore original forge-side description ---
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" "$original" <<'PY'
import sys, yaml
p, v = sys.argv[1], sys.argv[2]
d = yaml.safe_load(open(p))
d["repos"][0].setdefault("metadata", {})["description"] = v
open(p, "w").write(yaml.safe_dump(d))
PY
hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --remote "$url" >/dev/null

# --- no token leakage ---
full=$(hf_cmd repos push --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --remote "$url" 2>&1)
! echo "$full" | grep -q "$HF_TEST_GITHUB_TOKEN"

hf_teardown
echo "PASS"
