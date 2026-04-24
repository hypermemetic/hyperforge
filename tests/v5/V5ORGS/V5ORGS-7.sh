#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- add: appends a new CredentialEntry ---
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd orgs set_credential org=demo key=secrets://gh-token credential_type=token)
echo "$out" | hf_assert_event '(.org // .name) == "demo"'
detail=$(hf_cmd orgs get org=demo)
echo "$detail" | hf_assert_event '.type == "org_detail" and (.credentials | length == 1) and .credentials[0].key == "secrets://gh-token" and .credentials[0].type == "token" and .provider == "github"'
hf_teardown

# --- replace: same key twice, still one entry, same index ---
hf_spawn
hf_load_fixture minimal_org
hf_cmd orgs set_credential org=demo key=secrets://gh-token credential_type=token >/dev/null
out=$(hf_cmd orgs set_credential org=demo key=secrets://gh-token credential_type=token)
# Caller must be able to distinguish add from replace on the success event.
# We do not pin the exact field name; assert the event stream contains a token
# discriminating "replaced" vs "added" somewhere.
echo "$out" | jq -e 'select(.type != null) | .. | strings | test("replace"; "i")' >/dev/null
detail=$(hf_cmd orgs get org=demo)
echo "$detail" | hf_assert_event '.type == "org_detail" and (.credentials | length == 1)'
hf_teardown

# --- add alongside existing: append preserves prior entry at index 0 ---
hf_spawn
hf_load_fixture org_with_credentials
hf_cmd orgs set_credential org=demo key=secrets://gh-token-2 credential_type=token >/dev/null
detail=$(hf_cmd orgs get org=demo)
echo "$detail" | hf_assert_event '.type == "org_detail" and (.credentials | length == 2) and .credentials[0].key == "secrets://gh-token" and .credentials[1].key == "secrets://gh-token-2"'
hf_teardown

# --- dry_run: event emitted, disk unchanged ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
hf_cmd orgs set_credential org=demo key=secrets://gh-token credential_type=token dry_run=true >/dev/null
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
# fresh daemon still sees zero credentials
hf_teardown
hf_spawn
hf_load_fixture minimal_org
out=$(hf_cmd orgs get org=demo)
echo "$out" | hf_assert_event '.type == "org_detail" and (.credentials | length == 0)'
hf_teardown

# --- invalid key (plaintext): typed error, file byte-identical ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs set_credential org=demo key=ghp_leaky_plaintext credential_type=token 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error"'
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- unknown org: typed error, no files created or modified ---
hf_spawn
hf_load_fixture minimal_org
before=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
set +e
out=$(hf_cmd orgs set_credential org=nonexistent key=secrets://x credential_type=token 2>&1)
set -e
echo "$out" | hf_assert_event '.type == "error" and (.message // "" | contains("nonexistent"))'
[[ ! -f "$HF_CONFIG/orgs/nonexistent.yaml" ]]
after=$(sha256sum "$HF_CONFIG/orgs/demo.yaml" | awk '{print $1}')
[[ "$before" == "$after" ]]
hf_teardown

# --- secrets never land in yaml: seed a value, set credential, yaml has no plaintext ---
hf_spawn
hf_load_fixture minimal_org
hf_put_secret "secrets://gh-token" "ghp_extremely_secret"
hf_cmd orgs set_credential org=demo key=secrets://gh-token credential_type=token >/dev/null
if grep -q 'ghp_extremely_secret' "$HF_CONFIG/orgs/demo.yaml"; then
  echo "REDACTION FAIL: plaintext leaked into org yaml" >&2
  exit 1
fi
hf_teardown

echo "PASS"
