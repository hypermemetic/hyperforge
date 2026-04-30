---
id: V5PARITY-31
title: "SSH-CLI — standalone hyperforge-ssh binary"
status: Pending
type: implementation
blocked_by: []
unlocks: []
---

## Problem

v4 shipped a separate `hyperforge-ssh` binary for SSH key management — generate keys, register them with forges, swap between keys per repo. v5 covers the runtime functionality through `repos.set_ssh_key` (V5PARITY-5) and `config_set_ssh_key` (V5PARITY-8) but ships no standalone CLI. Power users with v4 muscle memory miss the dedicated tool.

## Required behavior

**Decision required at impl time:** *do we actually want this binary, or should the v5 RPC surface plus a thin shell wrapper suffice?*

If we ship it: `hyperforge-ssh` becomes a thin client over the running v5 daemon plus local key generation. Subcommands:

| Command | Behavior |
|---|---|
| `hyperforge-ssh generate --name <key> [--org X --forge github]` | `ssh-keygen -t ed25519 -f ~/.ssh/<key>` + optionally registers the key path with the org via `config_set_ssh_key`. |
| `hyperforge-ssh list` | Lists all SSH keys configured across orgs. Composes `orgs.list` + per-org `config_show_ssh_key`. |
| `hyperforge-ssh register --org X --forge github --pubkey-path P` | Reads the public key, posts it to the forge's user-keys endpoint via the org's token. New `ForgePort::add_ssh_key`. |
| `hyperforge-ssh use --org X --forge github --key P` | Wires the key path on the org via the existing `config_set_ssh_key` RPC. |

If we don't: this ticket converts to `Skipped` with rationale documented in CONTRACTS, and the v4-vs-v5 doc is updated to note that SSH key management is RPC-only by design.

## What must NOT change

- Existing `repos.set_ssh_key` and `config_set_ssh_key` RPCs — `hyperforge-ssh` calls them, doesn't replace them.
- D9 — public keys are fine to display; private keys NEVER appear in any output.

## Acceptance criteria

1. `hyperforge-ssh generate --name id_test --org foo --forge github` produces `~/.ssh/id_test` + `~/.ssh/id_test.pub` and updates `orgs/foo.yaml`'s SSH credential.
2. `hyperforge-ssh list` after generation shows the new key in the listing.
3. `hyperforge-ssh register` against a real forge (tier 2) adds the public key to the user's account.

OR — implementer chooses to skip:

1. CONTRACTS gains a §decision documenting "SSH key management is RPC-only; no standalone CLI".
2. `docs/v5/v4-vs-v5.md` notes the deliberate divergence.
3. Ticket flips to `Skipped`.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-31.sh` → exit 0 (tier 1; if Skipped, the script just reports SKIP).
- Ready → Complete (or Skipped) in-commit.
