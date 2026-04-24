#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# Per-test unique workspace dir so scenarios don't collide.
make_ws_dir () {
  local d
  d=$(mktemp -d -t hf-v5-reconcile-XXXXXX)
  echo "$d"
}

# Write a fresh single-member fixture that points at the given ws dir.
write_fixture_pointing_at () {
  local ws_dir="$1"
  # Fresh config tree in $HF_CONFIG
  cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map:
  github.com: github
YAML
  mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
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
  cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: ${ws_dir}
repos:
  - demo/widget
YAML
}

init_git_dir () {
  local dir="$1"; local url="$2"
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$url"
}

# --- scenario 1: all aligned ---
hf_spawn
WS_DIR=$(make_ws_dir)
write_fixture_pointing_at "$WS_DIR"
init_git_dir "$WS_DIR/widget" "https://github.com/demo/widget.git"
ws_yaml_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
fs_before=$(cd "$WS_DIR" && find . | sort | xargs -I{} sh -c 'test -f "{}" && sha256sum "{}" || echo "DIR {}"')
out=$(hf_cmd workspaces reconcile name=main)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "matched" and .ref.org == "demo" and .ref.name == "widget"'
echo "$out" | hf_assert_count '.type == "reconcile_event" and .kind == "renamed"' 0
echo "$out" | hf_assert_count '.type == "reconcile_event" and .kind == "removed"' 0
# No filesystem change
fs_after=$(cd "$WS_DIR" && find . | sort | xargs -I{} sh -c 'test -f "{}" && sha256sum "{}" || echo "DIR {}"')
[[ "$fs_before" == "$fs_after" ]]
rm -rf "$WS_DIR"
hf_teardown

# --- scenario 2: dir renamed ---
hf_spawn
WS_DIR=$(make_ws_dir)
write_fixture_pointing_at "$WS_DIR"
init_git_dir "$WS_DIR/widget-local" "https://github.com/demo/widget.git"

# dry_run first — yaml byte-identical, event emitted
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces reconcile name=main dry_run=true)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "renamed" and .ref.org == "demo" and .ref.name == "widget" and .dir == "widget-local"'
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$before" ]]

# real run — yaml rewritten
out=$(hf_cmd workspaces reconcile name=main)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "renamed" and .dir == "widget-local"'
detail=$(hf_cmd workspaces get name=main)
# Post-reconcile entry should be object form with dir "widget-local"
echo "$detail" | hf_assert_event '.type == "workspace_detail" and (.repos[0].dir == "widget-local")'
echo "$detail" | hf_assert_event '.type == "workspace_detail" and (.repos[0].ref.org == "demo") and (.repos[0].ref.name == "widget")'
rm -rf "$WS_DIR"
hf_teardown

# --- scenario 3: dir removed (no matching dir exists at all) ---
hf_spawn
WS_DIR=$(make_ws_dir)
write_fixture_pointing_at "$WS_DIR"
# No subdir; workspace path exists but is empty.

# dry_run first
before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces reconcile name=main dry_run=true)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "removed" and .ref.org == "demo" and .ref.name == "widget"'
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$before" ]]

# real run
out=$(hf_cmd workspaces reconcile name=main)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "removed"'
detail=$(hf_cmd workspaces get name=main)
echo "$detail" | hf_assert_event '.type == "workspace_detail" and (.repos == [])'

# Workspace path itself is untouched (still exists, still empty)
[[ -d "$WS_DIR" ]]
[[ -z "$(ls -A "$WS_DIR")" ]]
rm -rf "$WS_DIR"
hf_teardown

# --- scenario 4: ambiguous — two dirs with same remote, alphabetical winner ---
hf_spawn
WS_DIR=$(make_ws_dir)
write_fixture_pointing_at "$WS_DIR"
init_git_dir "$WS_DIR/alpha" "https://github.com/demo/widget.git"
init_git_dir "$WS_DIR/beta"  "https://github.com/demo/widget.git"

out=$(hf_cmd workspaces reconcile name=main)
# Exactly one winner event (matched or renamed) pointing at alpha
winners=$(echo "$out" | jq -c 'select(.type == "reconcile_event" and (.kind == "matched" or .kind == "renamed") and .ref.org == "demo" and .ref.name == "widget")')
n_winners=$(echo "$winners" | grep -c . || true)
[[ "$n_winners" == "1" ]]
# Ambiguous event names beta (not alpha)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "ambiguous" and .dir == "beta"'
echo "$out" | hf_assert_no_event '.type == "reconcile_event" and .kind == "ambiguous" and .dir == "alpha"'

# Winner is alpha by ref
detail=$(hf_cmd workspaces get name=main)
echo "$detail" | hf_assert_event '.type == "workspace_detail" and (.repos[0].dir == "alpha" or .repos[0] == "demo/widget")'

# Stable: second reconcile emits the same winner
out2=$(hf_cmd workspaces reconcile name=main)
echo "$out2" | hf_assert_event '.type == "reconcile_event" and .kind == "ambiguous" and .dir == "beta"'
echo "$out2" | hf_assert_no_event '.type == "reconcile_event" and .kind == "ambiguous" and .dir == "alpha"'
rm -rf "$WS_DIR"
hf_teardown

# --- scenario 5: new_matched — dir with known repo's origin, not a member ---
hf_spawn
WS_DIR=$(make_ws_dir)
# Workspace with zero members but the dir has origin matching demo/widget
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map:
  github.com: github
YAML
mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
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
cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: ${WS_DIR}
repos: []
YAML
init_git_dir "$WS_DIR/widget" "https://github.com/demo/widget.git"
ws_before=$(sha256sum "$HF_CONFIG/workspaces/main.yaml")
out=$(hf_cmd workspaces reconcile name=main)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "new_matched" and .ref.org == "demo" and .ref.name == "widget" and .dir == "widget"'
# Workspace yaml untouched regardless of dry_run
[[ "$(sha256sum "$HF_CONFIG/workspaces/main.yaml")" == "$ws_before" ]]
rm -rf "$WS_DIR"
hf_teardown

# --- scenario 6: non-git subdirectories ignored ---
hf_spawn
WS_DIR=$(make_ws_dir)
write_fixture_pointing_at "$WS_DIR"
init_git_dir "$WS_DIR/widget" "https://github.com/demo/widget.git"
mkdir -p "$WS_DIR/not-a-git-dir"
echo "data" > "$WS_DIR/not-a-git-dir/file.txt"
out=$(hf_cmd workspaces reconcile name=main)
echo "$out" | hf_assert_event '.type == "reconcile_event" and .kind == "matched"'
echo "$out" | hf_assert_no_event '.dir == "not-a-git-dir"'
rm -rf "$WS_DIR"
hf_teardown

echo "PASS"
