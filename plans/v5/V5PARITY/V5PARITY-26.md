---
id: V5PARITY-26
title: "BEGIN-AS-FIRST-RUN — surface onboarding hints automatically"
status: Complete
type: implementation
blocked_by: []
unlocks: []
---

## Problem

`begin` exists (V5PARITY-8) and emits useful next-step hints, but you only see them if you know to call `begin`. The first user experience on a fresh daemon is `synapse … status` returning a version + config dir — no signal that the next step is `orgs.bootstrap` or `auth.detect_external`.

## Required behavior

**`status` becomes onboarding-aware.** When the daemon reports status and detects an empty config (no orgs, no workspaces, no secrets refs), it includes an `onboarding_hint` field on the `status` event:

```json
{
  "type": "status",
  "version": "...",
  "config_dir": "...",
  "onboarding_hint": "Run `synapse … hyperforge begin` for next steps, or `hyperforge-v5 onboard` from the CLI."
}
```

**Daemon stderr on empty-config startup.** When the daemon boots and finds no orgs configured, it logs (one line, to stderr, only on first observation per process) the same hint. Quiet on subsequent startups (because by then there's config to read).

**Optional: `status_extended` method.** Returns `status` + `begin`'s next-step events in a single call so RPC clients can render onboarding state without two roundtrips.

## What must NOT change

- The wire shape of `status` for a *configured* daemon — `onboarding_hint` is `Option<String>` and skip-serialized when absent. Existing consumers see no change.
- D9 events — no secret content in onboarding hints.
- `begin` itself — still emits the structured next-step events; `status`'s hint is a string pointer to that flow.

## Acceptance criteria

1. On a fresh daemon (empty config dir), `synapse … status` includes `onboarding_hint: "..."` in the event payload.
2. After `orgs.bootstrap` registers the first org, subsequent `status` calls omit `onboarding_hint`.
3. Daemon stderr on first boot with empty config contains the hint exactly once; second boot (still empty) is silent (the persistence is in-process, not durable, but a single hint per process suffices).
4. `status_extended` (if implemented) returns `status` + the `begin_next_step` event sequence in one stream.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-26.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
