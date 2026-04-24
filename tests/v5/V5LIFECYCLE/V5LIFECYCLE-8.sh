#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn
hf_load_fixture "minimal_org"

# Seed a repo.
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

# --- protect true ---
out=$(hf_cmd repos protect --org demo --name widget --protected true)
echo "$out" | hf_assert_event '.type == "repo_protection_set" and .protected == true'
got=$(hf_cmd repos get --org demo --name widget)
echo "$got" | hf_assert_event '(.metadata // {}).protected == true'

# --- protect false (idempotent clear) ---
out=$(hf_cmd repos protect --org demo --name widget --protected false)
echo "$out" | hf_assert_event '.type == "repo_protection_set" and .protected == false'
got=$(hf_cmd repos get --org demo --name widget)
# protected defaults to false; either absent or explicitly false is OK.
echo "$got" | hf_assert_event '((.metadata // {}).protected // false) == false'

# --- dry_run leaves disk untouched ---
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
hf_cmd repos protect --org demo --name widget --protected true --dry_run true >/dev/null
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml")
[[ "$before" == "$after" ]]

hf_teardown
echo "PASS"
