---
id: V5PARITY-20
title: "ONBOARD — `hyperforge-v5 onboard` CLI subcommand"
status: Pending
type: implementation
blocked_by: [V5PARITY-21, V5PARITY-23]
unlocks: []
---

## Problem

A first-time user with no v5 config in place has to: build the binary, learn the daemon flags, spawn it, learn `secrets.set` / `orgs.create` / `orgs.set_credential` / `repos.import`, and string those four RPCs together correctly (with parameter names that differ across methods). v4's `auth_setup` was an interactive walk; v5 has nothing equivalent. Real workflow regression.

## Required behavior

**`hyperforge-v5 onboard [--provider github|codeberg|gitlab]`** — an interactive subcommand that:

1. Detects the user's existing forge auth (per provider): `gh auth status`, `glab auth status`, etc.
2. If a token is found, fetches it (via `gh auth token` etc.) and surfaces the username + accessible orgs.
3. Prompts (or `--yes` for non-interactive) to register one or more orgs.
4. For each chosen org: spawns the daemon if not already running, calls `orgs.bootstrap` (V5PARITY-21) to handle the four-RPC sequence, optionally chains `workspaces.from_org` (V5PARITY-22) for the "and clone everything under /path" case.
5. Reports a final state summary: orgs registered, repos imported, workspaces created.

**Non-goals.** Replacing the per-method RPCs. The subcommand composes them; users with custom setups still call them directly.

**Interactive UX.** Plain stdin/stdout prompts; no TUI. Exit codes are conventional (0 success, non-zero failure with summary).

## What must NOT change

- The `secrets.*`, `orgs.*`, `repos.*` RPC surface — `onboard` only calls existing methods (plus V5PARITY-21/22 once landed).
- The daemon stays the source of truth; the CLI subcommand is a client-side wrapper.

## Acceptance criteria

1. `hyperforge-v5 onboard --provider github --yes` against a host with `gh auth login` already done: detects token, lists user-accessible orgs, registers each (via `orgs.bootstrap`), reports the count.
2. With no `gh` available: emits a clear "install gh and run `gh auth login` first" message and exits 1 — no half-configured state written.
3. With a daemon already running on 44105: re-uses it; doesn't spawn a second daemon.
4. The same operation re-run is idempotent (no duplicate org entries; uses `orgs.bootstrap`'s upsert semantics).

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-20.sh` → exit 0 (tier 1 — uses a stub `gh` shim to avoid touching real auth).
- Ready → Complete in-commit.
