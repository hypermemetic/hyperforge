---
id: V5REPOS-14
title: "repos.push — sequential per-remote metadata write per D4"
status: Ready
type: implementation
blocked_by: [V5REPOS-2, V5REPOS-12]
unlocks: [V5REPOS-15]
---

## Problem

Users cannot propagate local metadata edits to their forges without
hand-using each provider's API. `repos.push` applies local state to
every declared remote in sequence per D4 — first failure aborts; already
succeeded remotes are reported in the result. A caller may scope to one
remote via `--remote <url>`.

## Required behavior

| Input | Type | Required | Notes |
|---|---|---|---|
| `org` | `OrgName` | yes | MUST match an existing org file |
| `name` | `RepoName` | yes | MUST match a repo entry under that org |
| `remote` | `RemoteUrl` | no | when present, push only the matching remote |
| `dry_run` | `bool` | no | default false per D7 |

| Output / Event | Shape | Notes |
|---|---|---|
| per-remote success | event carrying the remote's URL and the fields that were applied | one per remote processed |
| per-remote failure | typed error event referencing the remote's URL and the V5REPOS-2 error class | emitted ONCE, then the method aborts |
| summary | one final event listing already-succeeded remotes and the aborted remote (if any) | ordered by the repo's declared remote order |
| dry_run preview | identical event stream to a real run | no forge calls; no filesystem change |
| not found | typed error event | `org`, `name`, or (when supplied) `remote` absent |

Ordering (per D4): remotes are processed in the order they appear in the
org yaml's `remotes[]`. First-fail-aborts: on the first per-remote
failure, no further remotes are attempted; already-succeeded remotes
are reported in the summary along with the failing remote.

`remote` parameter semantics: when supplied, exactly that remote is
processed; remotes before it in the declared list are NOT processed.
This is a pinpoint push, not a resume.

Edge cases:

- Repo with zero local changes vs forge: push still succeeds per remote (idempotent writes); summary reports each remote succeeded.
- `dry_run=true`: no adapter write is invoked; the event stream reports what WOULD happen — including the order and the per-remote field set — without mutating the forge.
- All remotes on the same provider vs mixed providers: adapter dispatch is per-remote (V5REPOS-12), not per-repo.
- An adapter `auth` error on remote 2 of 3: remote 1 is reported succeeded, remote 2 is reported errored, remote 3 is NOT attempted; the method exits non-successfully.

## What must NOT change

- v4's push behavior.
- The `ForgePort` capability pinned in V5REPOS-2: `repos.push` MUST NOT require a method the trait doesn't declare.
- Per D4, the ordering is fixed: declared remote order, sequential, first-fail-aborts.
- Per D7, dry_run emits the same events with no side effects.
- Per the Secret redaction rule, no event contains a resolved credential value.
- No local YAML mutation. `repos.push` reads from local, writes only to forges.

## Acceptance criteria (tier 2)

1. With `$HF_TEST_GITHUB_ORG`, `$HF_TEST_GITHUB_REPO`, `$HF_TEST_GITHUB_TOKEN` set and a local description set on the repo entry, `repos.push org=... name=...` emits a per-remote success event for the GitHub remote and a summary listing that remote succeeded. A subsequent `repos.sync` reports `in_sync`.
2. With `dry_run=true`, the forge-side description is NOT modified; events match the shape of a real run. A subsequent `repos.sync` still reports `drifted`.
3. For a repo with two remotes where the first remote's adapter has a valid token and the second's does not, a non-dry `repos.push` emits: one per-remote success (remote 1), one per-remote failure (remote 2, class `auth`), then the summary listing remote 1 succeeded and remote 2 failed. No further remotes are attempted.
4. `repos.push ... remote=<url>` processes only the one matching remote; other declared remotes produce no events.
5. Any per-remote failure causes overall non-zero exit from the command; a full success causes zero exit.
6. No event emitted by `repos.push` contains the literal value of `$HF_TEST_GITHUB_TOKEN`.
7. Test script exits 0 with a `SKIP:` line when required env vars are unset.
8. Test MUST restore the original forge-side description before exit.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-14.sh` → exit 0.
- Status flips in-commit with the implementation.
