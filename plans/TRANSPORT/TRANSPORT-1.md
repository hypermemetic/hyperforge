---
id: TRANSPORT-1
title: "Transport — remote-URL transport (SSH ↔ HTTPS) as a first-class capability"
status: Epic
type: epic
blocked_by: []
unlocks: []
---

## Goal

Hyperforge today hardcodes SSH as the remote-URL transport during
`repo init`, with no way to express a preference and no way to switch
later without dropping out of the tool and running `git remote
set-url` directly. When the preconditions for SSH aren't met (no
keys, no agent, no per-org key configured), hyperforge's git
operations all fail and the user has no hyperforge-native recovery
path — the only fix is to leave the tool.

When this epic is done:

- A caller can opt into `ssh` or `https` at repo-init time without
  editing files.
- A caller can change the remote-URL transport on an already-registered
  repo, idempotently, via hyperforge — no `git remote set-url` bypass
  needed.
- An operator can query a repo's current transport to know what state
  they're in before changing anything.
- Default transport behavior is explicit and documented; changing the
  default is a known one-line config edit.
- Existing `repo init` callers continue to work without flag changes;
  the existing SSH default is preserved unless a caller opts into
  HTTPS (no silent behavior change on the default path).

## Motivation and support

This epic exists because a real workflow hit the gap:

1. `repo init` was run on a repo whose remote was HTTPS and whose
   machine had no SSH keys configured.
2. Init rewrote the remote to SSH and left the repo unpushable.
3. Recovery required reverting via raw `git remote set-url` —
   hyperforge could not repair what it had changed.

Beyond this specific incident, HTTPS-over-`gh`-token is a perfectly
valid long-term transport for some orgs (especially those where the
user already has `gh` set up and isn't ready for SSH infra). Forcing
SSH excludes that use case.

## Dependency DAG

```
(root) ── TRANSPORT-2  transport control  ─── TRANSPORT-3  checkpoint
```

One capability ticket plus a checkpoint. The capability is small
enough that splitting init-time and post-init transport control into
separate tickets would just add ceremony — they share the same
validation and the same on-disk effect.

## Tickets

| ID | Status | Summary |
|----|--------|---------|
| TRANSPORT-2 | Pending | Caller-controlled remote-URL transport for a hyperforge-registered repo |
| TRANSPORT-3 | Pending | Checkpoint — verify transport control at init, after init, and on recovery |

## Out of scope

- Transport for non-github forges (codeberg, gitlab SSH/HTTPS) — same
  principle, but those forges are not wired into any test on this
  machine today. A follow-up can extend once github's path is proven.
- SSH key generation, ssh-agent management, or per-org key assignment
  (those live with existing `hyperforge-ssh` / `config_set_ssh_key`).
- Changing `repo init`'s default transport from SSH to anything else.
  This epic adds the capability to opt into HTTPS; choosing a different
  default is a separate decision.
- Mixed-transport-per-forge state (e.g. github over SSH but codeberg
  over HTTPS on the same repo). Hyperforge treats each repo as
  single-transport today; multi-transport is future work.
