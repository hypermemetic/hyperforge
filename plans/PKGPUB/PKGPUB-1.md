# PKGPUB-1: Epic Overview — Package Publishing Parity

blocked_by: []
unlocks: [PKGPUB-2, PKGPUB-3, PKGPUB-4, PKGPUB-5, PKGPUB-6, PKGPUB-7]

## Goal

Bring v5's package publishing to parity with v4. v4 had workspace-wide
dependency-ordered publishing, registry version comparison, hackage support,
and auto-bump for drifted packages. v5 has per-repo publish but no
orchestration layer.

## Current state (v5)

- `build.publish` — works per-repo for crates.io, npm, pypi (real shell execution)
- `build.bump` — edits Cargo.toml version, commits + tags
- `build.release` — combines bump + push + optional publish
- `build.release_all` — iterates workspace members but NO dependency ordering
- `build.package_diff` — git-ref-based manifest diff, NOT registry comparison
- No hackage support
- No workspace-wide publish with transitive dep resolution

## v4 capabilities to match

1. `package_diff` comparing local vs registry (crates.io, hackage) — "ahead", "stale", "drifted", "unpublished"
2. Workspace-wide `publish` with dependency topo-sort
3. Transitive dep auto-publish (publishing B auto-publishes A if B depends on A)
4. Auto-bump for drifted packages (same version, changed source → patch bump)
5. Hackage publish support (cabal sdist + cabal upload)
6. `--include`/`--exclude` glob filters on workspace publish
7. Dry-run default, `--execute` to actually publish

## Dependency DAG

```
PKGPUB-2 (registry version comparison)
  └── PKGPUB-4 (workspace publish with dep ordering) [also needs PKGPUB-3]
PKGPUB-3 (dep graph topo-sort)
  └── PKGPUB-4
PKGPUB-5 (hackage support)
PKGPUB-6 (auto-bump drifted)    [needs PKGPUB-2]
PKGPUB-7 (workspace filters)    [needs PKGPUB-4]
```

Parallelizable: PKGPUB-2, PKGPUB-3, PKGPUB-5 can all start immediately.
