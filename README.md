# Hyperforge

Multi-forge repository management — declarative YAML config, typed RPC over WebSocket, ground-up rewrite as of v5.0.0.

Hyperforge syncs repositories across GitHub, Codeberg, and GitLab via direct REST APIs. It tracks your orgs, workspaces, and credentials in `~/.config/hyperforge/`, exposes ~80 RPC methods over a single daemon, and routes every git operation through a typed abstraction (subprocess for network ops, libgit2 for local).

## Install

```bash
cargo install --path .   # produces hyperforge, hyperforge-auth, hyperforge-ssh, hyperforge-legacy
```

## Quick start

If you've already authenticated with `gh`, the entire onboarding is two commands per org:

```bash
# Daemon
hyperforge --port 44104 --config-dir ~/.config/hyperforge &

# Onboarding (one RPC composes secret + org + credential + import)
synapse -P 44104 --json lforge hyperforge orgs bootstrap \
    --name <my-org> --provider github \
    --token gh-token:// --use_default_token true

# Materialize a checkout for everything tracked
synapse -P 44104 --json lforge hyperforge workspaces from_org \
    --org <my-org> --target_path ~/code/<my-org>
```

That's the whole flow. See `docs/v5/getting-started.md` for the long version.

## Binaries

| Binary | Default port | Role |
|---|---|---|
| `hyperforge` | 44104 | Daemon: orgs, repos, workspaces, secrets, build (the canonical v5 surface) |
| `hyperforge-auth` | — | Secrets sidecar (YAML-backed secret store; v5 embeds it but the standalone binary is preserved) |
| `hyperforge-ssh` | — | SSH key management CLI (`V5PARITY-31` — currently only available on `hyperforge-legacy`; v5 covers the runtime via `repos.set_ssh_key`) |
| `hyperforge-legacy` | 44104 | The pre-5.0.0 daemon, preserved for one release cycle |

## Architecture at a glance

```
hyperforge (port 44104)
├─ HyperforgeHub (root)            ← status, begin, auth_*, config_*
│  ├─ OrgsHub      → orgs.*        ← list, create, bootstrap, set_credential
│  ├─ ReposHub     → repos.*       ← clone, fetch, pull, status, register, sync, …
│  ├─ WorkspacesHub → workspaces.* ← from_org, status, checkout, commit, tag, diff
│  ├─ SecretsHub   → secrets.*     ← set, list_refs, delete
│  └─ BuildHub     → build.*       ← unify, release, dist_init, run, exec
├─ ops::*                          ← typed wrappers (state, git, external_auth)
└─ adapters::*                     ← ForgePort: github, codeberg, gitlab
```

## CLI invocation

The daemon's namespace is `lforge` (Plexus naming — normalized from
the transitional `lforge-v5` once v4 retired to `lforge-deprecated`).
Two equivalent forms:

```bash
# Standalone daemon (v5 default — recommended)
synapse -P 44104 --json lforge hyperforge <namespace> <method> --param value …

# When embedded in a substrate Plexus server
synapse substrate hyperforge <namespace> <method> --param value …
```

## Common operations

```bash
# Status across a workspace
synapse … workspaces status --name <ws>

# Pull every member
synapse … workspaces pull --name <ws>

# Create a coordinated tag across the whole workspace
synapse … workspaces tag --name <ws> --tag v0.5.0 --message "release"

# Adopt an existing local checkout into the registry
synapse … repos register --target_path ~/code/some-orphan

# Cut a release on a single repo (bump + tag + push + optional publish)
synapse … build release --org <org> --name <repo> --bump patch
```

## Config layout

```
~/.config/hyperforge/
├── config.yaml                    # provider_map, default_workspace, owner_aliases
├── secrets.yaml                   # secrets://… resolved here (YAML-backed)
├── orgs/<org>.yaml                # one file per org: provider + credentials + repos
└── workspaces/<name>.yaml         # one file per workspace: name + path + members
```

Per-repo identity lives in `<repo>/.hyperforge/config.toml`. Distribution config lives in `<repo>/.hyperforge/dist.toml`.

### Registry conventions

The registry is two layers of YAML, and they answer different questions:

- **`orgs/<org>.yaml` — the inventory.** One file per org (or user). It carries the
  provider, the `credentials[]` refs, and every repo the org owns inline under `repos:`.
  Each repo lists its `remotes[]`, and each remote carries its own `provider:` — so a
  single repo mirrored to both GitHub and Codeberg is one entry with two remotes. This is
  the complete set of projects hyperforge tracks, whether or not they're checked out
  locally.
- **`workspaces/<name>.yaml` — the checked-out subset.** One file per workspace: a `path`
  and a list of members, each an `<org>/<name>` ref into the registry. A workspace is the
  slice of the inventory that lives on disk at `path`.

"Complete" (the bar HYPE-8 set) means the registry equals remote reality: every project
across every org/user you own is in some `orgs/*.yaml` under its **canonical** owner, and
every local checkout is a member of some workspace. `repos doctor` (below) is what keeps
it complete as repos move.

### Owner aliases

When a repo moves from a user to an org (e.g. GitHub user `hypermemetic` →
GitHub org `hypermemetic-ai`), the two owner strings name **one** identity. `owner_aliases`
in `config.yaml` teaches hyperforge that, so every owner comparison treats them as equal
instead of flagging a false divergence or blocking a push:

```yaml
# ~/.config/hyperforge/config.yaml
owner_aliases:
  hypermemetic-ai:        # canonical owner (the org)
    - hypermemetic        # alias(es) that resolve to it (the old user)
```

The table is `canonical → [aliases]`. It feeds `canonical_owner()` / `same_owner()`, which
are consulted at every owner-comparison site:

- **register / adopt** file a repo under the canonical owner, not the raw URL-derived one.
- **drift / sync** matching compares canonical owners, so an aliased repo reads in-sync.
- the **pre-push hook** compares `canonical_owner(url_org)` against the declared org — an
  aliased owner passes; a genuinely unrelated org still blocks.
- **`repos publish --status`** reports `same_identity: true` for an aliased remote.

Comparison is case-sensitive: `owner_aliases` is the place to reconcile a case difference
too (e.g. list `juggernautlabs` as an alias of canonical `JuggernautLabs`).

## Publishing

Two different things are called "publish"; they never overlap, and both are
capability-not-act (explicit, dry-run-first — nothing pushes as a side effect):

- **`repos publish` — git.** Pushes a branch to the forge remotes.
- **`build publish` — artifacts.** Publishes a built package to a package registry
  (crates.io / npm / pypi), token drawn from the secret store:

  ```bash
  synapse -P 44104 --json lforge hyperforge build publish \
      --org <org> --name <repo> --channel crates.io
  ```

  `--channel` ∈ `{crates.io, npm, pypi}` (default `crates.io`). Tokens resolve from
  `secrets://cargo/token`, `secrets://npm/token`, `secrets://pypi/token`. The crates.io
  path runs `cargo publish --allow-dirty`.

### `repos publish` — mirror-aware git push

Most repos here are **mirrored**: one repo carries both a GitHub and a Codeberg remote,
and the remote *names* aren't consistent across the fleet (Pattern A has `origin` =
codeberg, Pattern B has `origin` = github). A plain `git push` in that world pushes to
exactly one remote — whichever `origin` happens to be — silently leaving the mirror
behind. `repos publish` reads the actual remote names from each repo's `.git/config` and
pushes the **same branch to every configured forge remote**, so the mirrors stay in
lockstep. Owner comparison throughout is alias-aware (see above).

Three modes, dry-run by default:

```bash
# 1. --status — read-only inventory: ahead/behind per forge remote, no push, no fetch
synapse -P 44104 --json lforge hyperforge repos publish --status

# 2. default (no flag) — dry-run plan: one line per repo/remote, pushes nothing
synapse -P 44104 --json lforge hyperforge repos publish --org <org> --name <repo>

# 3. --execute — actually push
synapse -P 44104 --json lforge hyperforge repos publish --org <org> --name <repo> --execute
```

Scope with `--workspace <ws>` / `--org <org>` / `--name <repo>` (default: the
`default_workspace`), or `--path <dir>` to target a single checkout.

- **`--status`** emits one `publish_status_entry` per forge remote — the `.git/config`
  remote name, provider, `ahead`/`behind` (vs the last-known remote-tracking ref — **no
  network fetch**, so the `publish_summary` carries `fetched: false`), and an alias-aware
  `same_identity`. Zero mutations. A recent live run: *59 repos / 94 forge remotes
  scanned, 21 repos with unpushed work, 392 commits ahead in total.*
- **default** emits one `publish_plan` per repo/remote (remote, provider, URL, branch) and
  pushes nothing.
- **`--execute`** runs `git push <remote> <branch>` per forge remote — **never `--force`,
  never `--no-verify`**, so the alias-aware pre-push hook still fires and is respected. A
  remote that 404s is skipped (`publish_skipped`) and the run continues; any other failure
  (hook block, non-fast-forward) emits `publish_error` and leaves that remote's history
  untouched.

The first `--execute` is operator-invoked; nothing pushes outward on its own.

### `repos doctor` / `sync --heal` — keep the registry honest

`repos doctor` asks the forge for each repo's canonical identity and reports whether the
registry still agrees. It resolves the first in-scope remote's `full_name` via
`gh api repos/OWNER/NAME` **following HTTP redirects** (a `git ls-remote` fallback when
`gh` is unavailable), then compares canonical owners.

```bash
# Report only (read-only): does the registry still match the forge?
synapse -P 44104 --json lforge hyperforge repos doctor --org <org>

# Repair a real divergence (writes the registry + rewrites the moved provider's remote)
synapse -P 44104 --json lforge hyperforge repos doctor --org <org> --heal
```

Each repo gets a `verdict`:

| verdict | meaning |
|---|---|
| `clean` | registry owner matches the forge's canonical owner (an **alias** folds to canonical, so an aliased repo reads `clean` — it is not a divergence) |
| `diverged` | the forge's canonical owner differs from the registry's — a real move |
| `unknown` | no in-scope remote could answer (e.g. codeberg-first repos whose github remote isn't queried) — reported, never healed |

`doctor` is read-only; `--heal` acts (add `--dry_run` to preview the heal). Healing reuses
the existing `migrate_one`, fixed to be **provider-scoped**: it rewrites only the moved
provider's remote(s) — using the remote *names* read from `.git/config`, not a
provider-convention guess — and leaves every other forge's URL byte-identical (the bug
that once corrupted codeberg URLs). Every repair is recorded as an `old_full_name →
new_full_name` entry in `doctor-renames.json` in the config dir. `sync --heal` is the same
repair reachable from the per-repo sync path.

## Migrating from v4

If you ran v4: see `MIGRATION.md`. Short version:
- `secrets.yaml` is file-compatible across versions.
- `orgs/<name>/repos.yaml` (v4 LocalForge) is **not** read by v5; v5 writes a single `orgs/<name>.yaml` with the repo registry inline.
- The `hyperforge` binary is now v5; the v4 daemon is preserved one release as `hyperforge-legacy`.

## Status

Production-ready for daily use. Twenty-five `V5PARITY-*` tickets shipped (see `plans/v5/V5PARITY/`); four v4-only features remain queued (release-asset upload, gitignore-sync, workspace check/verify, hyperforge-ssh CLI). Everything else from v4 is reachable.

## License

MIT
