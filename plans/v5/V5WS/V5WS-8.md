---
id: V5WS-8
title: "workspaces.reconcile — dir-rename and dir-removal detection via remote URL match"
status: Complete
type: implementation
blocked_by: [V5CORE-3, V5CORE-8, V5CORE-9]
unlocks: [V5WS-10]
---

## Problem

Local clone directories get renamed or deleted over time and the
workspace yaml drifts from disk. `reconcile` walks the workspace's
`path`, matches each git dir to a member by `origin` URL, and rewrites
the workspace yaml to match reality — without ever touching the
filesystem or any forge.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `name` | `WorkspaceName` | yes | must match an existing workspace |
| `dry_run` | `bool` | no (default false) | D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| per dir / per member | `ReconcileEvent` | `kind ∈ {matched, renamed, removed, new_matched, ambiguous}`; `ref?` and `dir?` populated per kind |
| workspace not found | typed error event | names the `WorkspaceName` |

Kind semantics (pinned here; V5WS-10 asserts): `matched` — declared
dir exists and `origin` matches a known remote; no yaml mutation.
`renamed` — declared dir absent but exactly one OTHER dir matches;
entry rewritten to `{ref, dir: <actual>}`. `removed` — no dir matches
any of the member's remotes; entry dropped. `new_matched` — a dir's
`origin` matches a non-member repo present in some org yaml;
informational, no mutation. `ambiguous` — when multiple dirs share a
URL matching one member (D5), first-scanned alphabetically wins
(`matched`/`renamed`); each other candidate emits `ambiguous`.

Scan is strictly local: subdirs in ascending ASCII order; `origin` via
`git config`-equivalent; no network. On `dry_run: false`, yaml writes
apply `renamed` + `removed` events in emission order (D8 atomic). On
`dry_run: true`: same event stream; yaml byte-identical.

Edge cases: workspace `path` absent → every member `removed`; `repos`
becomes `[]`. Non-git subdir → ignored. Member with zero remotes →
`removed` if dir absent. Two members sharing a URL + one matching dir
→ alphabetically-first member (by `<org>/<name>`) wins.

## What must NOT change

- v4's `workspace.*` namespace. v5 writes only `~/.config/hyperforge/workspaces/<ws>.yaml`.
- Org yamls are READ-only — reconcile reads for URL lookup, never writes.
- Filesystem under the workspace's `path` is NEVER mutated; scan is read-only.
- No forge endpoint is contacted under any code path.

## Acceptance criteria

(Scenarios stage real git dirs via `git init` + `git remote add origin <url>`.)

1. **Aligned** — one `matched` event for the member; yaml byte-identical regardless of `dry_run`.
2. **Renamed** — one `renamed` event with `dir == "widget-local"`. On `dry_run: false`, `workspaces.get` returns object form `{ref, dir: "widget-local"}`; on `dry_run: true`, yaml is byte-identical.
3. **Removed** — one `removed` event. On `dry_run: false`, `workspaces.get` omits that entry; the local filesystem is byte-identical (reconcile deletes nothing).
4. **Ambiguous (D5)** — two dirs `alpha` and `beta` share a member remote: one winner event points at `alpha`, one `ambiguous` event names `beta`. Stable across repeated calls. Post-reconcile (non-dry) binds the member to `dir: "alpha"`.
5. **New matched** — a non-member dir whose `origin` matches a known org yaml entry emits `new_matched`; yaml byte-identical regardless of `dry_run`.
6. **Non-git subdir** — no event; no error.
7. Across every scenario, no file under the workspace `path` or under `orgs/` appears, disappears, or changes bytes.

## Completion

- Run `bash tests/v5/V5WS/V5WS-8.sh` → exit 0.
- Status flips in-commit with the implementation.
