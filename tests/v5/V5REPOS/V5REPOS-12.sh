#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- github.com → github ---
hf_spawn
hf_load_fixture org_with_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github"'
hf_teardown

# --- order preserved: github, codeberg ---
hf_spawn
hf_load_fixture org_with_mirror_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github" and .remotes[1].provider == "codeberg"'
hf_teardown

# --- per-remote override wins ---
hf_spawn
hf_load_fixture org_with_custom_domain_repo
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "gitlab"'
hf_teardown

# --- unknown domain, no override: derivation error ---
hf_spawn
hf_load_fixture minimal_org
set +e
out=$(hf_cmd repos add --org demo --name widget --remotes '[{"url":"https://git.unknown.example/demo/widget.git"}]' 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
# The error must name the URL or the extracted host.
echo "$out" | grep -qE 'unknown\.example|git\.unknown'
hf_teardown

# --- provider_map hot-swap: remove entry → error; add back → success (no restart) ---
hf_spawn
hf_load_fixture org_with_repo
# Remove github.com entry.
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c["provider_map"] = {k: v for k, v in (c.get("provider_map") or {}).items() if k != "github.com"}
open(p, "w").write(yaml.safe_dump(c))
PY
set +e
out=$(hf_cmd repos get --org demo --name widget 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
# Restore and expect success without a respawn.
python3 - "$HF_CONFIG/config.yaml" <<'PY'
import sys, yaml
p = sys.argv[1]
c = yaml.safe_load(open(p)) or {}
c.setdefault("provider_map", {})["github.com"] = "github"
open(p, "w").write(yaml.safe_dump(c))
PY
out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '.type == "repo_detail" and .remotes[0].provider == "github"'
hf_teardown

echo "PASS"
