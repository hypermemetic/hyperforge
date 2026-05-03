# TRAK-2: Core Types, FoldGraph, Backend Trait, SQLite Backend

blocked_by: []
unlocks: [TRAK-3, TRAK-4]

## Scope

Define the Fold type, the FoldGraph in-memory structure, the pluggable
Backend trait, and the first backend implementation (SQLite).

## Architecture

```
Hub (RPC)
  │
  ▼
FoldGraph (in-memory graph, the primary data structure)
  │  ▲
  │  │ load / flush
  ▼  │
Backend trait (async persistence)
  │
  ▼
SqliteBackend (first implementation)
```

The FoldGraph is NOT a cache over a database. It IS the data structure.
The backend is durability — it serializes snapshots of the graph to disk
and loads them back. All queries, traversals, and mutations happen against
the in-memory graph.

## Types (`src/types.rs`)

```rust
use slotmap::{SlotMap, SecondaryMap, new_key_type};
use smallvec::SmallVec;

new_key_type! { pub struct FoldId; }

/// A fold — the recursive unit of work.
pub struct Fold {
    pub uuid: Uuid,             // stable external identifier
    pub title: String,
    pub body: Option<String>,   // markdown
    pub status: String,         // user-defined: "open", "in_progress", "done", ...
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub priority: Option<i32>,  // lower = higher priority
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: serde_json::Value,  // arbitrary KV for adapters
}

/// A typed edge between folds (NOT containment — that's the tree).
pub struct Edge {
    pub target: FoldId,
    pub kind: EdgeKind,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
}

/// Edge kinds — open enum serialized as string.
pub enum EdgeKind {
    DependsOn,
    Blocks,
    RelatesTo,
    Duplicates,
    Custom(String),
}

/// External reference (PR, commit, URL, container spec).
pub struct ExternalRef {
    pub id: Uuid,
    pub kind: String,           // "pr", "commit", "url", "container", "doc"
    pub url: String,
    pub provider: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Comment on a fold.
pub struct Comment {
    pub id: Uuid,
    pub author: Option<String>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Linear history entry.
pub struct HistoryEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub actor: Option<String>,
    pub action: String,         // "created", "updated", "linked", "commented", ...
    pub diff: serde_json::Value,
}
```

## FoldGraph (`src/graph.rs`)

The primary in-memory data structure. All queries run against this.

```rust
pub struct FoldGraph {
    // --- Core storage ---
    pub(crate) folds: SlotMap<FoldId, Fold>,
    pub(crate) uuid_to_id: HashMap<Uuid, FoldId>,

    // --- Containment tree ---
    pub(crate) parent: SecondaryMap<FoldId, FoldId>,
    pub(crate) children: SecondaryMap<FoldId, SmallVec<[FoldId; 8]>>,
    pub(crate) roots: Vec<FoldId>,  // nodes with no parent

    // --- Typed edges (dependency graph) ---
    pub(crate) edges_out: SecondaryMap<FoldId, SmallVec<[Edge; 4]>>,
    pub(crate) edges_in: SecondaryMap<FoldId, SmallVec<[Edge; 4]>>,

    // ---附属 data (per-fold, separate for memory efficiency) ---
    pub(crate) comments: HashMap<FoldId, Vec<Comment>>,
    pub(crate) history: HashMap<FoldId, Vec<HistoryEntry>>,
    pub(crate) refs: HashMap<FoldId, Vec<ExternalRef>>,
}

impl FoldGraph {
    pub fn new() -> Self;

    // --- Fold CRUD ---
    pub fn insert(&mut self, fold: Fold, parent: Option<FoldId>) -> FoldId;
    pub fn get(&self, id: FoldId) -> Option<&Fold>;
    pub fn get_mut(&mut self, id: FoldId) -> Option<&mut Fold>;
    pub fn remove(&mut self, id: FoldId, cascade: bool) -> Result<u32, GraphError>;
    pub fn reparent(&mut self, id: FoldId, new_parent: Option<FoldId>) -> Result<(), GraphError>;
    pub fn resolve(&self, uuid: &Uuid) -> Option<FoldId>;

    // --- Tree queries ---
    pub fn children_of(&self, id: FoldId) -> &[FoldId];
    pub fn parent_of(&self, id: FoldId) -> Option<FoldId>;
    pub fn ancestors(&self, id: FoldId) -> Vec<FoldId>;  // root-first
    pub fn descendants(&self, id: FoldId, depth: Option<u32>) -> Vec<(FoldId, u32)>;
    pub fn roots(&self) -> &[FoldId];
    pub fn depth(&self, id: FoldId) -> u32;
    pub fn subtree_count(&self, id: FoldId) -> u32;

    // --- Edge operations ---
    pub fn add_edge(&mut self, source: FoldId, target: FoldId, kind: EdgeKind) -> Result<(), GraphError>;
    pub fn remove_edge(&mut self, source: FoldId, target: FoldId, kind: Option<&EdgeKind>) -> u32;
    pub fn edges_from(&self, id: FoldId, kind: Option<&EdgeKind>) -> &[Edge];
    pub fn edges_to(&self, id: FoldId, kind: Option<&EdgeKind>) -> Vec<&Edge>;

    // --- Graph traversals ---
    pub fn traverse(&self, start: FoldId, kind: Option<&EdgeKind>, direction: Direction, depth: Option<u32>) -> Vec<(FoldId, u32)>;
    pub fn critical_path(&self, target: FoldId) -> Result<Vec<FoldId>, GraphError>;
    pub fn find_blocked(&self, scope: Option<FoldId>) -> Vec<(FoldId, Vec<FoldId>)>;
    pub fn detect_cycles(&self, kind: &EdgeKind) -> Vec<Vec<FoldId>>;
    pub fn topo_sort(&self, kind: &EdgeKind, scope: Option<FoldId>) -> Result<Vec<Vec<FoldId>>, GraphError>;

    // --- Search (linear scan, FTS is backend-level) ---
    pub fn filter(&self, filters: &Filters) -> Vec<FoldId>;
}

pub enum Direction { Outgoing, Incoming, Both }

pub struct Filters {
    pub status: Option<String>,
    pub label: Option<String>,
    pub assignee: Option<String>,
    pub parent: Option<FoldId>,
}
```

### Memory characteristics

- `SlotMap<FoldId, Fold>`: contiguous Vec under the hood, O(1) access by key
- `SecondaryMap<FoldId, _>`: same layout, parallel to the slotmap
- `SmallVec<[FoldId; 8]>`: inline storage for ≤8 children (no heap alloc)
- `SmallVec<[Edge; 4]>`: inline for ≤4 edges per direction
- Comments/history/refs: only allocated for folds that have them (HashMap, not SecondaryMap)

For 10,000 folds: ~2MB for the graph structure itself (folds + tree + edges).
Comments and history dominate at scale but are append-only.

## Backend Trait (`src/backend.rs`)

```rust
#[async_trait]
pub trait TrakBackend: Send + Sync {
    /// Load the entire graph from storage.
    async fn load(&self) -> Result<FoldGraph, BackendError>;

    /// Persist the entire graph (full snapshot).
    async fn save(&self, graph: &FoldGraph) -> Result<(), BackendError>;

    /// Persist incremental changes (WAL-style).
    async fn apply_op(&self, op: &GraphOp) -> Result<(), BackendError>;

    /// Full-text search (backend-level optimization, e.g. FTS5).
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Uuid>, BackendError>;
}

/// Incremental operation for write-ahead persistence.
pub enum GraphOp {
    InsertFold { fold: Fold, parent: Option<Uuid> },
    UpdateFold { fold: Fold },
    DeleteFold { uuid: Uuid, cascade: bool },
    Reparent { uuid: Uuid, new_parent: Option<Uuid> },
    AddEdge { source: Uuid, target: Uuid, kind: EdgeKind },
    RemoveEdge { source: Uuid, target: Uuid, kind: Option<EdgeKind> },
    AddComment { fold_uuid: Uuid, comment: Comment },
    AppendHistory { fold_uuid: Uuid, entry: HistoryEntry },
    AddRef { fold_uuid: Uuid, r: ExternalRef },
    RemoveRef { fold_uuid: Uuid, url: String },
}
```

### Two persistence modes

1. **`apply_op`** (default): each mutation is persisted immediately as an
   incremental write. Low latency, WAL-friendly.

2. **`save`** (periodic): full snapshot for backup/recovery. Called on shutdown
   or on a timer.

On startup: `load()` reconstructs the full FoldGraph from the backend.

## SQLite Backend (`src/backends/sqlite.rs`)

First implementation. Schema matches the ops:

```sql
CREATE TABLE folds (...);        -- one row per fold
CREATE TABLE edges (...);        -- typed links
CREATE TABLE comments (...);
CREATE TABLE history (...);
CREATE TABLE external_refs (...);
CREATE VIRTUAL TABLE folds_fts USING fts5(title, body, content=folds, content_rowid=rowid);
```

`load()` reads all tables into a FoldGraph.
`apply_op()` writes individual INSERT/UPDATE/DELETE.
`search()` uses the FTS5 virtual table.

## Dependencies

```toml
[dependencies]
slotmap = "1"
smallvec = { version = "1", features = ["serde"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
plexus-core = "0.5"
plexus-macros = "0.5"
```

## Tests

### Graph tests (no backend)

#### `test_insert_and_get`
Insert fold. Get by FoldId. Assert fields match.

#### `test_parent_child`
Insert parent, insert child under parent. Assert children_of(parent) = [child].
Assert parent_of(child) = parent. Assert roots = [parent].

#### `test_deep_recursion`
Insert 100-deep chain. Assert depth(leaf) = 99. Assert ancestors(leaf).len() = 99.

#### `test_remove_cascade`
Insert parent → child → grandchild. Remove parent cascade=true. Assert all gone.

#### `test_remove_no_cascade_fails`
Insert parent + child. Remove parent cascade=false. Assert error.

#### `test_reparent`
Insert A, B, C under A. Reparent C to root. Assert parent_of(C) = None.
Assert children_of(A) = [B]. Assert roots contains C.

#### `test_add_edge`
Insert A, B. Add edge A→B DependsOn. Assert edges_from(A) contains B.
Assert edges_to(B) contains A.

#### `test_cycle_detection`
A→B→C→A DependsOn. detect_cycles. Assert cycle found.

#### `test_topo_sort`
A→B→C linear. topo_sort. Assert [[C], [B], [A]].

#### `test_critical_path`
Diamond: A→B, A→C, B→D, C→D. critical_path(A). Assert longest chain.

#### `test_find_blocked`
A depends_on B (open). B depends_on C (done). find_blocked. Assert A blocked by B.

#### `test_filter_by_status`
Insert 5 folds: 3 open, 2 done. filter(status="open"). Assert 3 results.

### Backend tests (SQLite)

#### `test_save_and_load`
Build graph with 10 folds, edges, comments. Save. Load into new graph. Assert identical.

#### `test_apply_op_insert`
apply_op InsertFold. Load. Assert fold present.

#### `test_apply_op_delete`
Insert, apply_op DeleteFold. Load. Assert fold gone.

#### `test_fts_search`
Insert folds with varied titles. search("deploy"). Assert matching UUIDs.
