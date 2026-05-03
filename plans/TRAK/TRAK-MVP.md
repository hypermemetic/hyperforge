# TRAK-MVP: Minimal Viable Facet Tracker

blocked_by: []
unlocks: everything else

## Goal

First usable cut. Create, nest, list, update, link facets. In-memory only.
The crate layout is FINAL — adding features means filling in files, never
moving them.

## Crate layout (day 1 = final structure)

```
plexus-trak/
├── Cargo.toml
├── src/
│   ├── lib.rs                      // pub mod declarations + re-exports
│   ├── types.rs                    // Facet, Edge, EdgeKind, FacetMeta
│   │
│   ├── index/
│   │   ├── mod.rs                  // FacetIndex (topology graph)
│   │   └── traversal.rs           // tree walk, graph BFS/DFS, critical path
│   │
│   ├── store/
│   │   ├── mod.rs                  // FacetStore trait (the backend contract)
│   │   ├── memory.rs              // MemoryStore (MVP — HashMap, no persistence)
│   │   └── sqlite.rs             // [STUB] SqliteStore (future)
│   │
│   ├── hubs/
│   │   ├── mod.rs                  // re-exports all hubs
│   │   ├── facet.rs               // FacetHub — CRUD, tree, edges, fork
│   │   ├── discuss.rs             // [STUB] DiscussHub — comments
│   │   ├── audit.rs              // [STUB] AuditHub — history
│   │   ├── access.rs             // [STUB] AccessHub — permissions
│   │   ├── collab.rs             // [STUB] CollabHub — forks, merge proposals
│   │   └── refs.rs               // [STUB] RefsHub — external references
│   │
│   ├── events.rs                   // TrakEvent enum (all event variants, all hubs)
│   │
│   └── bin/
│       └── main.rs                 // standalone daemon entry point
│
└── tests/
    └── facet_test.rs              // integration tests
```

### What "STUB" means

A stub file contains:
- The struct definition
- The `#[plexus_macros::activation]` attribute with correct namespace
- Method signatures that return `Error { code: "not_implemented" }`
- Zero logic

This means synapse discovers the full API surface on day 1. `synapse trak`
shows all sub-hubs. `synapse trak discuss` shows methods. They just return
"not implemented" until we fill them in.

## What's real in MVP

### types.rs — REAL

```rust
pub struct Facet {
    pub uuid: Uuid,
    pub title: String,
    pub body: Option<String>,
    pub status: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub priority: Option<i32>,
    pub owner: String,
    pub forked_from: Option<Uuid>,
    pub revision: u64,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct FacetMeta {
    pub uuid: Uuid,
    pub title: String,
    pub status: String,
    pub owner: String,
    pub child_count: u32,
}

pub struct Edge {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: EdgeKind,
    pub created_at: DateTime<Utc>,
}

pub enum EdgeKind {
    DependsOn,
    Blocks,
    RelatesTo,
    Duplicates,
    Custom(String),
}
```

### store/mod.rs — REAL (trait definition)

```rust
#[async_trait]
pub trait FacetStore: Send + Sync {
    // --- Facets ---
    async fn create_facet(&self, facet: &Facet, parent: Option<Uuid>) -> Result<(), StoreError>;
    async fn get_facet(&self, uuid: &Uuid) -> Result<Option<Facet>, StoreError>;
    async fn update_facet(&self, facet: &Facet) -> Result<(), StoreError>;
    async fn delete_facet(&self, uuid: &Uuid, cascade: bool) -> Result<u32, StoreError>;
    async fn move_facet(&self, uuid: &Uuid, new_parent: Option<Uuid>) -> Result<(), StoreError>;
    async fn list_children(&self, parent: Option<Uuid>, limit: u32, offset: u32) -> Result<Vec<Facet>, StoreError>;
    async fn list_roots(&self) -> Result<Vec<Facet>, StoreError>;
    async fn get_ancestors(&self, uuid: &Uuid) -> Result<Vec<FacetMeta>, StoreError>;
    async fn get_subtree(&self, root: &Uuid, depth: Option<u32>) -> Result<Vec<(Facet, u32)>, StoreError>;

    // --- Edges ---
    async fn add_edge(&self, edge: &Edge) -> Result<(), StoreError>;
    async fn remove_edge(&self, source: &Uuid, target: &Uuid, kind: Option<&EdgeKind>) -> Result<u32, StoreError>;
    async fn get_edges(&self, uuid: &Uuid, direction: Direction, kind: Option<&EdgeKind>) -> Result<Vec<Edge>, StoreError>;

    // --- Search ---
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Facet>, StoreError>;

    // --- Bulk ---
    async fn count_children(&self, uuid: &Uuid) -> Result<u32, StoreError>;
}

pub enum Direction { Outgoing, Incoming, Both }

pub enum StoreError {
    NotFound(Uuid),
    HasChildren(Uuid),
    CycleDetected,
    Internal(String),
}
```

### store/memory.rs — REAL (MVP implementation)

```rust
pub struct MemoryStore {
    facets: RwLock<HashMap<Uuid, Facet>>,
    children: RwLock<HashMap<Option<Uuid>, Vec<Uuid>>>,  // parent → children
    parents: RwLock<HashMap<Uuid, Option<Uuid>>>,        // child → parent
    edges: RwLock<Vec<Edge>>,
}
```

All methods implemented against these HashMaps. No persistence.
State lives as long as the process.

### hubs/facet.rs — REAL (MVP methods only)

```rust
/// Recursive facet tracker — create, nest, link, traverse
#[plexus_macros::activation(namespace = "facet")]
impl FacetHub {
    /// Create a new facet
    async fn create(&self, title: String, parent: Option<String>, ...) -> Stream

    /// Get a facet by ID
    async fn get(&self, id: String) -> Stream

    /// Update facet fields
    async fn update(&self, id: String, title: Option<String>, ...) -> Stream

    /// Delete a facet
    async fn delete(&self, id: String, cascade: Option<bool>) -> Stream

    /// Move a facet to a new parent
    async fn move_to(&self, id: String, new_parent: Option<String>) -> Stream

    /// List facets (children of parent, or roots)
    async fn list(&self, parent: Option<String>, status: Option<String>, ...) -> Stream

    /// Walk the facet tree recursively
    async fn tree(&self, id: Option<String>, depth: Option<u32>, ...) -> Stream

    /// Create a typed link between facets
    async fn link(&self, from: String, to: String, kind: String) -> Stream

    /// Remove a link
    async fn unlink(&self, from: String, to: String, kind: Option<String>) -> Stream

    /// List links for a facet
    async fn links(&self, id: String, direction: Option<String>, ...) -> Stream

    /// Find facets blocked by unresolved dependencies
    async fn blocked(&self, parent: Option<String>) -> Stream

    /// Full-text search
    async fn search(&self, query: String, ...) -> Stream
}
```

### hubs/discuss.rs — STUB

```rust
/// Comments and threads on facets
#[plexus_macros::activation(namespace = "discuss")]
impl DiscussHub {
    /// Add a comment to a facet
    async fn comment(&self, facet: String, body: String) -> Stream {
        stream! { yield TrakEvent::Error { code: "not_implemented".into(), message: "discuss.comment not yet implemented".into() }; }
    }
    /// List comments on a facet
    async fn list(&self, facet: String) -> Stream { /* stub */ }
}
```

Same pattern for audit, access, collab, refs.

### events.rs — REAL (full enum, all variants)

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrakEvent {
    // --- Facet events ---
    FacetCreated { facet: Facet },
    FacetUpdated { facet: Facet, changes: Vec<String> },
    FacetDeleted { id: String, children_deleted: u32 },
    FacetMoved { id: String, old_parent: Option<String>, new_parent: Option<String> },
    FacetDetail { facet: Facet, child_count: u32, link_count: u32 },
    FacetSummary { meta: FacetMeta, depth: u32 },
    LinkCreated { from: String, to: String, kind: String },
    LinkRemoved { from: String, to: String, count: u32 },
    LinkDetail { source: FacetMeta, target: FacetMeta, kind: String },
    Blocked { facet: FacetMeta, blocked_by: Vec<FacetMeta> },
    SearchResult { facet: FacetMeta, snippet: Option<String> },
    ListSummary { total: u32, returned: u32 },

    // --- Discuss events (stub) ---
    CommentAdded { facet_id: String, comment_id: String, body: String },
    CommentList { facet_id: String, comments: Vec<serde_json::Value> },

    // --- Audit events (stub) ---
    HistoryEntry { facet_id: String, action: String, timestamp: String },
    HistoryList { facet_id: String, entries: Vec<serde_json::Value> },

    // --- Access events (stub) ---
    PermissionGranted { facet_id: String, principal: String, role: String },
    PermissionRevoked { facet_id: String, principal: String },
    AccessInfo { facet_id: String, visibility: String, writers: Vec<String>, admins: Vec<String> },

    // --- Collab events (stub) ---
    Forked { source: String, fork: String, owner: String },
    ProposalCreated { id: String, source: String, target: String },
    ProposalMerged { id: String },
    ProposalRejected { id: String },

    // --- Refs events (stub) ---
    RefAdded { facet_id: String, kind: String, url: String },
    RefRemoved { facet_id: String, url: String },
    RefList { facet_id: String, refs: Vec<serde_json::Value> },

    // --- Common ---
    Error { code: String, message: String },
    Info { message: String },
}
```

### bin/main.rs — REAL

```rust
use plexus_trak::hubs::{FacetHub, DiscussHub, AuditHub, AccessHub, CollabHub, RefsHub};

#[tokio::main]
async fn main() {
    let store = Arc::new(MemoryStore::new());
    let trak = DynamicHub::new("trak")
        .register(FacetHub::new(store.clone()))
        .register(DiscussHub::new(store.clone()))
        .register(AuditHub::new(store.clone()))
        .register(AccessHub::new(store.clone()))
        .register(CollabHub::new(store.clone()))
        .register(RefsHub::new(store.clone()));

    TransportServer::builder(Arc::new(trak), |a| DynamicHub::arc_into_rpc_module(a))
        .with_websocket(44107)
        .build().await.unwrap()
        .serve().await.unwrap();
}
```

## Additive upgrade path

| Feature | What changes | What DOESN'T change |
|---------|-------------|---------------------|
| SQLite persistence | Fill in `store/sqlite.rs`, swap store in main.rs | types, hubs, events, trait |
| Comments | Fill in `hubs/discuss.rs` body | everything else |
| History | Fill in `hubs/audit.rs` body | everything else |
| Permissions | Fill in `hubs/access.rs` body, add check in facet hub | types, events, store trait |
| Forks | Fill in `hubs/collab.rs` body | everything else |
| Search (FTS) | Add to sqlite.rs, MemoryStore gets naive impl | hub API unchanged |
| FacetIndex (SlotMap) | New `index/` module, facet hub uses it for traversals | store trait, events, types |
| Separate services | Each hub becomes its own binary | wire surface unchanged |

Nothing moves. No renames. No restructuring. Just fill.

## Tests (MVP)

### `test_create_and_get`
Create facet, get by ID. Assert fields match.

### `test_parent_child`
Create parent, create child. List children of parent. Assert found.

### `test_tree`
Create 3-level tree. Call tree. Assert correct depths.

### `test_update`
Create, update status. Get. Assert new status.

### `test_delete_cascade`
Parent + children. Delete cascade=true. Assert all gone.

### `test_link_and_blocked`
A depends_on B (status=open). blocked(). Assert A listed.

### `test_move`
Create child under A. Move to B. Assert parent changed.

### `test_search`
Create 3 facets with "deploy" in title. Search "deploy". Assert 3 results.

## Acceptance

```bash
$ synapse -P 44107 trak facet create --title "build plexus-trak"
type: facet_created
facet:
  uuid: abc-123
  title: build plexus-trak
  status: open

$ synapse -P 44107 trak facet create --title "MVP" --parent abc-123
type: facet_created
facet:
  uuid: def-456
  title: MVP
  status: open

$ synapse -P 44107 trak facet tree
type: facet_summary
meta: {uuid: abc-123, title: "build plexus-trak", status: "open"}
depth: 0

type: facet_summary
meta: {uuid: def-456, title: "MVP", status: "open"}
depth: 1
```
