#!/usr/bin/env bash
# tier: 2
# V5PARITY-2 acceptance: repos.import + workspaces.discover.
# Tier 2 for the import path (real GitHub API). Discover is FS-only
# and runs regardless.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# --- Tier 1: workspaces.discover against fake git repos ---
hf_load_fixture org_with_repo
scan_root="$(mktemp -d -t v5prty2-disc-XXXXXX)"
mkdir -p "$scan_root/widget/.git" "$scan_root/orphan/.git"
cat > "$scan_root/widget/.git/config" <<CFG
[remote "origin"]
    url = https://github.com/demo/widget.git
CFG
cat > "$scan_root/orphan/.git/config" <<CFG
[remote "origin"]
    url = https://example.com/nobody/nothing.git
CFG

out=$(hf_cmd workspaces discover --path "$scan_root" --name disc-test)
echo "$out" | hf_assert_event '.type == "discover_match" and .status == "matched" and .dir == "widget"'
echo "$out" | hf_assert_event '.type == "discover_match" and .status == "orphan" and .dir == "orphan"'
echo "$out" | hf_assert_event '.type == "workspace_discovered" and .name == "disc-test" and .repo_count == 1'

# Verify the workspace yaml was written.
hf_cmd workspaces list | hf_assert_event '.name == "disc-test"'

rm -rf "$scan_root"
hf_teardown

# --- Tier 2: repos.import against the real sandbox org ---
if [[ -z "${HF_V5_TEST_CONFIG_DIR:-}" || ! -f "${HF_V5_TEST_CONFIG_DIR:-/dev/null}/tier2.env" ]]; then
    echo "SKIP: tier-2 config not set; discover tier-1 portion passed"
    echo "PASS"
    exit 0
fi

hf_spawn
hf_use_test_config
set -a; source "$HF_V5_TEST_CONFIG_DIR/tier2.env"; set +a
ORG="$HF_TIER2_GITHUB_ORG"

out=$(hf_cmd repos import --org "$ORG" --forge github)
echo "$out" | hf_assert_event '.type == "import_summary" and .org == "'"$ORG"'" and .total > 0'

# Second import is idempotent.
out2=$(hf_cmd repos import --org "$ORG" --forge github)
echo "$out2" | hf_assert_event '.type == "import_summary" and .added == 0'

hf_teardown
echo "PASS"
