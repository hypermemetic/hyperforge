# TRAK-8: Context Export for Adapters

blocked_by: [TRAK-5, TRAK-6, TRAK-7]
unlocks: []

## Scope

The `context` method exports everything an external adapter needs to
understand and act on a node — its full subtree, dependency graph,
comments, history, and external refs. This is the data contract for
container builders, CI integrators, and rendering engines.

## Method

### Hub method

```rust
async fn context(
    &self,
    id: String,
    depth: Option<u32>,         // subtree depth (default: unlimited)
    include_history: Option<bool>,  // default: true
    include_comments: Option<bool>, // default: true
    include_graph: Option<bool>,    // default: true
) -> impl Stream<Item = TrakEvent>
```

### Event

```rust
NodeContext {
    node: Node,
    children: Vec<NodeContext>,     // recursive
    links: Vec<LinkDetail>,
    refs: Vec<ExternalRef>,
    comments: Vec<Comment>,
    history: Vec<HistoryEntry>,
    ancestors: Vec<NodeSummary>,    // path from root to this node
    stats: ContextStats,
}

ContextStats {
    total_nodes: u32,               // in subtree
    total_links: u32,
    total_comments: u32,
    depth: u32,                     // max depth of subtree
    statuses: Map<String, u32>,     // count by status
    blocked_count: u32,
}
```

### Use cases this serves

**Container builder adapter**: reads `context` for a node, extracts:
- `refs` of kind "container" for base image
- `node.metadata` for env vars, build args
- `children` for subtask decomposition
- `links` of kind "depends_on" for prerequisite containers

**Rendering adapter**: reads `context`, produces:
- Tree view (from `children` recursion)
- Kanban board (from `node.status` grouping)
- Gantt chart (from `links` dependency graph)
- Timeline (from `history`)

**Agent adapter**: reads `context`, produces:
- Prompt with full task description (node.body)
- Subtask list (children)
- Blockers (blocked_by links to incomplete nodes)
- Reference material (external refs)

### Ancestor chain

`ancestors` is the path from root to the target node, ordered root-first.
This gives context about where this node sits in the hierarchy without
requiring the caller to walk up the tree.

## Tests

### `test_context_simple`
Create a node with 2 children, 1 link, 2 comments, 1 ref.
Get context. Assert all present.

### `test_context_recursive`
3-level tree. Get context of root. Assert children are nested recursively.

### `test_context_ancestors`
Create A → B → C. Get context of C. Assert ancestors = [A, B].

### `test_context_stats`
Create tree with mixed statuses. Assert stats.statuses counts correct.

### `test_context_depth_limit`
3-level tree. Context depth=1. Assert only direct children, not grandchildren.

### `test_context_exclude_history`
Context with include_history=false. Assert history empty.

### `test_context_blocked_count`
Node with 2 depends_on links to incomplete nodes. Assert blocked_count=2.
