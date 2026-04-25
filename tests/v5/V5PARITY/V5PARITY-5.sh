#!/usr/bin/env bash
# tier: 1
# V5PARITY-5 acceptance: per-repo core.sshCommand wiring + repos.set_ssh_key.
# Tier-1: verifies the .git/config writes and rejects nonexistent keys.
# No real SSH handshake is attempted; the clone-forwarding path is
# exercised against a local bare repo so the env forward is observed
# without needing a real SSH remote.
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

hf_spawn

TMP="$(mktemp -d -t v5prty5-XXXXXX)"
REMOTE="$TMP/remote.git"
SEED="$TMP/seed"
CHECKOUT="$TMP/checkout"
KEY="$TMP/id_fake"

# Seed a local bare repo.
git init -q --bare "$REMOTE"
git init -q "$SEED"
git -C "$SEED" -c user.email=t@t -c user.name=t commit --allow-empty -m initial -q
git -C "$SEED" branch -m main 2>/dev/null || true
git -C "$SEED" remote add origin "$REMOTE"
git -C "$SEED" push -q origin main

# Create a fake SSH key file (valid path, opaque contents — we never call ssh).
printf 'not-a-real-ssh-key\n' > "$KEY"
chmod 600 "$KEY"

# Clone via plain git first so we have a working tree to target.
git clone -q "$REMOTE" "$CHECKOUT"

# --- repos.set_ssh_key writes core.sshCommand ---
out=$(hf_cmd repos set_ssh_key --path "$CHECKOUT" --key "$KEY")
echo "$out" | hf_assert_event ".type == \"repo_ssh_key_set\" and .path == \"$CHECKOUT\" and .key == \"$KEY\""
got=$(git -C "$CHECKOUT" config --get core.sshCommand)
case "$got" in
    "ssh -i $KEY -o IdentitiesOnly=yes") ;;
    *) echo "unexpected core.sshCommand: $got" >&2; exit 1 ;;
esac

# --- nonexistent key raises invalid_key BEFORE any git write ---
git -C "$CHECKOUT" config --unset core.sshCommand
out=$(hf_cmd repos set_ssh_key --path "$CHECKOUT" --key "/no/such/key")
echo "$out" | hf_assert_event '.type == "error" and .code == "invalid_key"'
# Verify no sshCommand was written.
if git -C "$CHECKOUT" config --get core.sshCommand >/dev/null 2>&1; then
    echo "set_ssh_key with bogus key should not touch .git/config" >&2
    exit 1
fi

# --- ~ expansion ---
HOME_KEY="$HOME/.hf-test-key-v5parity5"
printf 'not-real\n' > "$HOME_KEY"
chmod 600 "$HOME_KEY"
out=$(hf_cmd repos set_ssh_key --path "$CHECKOUT" --key '~/.hf-test-key-v5parity5')
echo "$out" | hf_assert_event ".type == \"repo_ssh_key_set\" and (.key | endswith(\"/.hf-test-key-v5parity5\"))"
got=$(git -C "$CHECKOUT" config --get core.sshCommand)
case "$got" in
    *"/.hf-test-key-v5parity5 -o IdentitiesOnly=yes") ;;
    *) echo "~ not expanded in sshCommand: $got" >&2; exit 1 ;;
esac
rm -f "$HOME_KEY"

# --- persist_to_org appends the credential ---
mkdir -p "$HF_CONFIG/orgs"
cat > "$HF_CONFIG/config.yaml" <<'YAML'
provider_map: {}
YAML
cat > "$HF_CONFIG/orgs/demo.yaml" <<YAML
name: demo
forge:
  provider: github
  credentials: []
repos: []
YAML
out=$(hf_cmd repos set_ssh_key --path "$CHECKOUT" --key "$KEY" --org demo --persist_to_org true)
echo "$out" | hf_assert_event '.type == "repo_ssh_key_set" and .persisted == true'
grep -q 'type: ssh_key' "$HF_CONFIG/orgs/demo.yaml"

# --- repos.clone resolves the org's ssh_key and sets core.sshCommand on the clone ---
cat > "$HF_CONFIG/orgs/demo.yaml" <<YAML
name: demo
forge:
  provider: github
  credentials:
    - key: $KEY
      type: ssh_key
repos:
  - name: widget
    remotes:
      - url: $REMOTE
YAML
hf_cmd reload >/dev/null
DEST2="$TMP/clone2"
out=$(hf_cmd repos clone --org demo --name widget --dest "$DEST2")
echo "$out" | hf_assert_event '.type == "clone_done"'
got=$(git -C "$DEST2" config --get core.sshCommand)
case "$got" in
    "ssh -i $KEY -o IdentitiesOnly=yes") ;;
    *) echo "clone did not persist sshCommand: $got" >&2; exit 1 ;;
esac

rm -rf "$TMP"
hf_teardown
echo "PASS"
