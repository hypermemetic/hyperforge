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
    remotes:
      - url: https://github.com/${HF_TEST_GITHUB_ORG}/${HF_TEST_GITHUB_REPO}.git
YAML
hf_put_secret "secrets://gh-token" "$HF_TEST_GITHUB_TOKEN"

# --- baseline: one SyncDiff event for the single remote ---
out=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$out" | hf_assert_count '.type == "sync_diff"' 1

# --- targeted remote: identical count when parameter matches the only URL ---
url="https://github.com/${HF_TEST_GITHUB_ORG}/${HF_TEST_GITHUB_REPO}.git"
out=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" --remote "$url")
echo "$out" | hf_assert_count '.type == "sync_diff"' 1

# --- sync is read-only: yaml byte-identical before/after ---
before=$(sha256sum "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml")
hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO" >/dev/null
after=$(sha256sum "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml")
[[ "$before" == "$after" ]]

# --- drift: add a local description that disagrees; expect drifted ---
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
d = yaml.safe_load(open(p))
d["repos"][0].setdefault("metadata", {})["description"] = "LOCAL-DISAGREES-" + __import__("time").strftime("%s")
open(p, "w").write(yaml.safe_dump(d))
PY
out=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$out" | hf_assert_event '.type == "sync_diff" and .status == "drifted"'
echo "$out" | hf_assert_event '.type == "sync_diff" and (.drift[] | .field == "description")'

# --- two remotes, one errored (unreachable host), one ok: both SyncDiffs emitted ---
python3 - "$HF_CONFIG/orgs/${HF_TEST_GITHUB_ORG}.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
d = yaml.safe_load(open(p))
d["repos"][0]["remotes"].append({"url": "https://127.0.0.1:1/no/such.git", "provider": "github"})
open(p, "w").write(yaml.safe_dump(d))
PY
out=$(hf_cmd repos sync --org "$HF_TEST_GITHUB_ORG" --name "$HF_TEST_GITHUB_REPO")
echo "$out" | hf_assert_count '.type == "sync_diff"' 2
echo "$out" | hf_assert_event '.type == "sync_diff" and .status == "errored"'

# --- no token leakage ---
! echo "$out" | grep -q "$HF_TEST_GITHUB_TOKEN"

hf_teardown
echo "PASS"
