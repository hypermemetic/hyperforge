#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5LIFECYCLE-5: RepoLifecycle enum + privatized_on + protected fields
# round-trip through the org yaml and surface on repos.get.

hf_spawn
hf_load_fixture "minimal_org"

# Default state: active, no privatized_on, not protected.
out=$(hf_cmd repos get --org demo --name widget 2>&1 || true)
# The fixture doesn't have a 'widget' repo in minimal_org; make one.
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: widget
    remotes:
      - url: https://github.com/demo/widget.git
    metadata:
      description: "fresh"
YAML

out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '(.type == "repo_detail") and ((.metadata // {}).lifecycle == "active" or (.metadata // {}).lifecycle == null or (.metadata // {}).lifecycle == "")'

# Write a repo with all three new fields populated.
cat > "$HF_CONFIG/orgs/demo.yaml" <<'YAML'
name: demo
forge:
  provider: github
  credentials: []
repos:
  - name: widget
    remotes:
      - url: https://github.com/demo/widget.git
    metadata:
      description: "dismissed sample"
      lifecycle: dismissed
      privatized_on:
        - github
      protected: true
YAML

out=$(hf_cmd repos get --org demo --name widget)
echo "$out" | hf_assert_event '(.metadata // {}).lifecycle == "dismissed"'
echo "$out" | hf_assert_event '((.metadata // {}).privatized_on // []) | index("github")'
echo "$out" | hf_assert_event '(.metadata // {}).protected == true'

# Round-trip byte-identicality on a fixture without the new fields.
cp "$HF_CONFIG/orgs/demo.yaml" "$HF_CONFIG/orgs/demo.yaml.pre"
# Force a load → save cycle by triggering a synaspe call that only reads.
hf_cmd repos list --org demo >/dev/null
# The daemon shouldn't rewrite a yaml it only read.
cmp -s "$HF_CONFIG/orgs/demo.yaml" "$HF_CONFIG/orgs/demo.yaml.pre"

hf_teardown
echo "PASS"
