#!/usr/bin/env bash
# tier: 1
# V5PARITY-15 acceptance: ops::git is a backend-choosing abstraction.
# - Public API + GitError variants unchanged.
# - HF_GIT_FORCE_SUBPROCESS=1 routes every op through subprocess.
# - Hand-rolled .git/config INI parser in workspaces.rs is gone.
# - V5LIFECYCLE-11's command-git DRY invariant updated for the dir split.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

cd "$(dirname "$0")/../../.."

# --- (1) module split: ops/git/{mod,subprocess,local}.rs ---
for f in src/v5/ops/git/mod.rs src/v5/ops/git/subprocess.rs src/v5/ops/git/local.rs; do
    [[ -f "$f" ]] || { echo "FAIL: missing $f" >&2; exit 1; }
done
[[ ! -f src/v5/ops/git.rs ]] || { echo "FAIL: legacy single-file git.rs still present" >&2; exit 1; }
echo "module: split into mod/subprocess/local"

# --- (2) git2 dep declared with vendored features ---
grep -qE 'git2 .*vendored-libgit2' Cargo.toml || {
    echo "FAIL: git2 not pinned with vendored-libgit2" >&2; exit 1;
}
echo "dep: git2 with vendored-libgit2"

# --- (3) hand-rolled INI parser is gone from workspaces.rs ---
if grep -qE '\[remote "origin"\]' src/v5/workspaces.rs; then
    echo "FAIL: hand-rolled INI section header still in workspaces.rs" >&2
    exit 1
fi
if ! grep -q 'read_origin_url' src/v5/workspaces.rs; then
    echo "FAIL: workspaces.rs no longer references ops::git::read_origin_url" >&2
    exit 1
fi
echo "INI parser: removed; routes via ops::git::read_origin_url"

# --- (4) V5LIFECYCLE-11's command-git DRY invariant covers ops/git/* ---
out="$(bash tests/v5/V5LIFECYCLE/V5LIFECYCLE-11.sh 2>&1)"
echo "$out" | grep -q 'DRY:command-git green' || {
    echo "FAIL: command-git DRY invariant not green" >&2
    echo "$out" | tail -10 >&2
    exit 1
}
echo "DRY: command-git green (path scope = ops/git/*)"

# --- (5) Behavior parity: tier-1 V5PARITY tests pass under both backends ---
# We exercise the tests that drive ops::git directly: V5PARITY-3 (transport),
# V5PARITY-5 (ssh), V5PARITY-9 (manifest/show), V5PARITY-14 (workspace verbs).
suite=( V5PARITY-3.sh V5PARITY-5.sh V5PARITY-9.sh V5PARITY-14.sh )
for env in "" "HF_GIT_FORCE_SUBPROCESS=1"; do
    label="${env:-default(git2)}"
    for t in "${suite[@]}"; do
        if [[ -n "$env" ]]; then
            HF_GIT_FORCE_SUBPROCESS=1 bash "tests/v5/V5PARITY/$t" >/dev/null 2>&1 || {
                echo "FAIL: $t under $label" >&2; exit 1;
            }
        else
            bash "tests/v5/V5PARITY/$t" >/dev/null 2>&1 || {
                echo "FAIL: $t under $label" >&2; exit 1;
            }
        fi
    done
    echo "parity: tier-1 suite green under $label"
done

# --- (6) Perf signal: workspace.status over a 5-member workspace.
# Not a hard threshold (CI variance) but the git2 backend should be
# faster than subprocess. We just record both numbers as a smoke check.
hf_spawn
TMP="$(mktemp -d -t v5prty15-perf-XXXXXX)"
mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
YAML
echo "name: demo
forge:
  provider: github
  credentials: []
repos:" > "$HF_CONFIG/orgs/demo.yaml"
ws_repos=""
for n in m1 m2 m3 m4 m5; do
    git init -q "$TMP/$n"
    git -C "$TMP/$n" config user.email t@t
    git -C "$TMP/$n" config user.name t
    git -C "$TMP/$n" commit --allow-empty -q -m initial
    echo "  - name: $n
    remotes:
      - url: file://$TMP/$n" >> "$HF_CONFIG/orgs/demo.yaml"
    ws_repos+="  - demo/$n
"
done
cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: $TMP
repos:
$ws_repos
YAML

# Time both backends. We use bash's $SECONDS at second granularity —
# a cheap signal, not a benchmark. The git2 backend should be at least
# as fast; we don't fail if it isn't because CI variance dominates.
t_git2_start=$(date +%s%N)
hf_cmd workspaces status --name main >/dev/null
t_git2=$(( ($(date +%s%N) - t_git2_start) / 1000000 ))

# Restart daemon so HF_GIT_FORCE_SUBPROCESS takes effect (it's read at op time
# inside the daemon, but a clean restart eliminates any caching edge cases).
hf_teardown
HF_GIT_FORCE_SUBPROCESS=1 hf_spawn
mkdir -p "$HF_CONFIG/orgs" "$HF_CONFIG/workspaces"
cp "$TMP/.daemon-state-stash"/* "$HF_CONFIG/" 2>/dev/null || true
# Actually: just rebuild the config directly since it's small.
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
YAML
echo "name: demo
forge:
  provider: github
  credentials: []
repos:" > "$HF_CONFIG/orgs/demo.yaml"
for n in m1 m2 m3 m4 m5; do
    echo "  - name: $n
    remotes:
      - url: file://$TMP/$n" >> "$HF_CONFIG/orgs/demo.yaml"
done
cat > "$HF_CONFIG/workspaces/main.yaml" <<YAML
name: main
path: $TMP
repos:
$ws_repos
YAML

t_sub_start=$(date +%s%N)
hf_cmd workspaces status --name main >/dev/null
t_sub=$(( ($(date +%s%N) - t_sub_start) / 1000000 ))
hf_teardown
rm -rf "$TMP"

echo "perf: workspaces.status (5 members) — git2: ${t_git2}ms  subprocess: ${t_sub}ms"

echo "PASS"
