# CLEANUP-8: Final verification — all workspace tests pass

blocked_by: [CLEANUP-2, CLEANUP-3, CLEANUP-4, CLEANUP-5, CLEANUP-6, CLEANUP-7]
unlocks: []
difficulty: trivial

## Acceptance criteria

Run full workspace test suite:
```bash
synapse lforge hyperforge workspace exec \
  --path ~/dev/controlflow/hypermemetic \
  --command "cargo test 2>&1 | tail -5"
```

Expected: all 16 repos report 0 failures.

Also verify Haskell projects:
```bash
cd synapse && cabal test
cd plexus-protocol && cabal test
```
