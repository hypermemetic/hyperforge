---
id: V5PARITY-25
title: "REPO-REGISTER — adopt an existing local checkout"
status: Pending
type: implementation
blocked_by: []
unlocks: []
---

## Problem

Onboarding a *fresh* repo (clone + register) is well-supported via `repos.add --create_remote true` or `repos.import`. Onboarding an *existing* local checkout — the user has been working in `~/code/widget/` for a year — has no clean path. They have to `repos.add` manually, copy the right URL out of `git remote -v` themselves, and remember to run `repos.init` afterwards to write `.hyperforge/config.toml`. Three separate steps for a common case.

## Required behavior

**`repos.register --path <abs-path> [--org <org>] [--name <repo-name>] [--init bool]`**

1. Reads the checkout's `origin` URL via `ops::git::read_origin_url`.
2. If `--org` not supplied: derives from the URL's host + owner segment using the loaded `provider_map`. Fails with `validation` if no match.
3. If `--name` not supplied: derives from the URL's repo segment.
4. Adds an entry under the chosen org's yaml with the discovered `Remote { url, provider? }`. Idempotent — re-running with same `--path` is a no-op (or updates the existing entry's metadata if the URL changed).
5. With `--init true` (default), runs `repos.init --target_path <path> --org <org> --repo_name <name>` to write `.hyperforge/config.toml`.

**Discovers all remotes, not just origin.** If the checkout has additional remotes (mirror, fork, upstream), each is added to the `Remote` list. The first one is `origin` by convention.

**Conflict handling.** If the same name already exists under the org with different remotes, emits `repo_conflict { existing_remotes, observed_remotes }` and writes nothing — caller resolves manually.

## What must NOT change

- `repos.add`, `repos.init`, `repos.import` stay; this is composition + auto-detect.
- `.hyperforge/config.toml` schema (V5LIFECYCLE-9 + V5PARITY-12).
- D6 partial-failure tolerance — if `repos.init` fails, the org yaml is still written.

## Acceptance criteria

1. `repos.register --path /tmp/widget` on a checkout cloned from `https://github.com/demo/widget.git`, with `demo` already configured as an org, registers `widget` under `demo` with the canonical URL. `.hyperforge/config.toml` is written.
2. With no `--org` and no matching provider_map entry: `validation` error, no state mutation.
3. Re-registering the same path is a no-op (idempotent).
4. A checkout with `origin` + `mirror` remotes registers both; `origin` is `remotes[0]` (canonical).
5. With `--init false`, no `.hyperforge/config.toml` is written.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-25.sh` → exit 0 (tier 1).
- Ready → Complete in-commit.
