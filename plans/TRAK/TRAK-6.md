# TRAK-6: Comments + History

blocked_by: [TRAK-3]
unlocks: [TRAK-8]

## Scope

Comment threads on nodes and linear mutation history.

## Method

### Hub methods

```rust
async fn comment(
    &self,
    id: String,             // node UUID
    body: String,
    author: Option<String>,
) -> impl Stream<Item = TrakEvent>

async fn comments(
    &self,
    id: String,
    limit: Option<u32>,     // default: all
) -> impl Stream<Item = TrakEvent>

async fn history(
    &self,
    id: String,
    limit: Option<u32>,     // default: 50
    action: Option<String>, // filter by action type
) -> impl Stream<Item = TrakEvent>
```

### Events

```rust
CommentAdded { node_id, comment: Comment }
CommentList { node_id, comments: Vec<Comment>, total: u32 }
HistoryList { node_id, entries: Vec<HistoryEntry>, total: u32 }
```

### History actions (closed vocabulary)

| Action | Diff contents |
|--------|--------------|
| `created` | `{ title, status, parent? }` |
| `updated` | `{ field: { old, new } }` for each changed field |
| `deleted` | `{ cascade: bool, children_deleted: u32 }` |
| `moved` | `{ old_parent, new_parent }` |
| `linked` | `{ target, kind }` |
| `unlinked` | `{ target, kind }` |
| `commented` | `{ comment_id }` |
| `ref_added` | `{ kind, url }` |
| `ref_removed` | `{ url }` |
| `status_changed` | `{ old, new }` — also in `updated` but called out for filtering |

### Auto-history

TRAK-3's CRUD methods already call `append_history`. This ticket adds:
- History for link/unlink (from TRAK-5)
- History for comment (this ticket)
- History for ref add/remove (from TRAK-7)

Each hub method that mutates state calls `storage.append_history()`
with the appropriate action and diff.

## Tests

### `test_comment_added`
Create node, add comment. Assert CommentAdded event with body.

### `test_comments_ordered`
Add 3 comments. Get comments. Assert ordered by created_at ascending.

### `test_history_tracks_create`
Create node. Get history. Assert one entry with action="created".

### `test_history_tracks_update`
Create, update status "open"→"done". History. Assert "updated" entry
with diff showing old/new.

### `test_history_tracks_move`
Create, move. History. Assert "moved" entry with old/new parent.

### `test_history_filter_by_action`
Create, update, comment, link. Get history action="commented". Assert 1.

### `test_history_limit`
Generate 10 entries. Get with limit=3. Assert 3 returned, newest first.
