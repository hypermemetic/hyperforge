# MFORGE-1: Epic Overview — Multi-Forge Orgs

blocked_by: []
unlocks: [MFORGE-2, MFORGE-3, MFORGE-4, MFORGE-5, MFORGE-6, MFORGE-7, MFORGE-8]

## Goal

Replace single-provider org config (`forge: ForgeBlock`) with multi-forge
(`forges: BTreeMap<ProviderKind, ForgeProviderBlock>`), enabling one org to
span GitHub + Codeberg + GitLab with per-forge credentials.

## Dependency DAG

```
MFORGE-1 (schema + migration)
  ├── MFORGE-2 (ops::repo per-provider cred resolution)
  │     ├── MFORGE-4 (repos sync/push/delete per-remote dispatch)
  │     └── MFORGE-5 (multi-forge import + bootstrap) [also needs MFORGE-3]
  ├── MFORGE-3 (orgs hub CRUD adaptation)
  │     ├── MFORGE-5
  │     └── MFORGE-7 (wire shape migration)
  └── MFORGE-6 (auth_check/requirements/ssh helpers)
        └── MFORGE-8 (E2E integration) [also needs 4,5,7]
```

## Phases

**Phase 1 — Foundation (MFORGE-1):** Schema change + backward-compat deserialization.
Can be done in isolation; all downstream tickets depend on this.

**Phase 2 — Parallel adaptation (MFORGE-2, MFORGE-3, MFORGE-6):** Three independent
streams adapting ops layer, orgs hub, and auth helpers to the new shape.

**Phase 3 — Integration (MFORGE-4, MFORGE-5, MFORGE-7):** Repos dispatch, import/bootstrap,
and wire shape — all depend on Phase 2 outputs.

**Phase 4 — Validation (MFORGE-8):** E2E tests composing all pieces.

## Design invariants

- Backward compat: old `forge:` YAML deserializes into single-entry `forges:` map
- Serialize always writes `forges:` (new format)
- Per-repo `forges: [github, codeberg]` (V5PARITY-34) is leveraged, not duplicated
- ops:: layer boundary (D13) maintained
- Atomic writes (D8) maintained
- Secret redaction (D9) maintained
