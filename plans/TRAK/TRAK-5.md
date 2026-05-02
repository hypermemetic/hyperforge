# TRAK-5: Links + Graph Queries

blocked_by: [TRAK-3]
unlocks: [TRAK-8]

## Scope

Typed links between nodes and graph traversal queries. This is the
dependency/relationship layer — separate from the containment tree.

## Method

### Hub methods

```rust
async fn link(
    &self,
    from: String,           // source node UUID
    to: String,             // target node UUID
    kind: String,           // "depends_on", "blocks", "relates_to", "duplicates", ...
    created_by: Option<String>,
) -> impl Stream<Item = TrakEvent>

async fn unlink(
    &self,
    from: String,
    to: String,
    kind: Option<String>,   // None = remove all links between from→to
) -> impl Stream<Item = TrakEvent>

async fn links(
    &self,
    id: String,
    kind: Option<String>,
    direction: Option<String>,  // "outgoing", "incoming", "both" (default: "both")
) -> impl Stream<Item = TrakEvent>

async fn graph(
    &self,
    id: String,
    kind: Option<String>,       // filter by link kind (default: all)
    depth: Option<u32>,         // traversal depth (default: unlimited)
    direction: Option<String>,  // "outgoing", "incoming", "both"
) -> impl Stream<Item = TrakEvent>

async fn critical_path(
    &self,
    id: String,                 // target node
) -> impl Stream<Item = TrakEvent>

async fn blocked(
    &self,
    parent: Option<String>,     // scope to subtree
    status: Option<String>,     // filter
) -> impl Stream<Item = TrakEvent>
```

### Events

```rust
LinkCreated { from, to, kind }
LinkRemoved { from, to, kind, count: u32 }
LinkDetail { source: NodeSummary, target: NodeSummary, kind }
GraphNode { node: NodeSummary, depth: u32, links: Vec<LinkDetail> }
CriticalPathStep { node: NodeSummary, depth: u32, blocking_count: u32 }
BlockedNode { node: NodeSummary, blocked_by: Vec<NodeSummary> }
```

### Graph queries (`queries.rs`)

```rust
// BFS/DFS from a node following typed links
async fn traverse_graph(&self, start: &Uuid, kind: Option<&str>,
    direction: &str, depth: Option<u32>) -> Result<Vec<(Node, u32, Vec<Link>)>>

// Longest path to completion (all deps resolved)
async fn critical_path(&self, target: &Uuid) -> Result<Vec<(Node, u32)>>

// All nodes with unresolved "depends_on" links where target is not "done"
async fn find_blocked(&self, parent: Option<&Uuid>) -> Result<Vec<(Node, Vec<Node>)>>

// Cycle detection in link graph
async fn detect_cycles(&self, kind: &str) -> Result<Vec<Vec<Uuid>>>
```

### Critical path algorithm

For `depends_on` links: find the longest chain of unresolved dependencies
leading to the target node. This is the minimum time to completion assuming
parallelism on independent branches.

1. Build dependency DAG from target node (reverse `depends_on` links)
2. Topological sort
3. Longest path via DP on the topo order

## Tests

### `test_link_and_list`
Create A and B. Link A→B "depends_on". List links from A. Assert B found.

### `test_unlink`
Link A→B. Unlink. List links. Assert empty.

### `test_graph_traversal`
A→B→C→D chain (depends_on). Graph from A depth=None. Assert 4 nodes at depths 0-3.

### `test_graph_depth_limit`
Same chain. Graph from A depth=1. Assert only A and B.

### `test_critical_path`
Diamond: A→B, A→C, B→D, C→D. D depends_on B and C. Critical path to A.
Assert longest chain reported.

### `test_blocked_nodes`
A depends_on B (status=open). B depends_on C (status=done).
Blocked query. Assert A is blocked (by B). B is not blocked (C is done).

### `test_cycle_detection`
A→B→C→A. detect_cycles. Assert cycle found.

### `test_bidirectional_links`
A blocks B. Links from A direction=outgoing → B. Links from B direction=incoming → A.
