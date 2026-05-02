# TRAK-3: CRUD Hub Methods

blocked_by: [TRAK-2]
unlocks: [TRAK-5, TRAK-6, TRAK-7]

## Scope

Implement the TrakHub Plexus activation with core CRUD methods. This is
the wire surface for creating, reading, updating, deleting, and moving nodes.

## Method

### Hub (`src/hub.rs`)

```rust
#[plexus_macros::activation(
    namespace = "trak",
    description = "Recursive work tracker",
    crate_path = "plexus_core"
)]
impl TrakHub {
    // -- CRUD --

    async fn create(
        &self,
        title: String,
        parent: Option<String>,     // UUID of parent node
        status: Option<String>,     // default: "open"
        body: Option<String>,
        labels: Option<Vec<String>>,
        assignee: Option<String>,
        priority: Option<i32>,
        metadata: Option<Value>,
    ) -> impl Stream<Item = TrakEvent>

    async fn get(&self, id: String) -> impl Stream<Item = TrakEvent>

    async fn update(
        &self,
        id: String,
        title: Option<String>,
        status: Option<String>,
        body: Option<String>,
        labels: Option<Vec<String>>,
        assignee: Option<String>,
        priority: Option<i32>,
        metadata: Option<Value>,
    ) -> impl Stream<Item = TrakEvent>

    async fn delete(
        &self,
        id: String,
        cascade: Option<bool>,      // default: false (fail if has children)
    ) -> impl Stream<Item = TrakEvent>

    async fn move_node(
        &self,
        id: String,
        new_parent: Option<String>, // None = move to root
    ) -> impl Stream<Item = TrakEvent>
}
```

### Events (`src/events.rs`)

```rust
enum TrakEvent {
    NodeCreated { node: Node },
    NodeUpdated { node: Node, changes: Vec<String> },
    NodeDeleted { id: String, children_deleted: u32 },
    NodeMoved { id: String, old_parent: Option<String>, new_parent: Option<String> },
    NodeDetail { node: Node, child_count: u32, link_count: u32, comment_count: u32 },
    Error { code: String, message: String },
    Info { message: String },
}
```

### History integration

Every mutation (create, update, delete, move) appends a HistoryEntry
automatically. The hub method handles this — callers don't need to
think about it.

### Binary entry point

`src/main.rs` — standalone daemon:
```rust
#[tokio::main]
async fn main() {
    let hub = TrakHub::new("~/.config/trak/trak.db").await;
    let dynamic = DynamicHub::new("trak").register(hub);
    TransportServer::builder(dynamic, rpc_converter)
        .with_websocket(port)
        .build().await.serve().await;
}
```

Also supports registration on lforge dynamic hub via the plexus
activation trait (same as any other activation).

## Tests

### `test_create_returns_node`
Create with title. Assert NodeCreated event with matching title, status "open", non-empty ID.

### `test_get_returns_detail`
Create, then get. Assert NodeDetail with child_count=0.

### `test_update_changes_fields`
Create, update title+status. Assert NodeUpdated with changes=["title","status"].

### `test_delete_no_children`
Create, delete. Assert NodeDeleted with children_deleted=0.

### `test_delete_cascade`
Create parent + 2 children. Delete parent cascade=true. Assert children_deleted=2.

### `test_delete_fails_with_children`
Create parent + child. Delete parent cascade=false. Assert Error.

### `test_move_to_root`
Create parent + child. Move child to root. Assert old_parent=Some, new_parent=None.

### `test_create_with_parent`
Create parent, create child under it. Get parent. Assert child_count=1.
