#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# V5PROV-2 acceptance: ForgePort trait advertises the three new
# lifecycle methods and the expanded error-class set. Verifiable
# via the schema introspection surface V5REPOS-2 established.

hf_spawn
hf_load_fixture "empty"

out=$(hf_cmd repos forge_port_schema 2>/dev/null || hf_cmd repos forge-port-schema)

# Three new method names must appear.
for m in create_repo delete_repo repo_exists; do
    echo "$out" | hf_assert_event '(.methods // .method_set // []) | index("'"$m"'")'
done

# Error classes contain original five plus conflict + unsupported_visibility.
for c in auth network not_found rate_limited unsupported_field conflict unsupported_visibility; do
    echo "$out" | hf_assert_event '(.error_classes // []) | index("'"$c"'")'
done

hf_teardown
echo "PASS"
