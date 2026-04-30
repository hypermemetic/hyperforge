#!/usr/bin/env bash
# tier: 1
# V5PARITY-35: typed RPCs for scoping forges (single repo + workspace bulk).
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

TMP=$(mktemp -d -t v5prty35-XXXXXX)
mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map:
  github.com: github
  codeberg.org: codeberg
YAML
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: alpha
    remotes:
      - url: https://github.com/demo/alpha.git
      - url: https://codeberg.org/demo/alpha.git
  - name: beta
    remotes:
      - url: https://github.com/demo/beta.git
  - name: gamma
    remotes:
      - url: https://github.com/demo/gamma.git
YAML
hf_cmd reload >/dev/null

# --- (1) single repo: --forges codeberg ---
out=$(hf_cmd repos set_forges --org demo --name alpha --forges codeberg)
echo "$out" | hf_assert_event '.type == "forges_set" and .ref.name == "alpha" and .changed == true'
echo "$out" | hf_assert_event '.forges | index("codeberg")'
grep -q 'forges:' "$HF_CONFIG/orgs/demo.yaml"
grep -q 'codeberg' "$HF_CONFIG/orgs/demo.yaml"
echo "single: alpha scoped to [codeberg]"

# --- (2) idempotent re-run ---
out=$(hf_cmd repos set_forges --org demo --name alpha --forges codeberg)
echo "$out" | hf_assert_event '.type == "forges_set" and .changed == false'
echo "single: re-run is idempotent"

# --- (3) --forges none means [] (scoped to no forges) ---
out=$(hf_cmd repos set_forges --org demo --name beta --forges none)
echo "$out" | hf_assert_event '.type == "forges_set" and (.forges | length) == 0'
echo "single: --forges none → []"

# --- (4) --forges unset removes the field ---
out=$(hf_cmd repos set_forges --org demo --name alpha --forges unset)
echo "$out" | hf_assert_event '.type == "forges_set"'
# Use jq to verify .forges is null (absent) on the wire.
echo "$out" | jq -e 'select(.type == "forges_set") | .forges == null' >/dev/null
echo "single: --forges unset removes the field"

# --- (5) invalid provider rejected ---
out=$(hf_cmd repos set_forges --org demo --name beta --forges zoid,github)
echo "$out" | hf_assert_event '.type == "error" and .code == "validation"'
echo "validation: bad provider rejected"

# --- (6) workspaces.set_forges with --filter ---
cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: $TMP
repos:
  - demo/alpha
  - demo/beta
  - demo/gamma
YAML
hf_cmd reload >/dev/null
# Reset all to unscoped.
hf_cmd repos set_forges --org demo --name alpha --forges unset >/dev/null
hf_cmd repos set_forges --org demo --name beta --forges unset >/dev/null
hf_cmd repos set_forges --org demo --name gamma --forges unset >/dev/null

out=$(hf_cmd workspaces set_forges --name main --forges none --filter "alpha,beta")
echo "$out" | hf_assert_event '.type == "forges_set" and .ref.name == "alpha"'
echo "$out" | hf_assert_event '.type == "forges_set" and .ref.name == "beta"'
echo "$out" | hf_assert_no_event '.type == "forges_set" and .ref.name == "gamma"'
echo "$out" | hf_assert_event '.type == "workspace_set_forges_summary" and .total == 2 and .ok == 2'
echo "workspace: filter scoped only alpha+beta"

# --- (7) --dry_run true: events emitted but state unchanged ---
hf_cmd repos set_forges --org demo --name gamma --forges unset >/dev/null
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
out=$(hf_cmd workspaces set_forges --name main --forges codeberg --filter gamma --dry_run true)
echo "$out" | hf_assert_event '.type == "forges_set" and .dry_run == true'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]] || { echo "FAIL: dry_run mutated state" >&2; exit 1; }
echo "workspace: --dry_run true does not mutate"

rm -rf "$TMP"
hf_teardown
echo "PASS"
