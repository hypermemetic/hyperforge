# TRAK-2: Types + SQLite Storage

blocked_by: []
unlocks: [TRAK-3, TRAK-4]

## Scope

Define the core types (Node, Link, Comment, HistoryEntry, ExternalRef) and
implement SQLite-backed persistence. This is the foundation everything else
builds on.

## Method

### Crate scaffold

Create `plexus-trak/` as a standalone Rust crate with:
- `plexus-core` dependency (for Activation trait)
- `plexus-macros` dependency (for `#[activation]`)
- `sqlx` with SQLite (same pattern as plexus-substrate activations)
- `serde`, `serde_json`, `uuid`, `chrono`

### Types (`src/types.rs`)

All types derive `Serialize, Deserialize, Clone, Debug`.

```rust
struct Node { id, parent, title, body, status, labels, assignee, priority, created_at, updated_at, metadata }
struct Link { id, source, target, kind, created_at, created_by }
struct ExternalRef { id, node_id, kind, url, provider, title, metadata }
struct Comment { id, node_id, author, body, created_at, updated_at }
struct HistoryEntry { id, node_id, timestamp, actor, action, diff }
```

### Storage (`src/storage.rs`)

SQLite tables:
```sql
CREATE TABLE nodes (
    id TEXT PRIMARY KEY,
    parent TEXT REFERENCES nodes(id),
    title TEXT NOT NULL,
    body TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    labels TEXT NOT NULL DEFAULT '[]',   -- JSON array
    assignee TEXT,
    priority INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}'  -- JSON object
);

CREATE TABLE links (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL REFERENCES nodes(id),
    target TEXT NOT NULL REFERENCES nodes(id),
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    created_by TEXT
);

CREATE TABLE external_refs (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    kind TEXT NOT NULL,
    url TEXT NOT NULL,
    provider TEXT,
    title TEXT,
    metadata TEXT
);

CREATE TABLE comments (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    author TEXT,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE history (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    timestamp TEXT NOT NULL,
    actor TEXT,
    action TEXT NOT NULL,
    diff TEXT NOT NULL DEFAULT '{}'  -- JSON
);

CREATE INDEX idx_nodes_parent ON nodes(parent);
CREATE INDEX idx_nodes_status ON nodes(status);
CREATE INDEX idx_links_source ON links(source);
CREATE INDEX idx_links_target ON links(target);
CREATE INDEX idx_comments_node ON comments(node_id);
CREATE INDEX idx_history_node ON history(node_id);
CREATE INDEX idx_refs_node ON external_refs(node_id);
```

### Storage API

```rust
impl TrakStorage {
    async fn new(db_path: &str) -> Result<Self>
    async fn migrate(&self) -> Result<()>

    // Nodes
    async fn create_node(&self, node: &Node) -> Result<()>
    async fn get_node(&self, id: &Uuid) -> Result<Option<Node>>
    async fn update_node(&self, node: &Node) -> Result<()>
    async fn delete_node(&self, id: &Uuid) -> Result<()>
    async fn list_children(&self, parent: Option<&Uuid>, limit: Option<u32>) -> Result<Vec<Node>>
    async fn count_children(&self, parent: Option<&Uuid>) -> Result<u32>

    // Links
    async fn add_link(&self, link: &Link) -> Result<()>
    async fn remove_link(&self, source: &Uuid, target: &Uuid, kind: Option<&str>) -> Result<u32>
    async fn get_links(&self, node: &Uuid, direction: &str, kind: Option<&str>) -> Result<Vec<Link>>

    // Comments
    async fn add_comment(&self, comment: &Comment) -> Result<()>
    async fn get_comments(&self, node_id: &Uuid) -> Result<Vec<Comment>>

    // History
    async fn append_history(&self, entry: &HistoryEntry) -> Result<()>
    async fn get_history(&self, node_id: &Uuid, limit: Option<u32>) -> Result<Vec<HistoryEntry>>

    // Refs
    async fn add_ref(&self, r: &ExternalRef) -> Result<()>
    async fn remove_ref(&self, node_id: &Uuid, url: &str) -> Result<u32>
    async fn get_refs(&self, node_id: &Uuid) -> Result<Vec<ExternalRef>>
}
```

## Tests

### `test_create_and_get_node`
Create a node, get by ID. Assert all fields match.

### `test_parent_child`
Create parent, create child with parent set. List children of parent. Assert child returned.

### `test_update_node`
Create, update title+status, get. Assert updated fields.

### `test_delete_node`
Create, delete, get. Assert None.

### `test_delete_cascade`
Create parent with 2 children. Delete parent with cascade. Assert all gone.

### `test_links`
Create two nodes, add link. Get links from source. Assert target found.
Remove link. Assert empty.

### `test_comments`
Add 3 comments to a node. Get comments. Assert ordered by created_at.

### `test_history`
Append 5 history entries. Get with limit 3. Assert 3 returned, newest first.

### `test_refs`
Add external ref (PR). Get refs. Assert found. Remove by URL. Assert gone.
