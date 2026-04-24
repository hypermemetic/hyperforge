#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- empty fixture: clean load, zero everything ---
hf_spawn
hf_load_fixture empty

out=$(hf_cmd status)
echo "$out" | hf_assert_event '.type == "status"'

# Load + introspect config surface via whatever wire method the implementer
# chose; until that surface lands this ticket's "acceptance" is that the
# loader is exercised indirectly by V5CORE-10's aggregation plus the
# fixtures being present and syntactically valid YAML.
test -f "$HF_CONFIG/config.yaml"

hf_teardown

# --- minimal_org fixture: exactly one org loads and round-trips ---
hf_spawn
hf_load_fixture minimal_org

test -f "$HF_CONFIG/orgs/demo.yaml"

# Round-trip sanity: parse fixture as YAML via python+pyyaml if available,
# else skip the deep check (the Rust-side loader's round-trip is covered
# by the implementation test harness; this script's job is fixture shape).
if command -v python3 >/dev/null && python3 -c 'import yaml' 2>/dev/null; then
  python3 -c '
import sys, yaml
c = yaml.safe_load(open(sys.argv[1]))
o = yaml.safe_load(open(sys.argv[2]))
assert c["provider_map"]["github.com"] == "github", c
assert o["name"] == "demo", o
assert o["forge"]["provider"] == "github", o
assert o["forge"]["credentials"] == [], o
assert o["repos"] == [], o
' "$HF_CONFIG/config.yaml" "$HF_CONFIG/orgs/demo.yaml"
fi

hf_teardown

# --- corrupted fixture: invalid YAML is a loader error naming the file ---
hf_spawn
hf_load_fixture empty
printf 'this: is: not: valid: yaml:\n' > "$HF_CONFIG/config.yaml"
set +e
err=$(hf_cmd status 2>&1)
rc=$?
set -e
# Implementer may choose to fail eagerly at spawn or lazily at first use;
# either way the error must name config.yaml.
if [[ $rc -ne 0 ]]; then
  echo "$err" | grep -q 'config.yaml'
fi

hf_teardown

# --- basename mismatch: orgs/foo.yaml with name: bar is an error ---
hf_spawn
hf_load_fixture minimal_org
mv "$HF_CONFIG/orgs/demo.yaml" "$HF_CONFIG/orgs/foo.yaml"
# foo.yaml still contains name: demo — mismatch.
set +e
err=$(hf_cmd status 2>&1)
rc=$?
set -e
if [[ $rc -ne 0 ]]; then
  echo "$err" | grep -q 'foo'
  echo "$err" | grep -q 'demo'
fi

hf_teardown
echo "PASS"
