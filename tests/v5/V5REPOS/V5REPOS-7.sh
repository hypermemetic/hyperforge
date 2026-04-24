#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

CB='{"url":"https://codeberg.org/demo/widget.git"}'

# --- success: append codeberg remote to existing github-only repo ---
hf_spawn
hf_load_fixture org_with_repo
# Add codeberg.org to the provider_map (fixture has only github.com).
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c.setdefault("provider_map", {})["codeberg.org"] = "codeberg"
open(p, "w").write(yaml.safe_dump(c))
PY
out=$(hf_cmd repos add_remote --org demo --name widget --remote "$CB")
echo "$out" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 2 and .remotes[1].provider == "codeberg"'
hf_teardown

# --- dry_run: events match, file unchanged ---
hf_spawn
hf_load_fixture org_with_repo
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c.setdefault("provider_map", {})["codeberg.org"] = "codeberg"
open(p, "w").write(yaml.safe_dump(c))
PY
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
out=$(hf_cmd repos add_remote --org demo --name widget --remote "$CB" --dry_run true)
echo "$out" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 2'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]
hf_teardown

# --- duplicate URL rejected ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos add_remote --org demo --name widget --remote '{"url":"https://github.com/demo/widget.git"}' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- custom-domain override wins over missing provider_map entry ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos add_remote --org demo --name widget --remote '{"url":"https://git.internal.acme/demo/widget.git","provider":"gitlab"}')
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[1].provider == "gitlab"'
hf_teardown

# --- unknown provider variant rejected at wire boundary ---
hf_spawn
hf_load_fixture org_with_repo
set +e
out=$(hf_cmd repos add_remote --org demo --name widget --remote '{"url":"https://example.com/demo/widget.git","provider":"unknown"}' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
hf_teardown

# --- no credential required at add time ---
hf_spawn
hf_load_fixture org_with_repo
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c.setdefault("provider_map", {})["codeberg.org"] = "codeberg"
open(p, "w").write(yaml.safe_dump(c))
PY
# Org has no codeberg credential; add_remote must still succeed.
out=$(hf_cmd repos add_remote --org demo --name widget --remote "$CB")
echo "$out" | hf_assert_event '.type == "repo_detail"'
hf_teardown

# --- restart parity ---
hf_spawn
hf_load_fixture org_with_repo
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c.setdefault("provider_map", {})["codeberg.org"] = "codeberg"
open(p, "w").write(yaml.safe_dump(c))
PY
hf_cmd repos add_remote --org demo --name widget --remote "$CB" >/dev/null
saved="$HF_CONFIG"
hf_teardown
hf_spawn
cp -r "$saved"/* "$HF_CONFIG"/
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '.type == "repo_detail" and (.remotes | length) == 2'
rm -rf "$saved"
hf_teardown

echo "PASS"
