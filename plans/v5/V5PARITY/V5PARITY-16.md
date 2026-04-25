---
id: V5PARITY-16
title: "PARALLEL — bounded-concurrency workspace iteration"
status: Pending
type: implementation
blocked_by: [V5PARITY-15]
unlocks: []
---

## Problem

V5PARITY-3 spec'd bounded parallelism for `workspaces.{clone,fetch,pull,push}` and the V5PARITY-14 verbs added on top, but every implementation has been sequential. On a 50-member workspace, `workspaces.status` spawns 50 git operations one after another. Once V5PARITY-15 cuts the per-call cost for local ops via git2, sequential becomes the dominant overhead.

## Required behavior

**`--concurrency N` parameter on every `workspaces.*` git verb** (clone, fetch, pull, push, status, checkout, commit, tag, plus the diff method from V5PARITY-18 if it's landed): default 4, accepts 1..=64. `1` is the existing sequential behavior; >1 fans out via a bounded `Semaphore` or `buffer_unordered`.

**Event order is no longer member-source-order.** Per-member events stream in completion order. The aggregate summary still arrives last. Tests that asserted ordering (none currently do, but document the change) move to set-membership assertions.

**Failure semantics unchanged.** Each member's result is independent — one failing member doesn't cancel the rest. `errored` counter behaves the same.

**Default-4 rationale.** Subprocess spawn cost dominates beneath ~4 concurrent processes; above ~8 most filesystems contend on `.git/index.lock` or remote-side rate limits. 4 is a conservative middle.

## What must NOT change

- Public method signatures of every `workspaces.*` verb stay the same — only the new `concurrency` param is added.
- D6 (partial-failure tolerance) — concurrency must not turn a single bad member into a workspace-wide abort.
- D13 — git ops still route through `ops::git::*`.
- Per-method return types (V5PARITY-17 once landed) — concurrency is purely an iteration-shape change, doesn't alter the events emitted.

## Acceptance criteria

1. `workspaces.status --name W --concurrency 8` against a 16-member workspace runs in materially less wall time than `--concurrency 1` (target: ≤ 2× single-call latency, vs ~16× sequential).
2. Per-member events arrive in completion order; the aggregate summary is the last event.
3. With one member intentionally failing (e.g., bogus path), the remaining 15 still produce events; the failure surfaces as `errored: 1` in the summary.
4. `--concurrency 1` is byte-identical to the V5PARITY-14 sequential behavior.
5. Out-of-range `concurrency` (0 or 65) produces an `Error { code: "validation" }` event.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-16.sh` → exit 0 (tier 1; uses local bare repos).
- Ready → Complete in-commit.
