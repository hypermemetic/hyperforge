# TRAK-1: Epic Overview — Recursive Work Tracker

blocked_by: []
unlocks: [TRAK-2, TRAK-3, TRAK-4, TRAK-5, TRAK-6, TRAK-7, TRAK-8, TRAK-9, TRAK-10]

## Core Abstraction: Facet

A **facet** is the recursive unit of work. It has an ID, a title, a status,
an owner. It contains other facets. It links to other facets. That's the
entire data model.

Everything else — comments, history, permissions, forks, external refs — is
a **service that takes a facet ID**. No special vocabulary for aspects of a
facet; they're just activations in the constellation.

## Architecture

```
trak (coordinator)
  trak.facet    — identity, tree, edges, content, search
  trak.discuss  — comments/threads per facet
  trak.audit    — revision history per facet (append-only)
  trak.access   — ownership, permissions, groups
  trak.collab   — forks, merge proposals
  trak.refs     — external references (PRs, commits, URLs)
```

Day 1: all activations in one process, one SQLite DB.
Later: any activation can split into its own service.
The API doesn't change either way.

## Facet data model

```rust
struct Facet {
    uuid: Uuid,
    title: String,
    body: Option<String>,           // markdown
    status: String,                 // user-defined
    labels: Vec<String>,
    assignee: Option<String>,
    priority: Option<i32>,
    owner: String,                  // from auth
    forked_from: Option<Uuid>,      // lineage
    revision: u64,                  // monotonic
    metadata: Value,                // extensible
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
```

## In-memory: FacetIndex (topology only)

```rust
struct FacetIndex {
    nodes: SlotMap<FacetId, FacetMeta>,     // ~100 bytes/node
    parent: SecondaryMap<FacetId, FacetId>,
    children: SecondaryMap<FacetId, SmallVec<[FacetId; 8]>>,
    edges_out: SecondaryMap<FacetId, SmallVec<[EdgeRef; 4]>>,
    edges_in: SecondaryMap<FacetId, SmallVec<[EdgeRef; 4]>>,
    roots: Vec<FacetId>,
    uuid_to_id: HashMap<Uuid, FacetId>,
}

struct FacetMeta {
    uuid: Uuid,
    title: String,
    status: String,
    owner: String,
    child_count: u32,
}
```

100K facets ≈ 10MB. Graph traversals run in-memory. Content, comments,
history loaded on demand from backend.

## Wire surface

```
trak.facet.create       --title --parent? --status? --body? --labels? --assignee? --priority?
trak.facet.get          --id
trak.facet.update       --id --title? --status? --body? --labels? --assignee? --priority?
trak.facet.delete       --id --cascade?
trak.facet.move         --id --new_parent?
trak.facet.list         --parent? --status? --label? --assignee? --depth? --limit?
trak.facet.search       --query --status? --label? --limit?
trak.facet.tree         --id? --depth? --status? --collapse_done?
trak.facet.link         --from --to --kind
trak.facet.unlink       --from --to --kind?
trak.facet.links        --id --kind? --direction?
trak.facet.graph        --id --kind? --depth? --direction?
trak.facet.critical_path --id
trak.facet.blocked      --parent? --status?
trak.facet.fork         --id --deep?
trak.facet.forks        --id
trak.facet.context      --id --depth?

trak.discuss.comment    --facet --body --author?
trak.discuss.list       --facet --limit?

trak.audit.history      --facet --limit? --action?

trak.access.grant       --facet --principal --role
trak.access.revoke      --facet --principal
trak.access.who_can     --facet
trak.access.set_visibility --facet --visibility

trak.collab.propose     --source --target --message?
trak.collab.merge       --proposal
trak.collab.reject      --proposal --reason?
trak.collab.proposals   --facet --status?

trak.refs.add           --facet --kind --url --provider? --title?
trak.refs.remove        --facet --url
trak.refs.list          --facet --kind?
```

## Backend trait

```rust
trait TrakBackend: Send + Sync {
    async fn load_index(&self) -> Result<FacetIndex>;
    async fn apply_op(&self, op: &FacetOp) -> Result<()>;
    async fn load_content(&self, uuid: &Uuid) -> Result<Option<Facet>>;
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Uuid>>;
    // Facet-scoped queries for each service:
    async fn load_comments(&self, uuid: &Uuid, limit: Option<u32>) -> Result<Vec<Comment>>;
    async fn load_history(&self, uuid: &Uuid, limit: Option<u32>, action: Option<&str>) -> Result<Vec<HistoryEntry>>;
    async fn load_permissions(&self, uuid: &Uuid) -> Result<FacetPermissions>;
    async fn load_refs(&self, uuid: &Uuid, kind: Option<&str>) -> Result<Vec<ExternalRef>>;
    async fn load_forks(&self, uuid: &Uuid) -> Result<Vec<FacetMeta>>;
    async fn load_proposals(&self, uuid: &Uuid, status: Option<&str>) -> Result<Vec<MergeProposal>>;
}
```

## Dependency DAG

```
TRAK-2 (FacetIndex + Backend trait + SQLite)
  ├── TRAK-3 (facet hub: CRUD + tree + edges)
  │     ├── TRAK-5 (discuss hub: comments)
  │     ├── TRAK-6 (audit hub: history)
  │     ├── TRAK-7 (refs hub: external references)
  │     ├── TRAK-8 (access hub: permissions)
  │     └── TRAK-9 (collab hub: fork + merge)
  ├── TRAK-4 (search + graph queries)
  └── TRAK-10 (context export)
        needs: all of 5-9
```

Phase 1: TRAK-2 (foundation)
Phase 2: TRAK-3, TRAK-4 (parallel — core facet ops)
Phase 3: TRAK-5, TRAK-6, TRAK-7, TRAK-8, TRAK-9 (parallel — all services)
Phase 4: TRAK-10 (context export, terminal)

## Design principles

1. **A facet is a reference, not a value.** You never load "all of a facet."
   You load the aspect you need.

2. **The index is topology.** In-memory graph of IDs, statuses, and edges.
   Everything else is on-demand from the backend.

3. **Services share a UUID namespace.** discuss, audit, access, collab, refs
   all take a facet UUID. No foreign keys between services — just IDs.

4. **Auth is pervasive.** Every mutation checks permissions. Every write
   records the actor. owner + permissions set on creation, mutable by admins.

5. **The API is the decomposition boundary.** Day 1: one process. Day N:
   split any service out. Wire surface doesn't change.

6. **Status is a string.** No state machine. Different workflows use
   different status values.

7. **Edges are typed and directional.** depends_on, blocks, relates_to,
   duplicates, or any custom kind. The system stores them; interpreters
   give them meaning.
