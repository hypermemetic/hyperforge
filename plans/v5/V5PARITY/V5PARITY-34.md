---
id: V5PARITY-34
title: "PER-REPO-FORGE-SCOPE — `.hyperforge/config.toml` `forges` is authoritative"
status: Complete
type: implementation
blocked_by: []
unlocks: []
---

## Problem

`.hyperforge/config.toml` carries a per-repo `forges: Vec<ProviderKind>` field but v5 reads it only for identity-drift detection. Routing decisions (sync / push / set_archived / set_default_branch / clone / rename) iterate every URL in the org yaml's `Remote` list regardless of what the checkout's per-repo config says.

The user's mental model — *"this repo lives on github+codeberg; remove github from `forges` and v5 stops syncing it there"* — does not work. There's no way to scope a single repo's forge presence without removing the URL from the org yaml.

## Required behavior

**Per-repo `forges` becomes authoritative for routing.** When set on an `OrgRepo`, it filters which remotes participate in any operation that fans out across remotes.

**Schema additions:**

- `OrgRepo` (org yaml) gains optional `forges: Option<Vec<ProviderKind>>`. When `None`, behavior is unchanged (all remotes participate). When `Some([github])`, only remotes whose derived provider is `github` participate.
- `.hyperforge/config.toml`'s existing `forges` field stays — but it's now mirrored from the org yaml on `repos.init` (existing behavior) AND read back via a new `repos.sync_config` RPC.

**New RPC: `repos.sync_config --target_path P [--mode push|pull]`**

| mode | direction |
|---|---|
| `pull` (default) | Reads `<P>/.hyperforge/config.toml`, updates `OrgRepo.forges` + other declarable fields in the org yaml. User-edits-file workflow. |
| `push` | Writes the org yaml's `OrgRepo.forges` (etc.) into `<P>/.hyperforge/config.toml`. Useful after `repos.set_archived` etc. |

**Routing changes** — every site that iterates `repo.remotes` to call a `ForgePort` adapter checks the new `forges` filter first:

| Method | Filter applied? |
|---|---|
| `repos.sync` | yes |
| `repos.push` (metadata) | yes |
| `repos.set_archived`, `set_default_branch`, `rename` | yes (operates on canonical remote; if canonical's provider is excluded, errors with `forge_excluded`) |
| `repos.clone` | yes (chooses first remote whose provider is in `forges`; falls back to canonical if no match) |
| `workspaces.sync` | yes (each member uses its own filter) |
| `repos.fetch`, `repos.pull`, `repos.push_refs` | **no** — these operate on a path's `.git/config`, not the org yaml's remotes. The git CLI/git2 already honors whatever the local repo has configured. |

**Compatibility** — existing org yamls with no `forges` field on their repos behave exactly as before. The field is fully optional.

## What must NOT change

- v5 never deletes URLs from the org yaml's `remotes` list as a side effect of `forges` filtering. The exclusion is at the *operation* level, not the *registry* level. (Use `repos.remove_remote` to actually drop a remote.)
- D9, D13, D6 invariants.

## Acceptance criteria

1. Set `forges: ["github"]` on a repo with both github + codeberg remotes; `repos.sync` makes only the github API call.
2. Same setup, run `repos.set_archived --archived true` — only github gets the archive flag flipped; codeberg untouched.
3. With `forges: []` (empty list), every routing op emits `forge_excluded { ref, reason }` for every remote and exits cleanly without forge calls. (The user expressed "remove from all forges" intent.)
4. `repos.sync_config --target_path P --mode pull` after the user edits `.hyperforge/config.toml` updates the org yaml's `OrgRepo.forges`. Subsequent ops honor the change.
5. Round-trip: `repos.sync_config --mode push` followed by `repos.sync_config --mode pull` is a byte-identical no-op.
6. Org yamls with no per-repo `forges` field load unchanged; routing works exactly as before.

## Completion

- Run `bash tests/v5/V5PARITY/V5PARITY-34.sh` → exit 0 (tier 1; uses local bare repos for both forges via stub URL host overrides).
- Ready → Complete in-commit.
