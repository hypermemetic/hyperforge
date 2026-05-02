# TRAK-4: Search, List, Tree Views

blocked_by: [TRAK-2]
unlocks: [TRAK-8]

## Scope

Query methods for listing, searching, and tree-walking nodes. These are
read-only projections over the recursive graph.

## Method

### Hub methods

```rust
async fn list(
    &self,
    parent: Option<String>,         // None = root nodes
    status: Option<String>,
    label: Option<String>,
    assignee: Option<String>,
    depth: Option<u32>,             // 1 = direct children only (default)
    limit: Option<u32>,             // default: 100
    offset: Option<u32>,
) -> impl Stream<Item = TrakEvent>

async fn search(
    &self,
    query: String,                  // full-text search on title + body
    status: Option<String>,
    label: Option<String>,
    assignee: Option<String>,
    limit: Option<u32>,
) -> impl Stream<Item = TrakEvent>

async fn tree(
    &self,
    id: Option<String>,             // None = full tree from roots
    depth: Option<u32>,             // default: unlimited
    status: Option<String>,         // filter by status
    collapse_done: Option<bool>,    // hide completed subtrees
) -> impl Stream<Item = TrakEvent>
```

### Events

```rust
NodeSummary { id, title, status, labels, assignee, priority, child_count, depth }
TreeNode { id, title, status, depth, child_count, indent: String }  // for text rendering
SearchResult { node: NodeSummary, score: f32, snippet: String }
ListSummary { total: u32, returned: u32, offset: u32 }
```

### Storage additions (`queries.rs`)

```rust
async fn search_nodes(&self, query: &str, filters: &Filters, limit: u32) -> Result<Vec<(Node, f32)>>
async fn tree_walk(&self, root: Option<&Uuid>, depth: Option<u32>, filter: &Filters) -> Result<Vec<(Node, u32)>>
async fn count_descendants(&self, id: &Uuid) -> Result<u32>
```

Full-text search: SQLite FTS5 on title + body columns.

Tree walk: recursive CTE:
```sql
WITH RECURSIVE tree AS (
    SELECT id, parent, title, status, 0 as depth FROM nodes WHERE parent IS ?
    UNION ALL
    SELECT n.id, n.parent, n.title, n.status, t.depth + 1
    FROM nodes n JOIN tree t ON n.parent = t.id
    WHERE (? IS NULL OR t.depth < ?)
)
SELECT * FROM tree ORDER BY depth, title;
```

## Tests

### `test_list_root_nodes`
Create 3 root nodes. List with parent=None. Assert 3 returned.

### `test_list_children`
Create parent + 3 children. List with parent=parent_id. Assert 3.

### `test_list_filter_by_status`
Create 5 nodes: 3 open, 2 done. List status="open". Assert 3.

### `test_search_by_title`
Create nodes with various titles. Search "deploy". Assert matching nodes.

### `test_tree_full`
Create 3-level tree. Tree with depth=None. Assert all nodes with correct depths.

### `test_tree_depth_limit`
Same tree. Tree with depth=1. Assert only root + direct children.

### `test_tree_collapse_done`
Tree with some done subtrees. collapse_done=true. Assert done branches hidden.
