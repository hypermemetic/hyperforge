#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

# --- store + resolve round-trip ---
hf_put_secret "secrets://gh-token" "ghp_abc"

# The resolver capability is internal; verify it through a wire surface
# that consumes it. Until an adapter lands, probe via a debug/resolve
# method the implementer exposes OR through any method that echoes a
# resolution result. If no such wire surface exists, the implementer
# MUST add one scoped to tests (see ticket acceptance #1). This script
# calls that method as `resolve_secret <ref>`.
out=$(hf_cmd resolve_secret secret_ref="secrets://gh-token")
echo "$out" | hf_assert_event '.value == "ghp_abc"'

# Missing key → not-found error event naming the ref.
set +e
out=$(hf_cmd resolve_secret secret_ref="secrets://missing-key" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("secrets://missing-key"))'

# Malformed ref → invalid-ref error event.
set +e
out=$(hf_cmd resolve_secret secret_ref="not-a-secret-ref" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'

# Redaction invariant: status must not leak the plaintext.
status=$(hf_cmd status)
if echo "$status" | grep -q 'ghp_abc'; then
  echo "REDACTION FAIL: status leaked secret" >&2
  exit 1
fi

hf_teardown

# --- no secrets.yaml at all: resolution returns not-found, not an I/O error ---
hf_spawn
# Deliberately do not create secrets.yaml.
set +e
out=$(hf_cmd resolve_secret secret_ref="secrets://anything" 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("secrets://anything"))'

hf_teardown

# --- corrupted secrets.yaml: error names the file ---
hf_spawn
printf ':::: not valid yaml ::::\n' > "$HF_CONFIG/secrets.yaml"
set +e
out=$(hf_cmd resolve_secret secret_ref="secrets://gh-token" 2>&1)
set -e
echo "$out" | grep -q 'secrets.yaml'

hf_teardown
echo "PASS"
