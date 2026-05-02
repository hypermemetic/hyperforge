# TRAK-7: External References

blocked_by: [TRAK-3]
unlocks: [TRAK-8]

## Scope

Link nodes to external systems — PRs, commits, forge issues, URLs,
documentation, container specs. This is the bridge between trak and
the outside world.

## Method

### Hub methods

```rust
async fn ref_add(
    &self,
    id: String,             // node UUID
    kind: String,           // "pr", "commit", "url", "forge_issue", "container", "doc"
    url: String,
    provider: Option<String>,   // "github", "codeberg", "linear", ...
    title: Option<String>,
    metadata: Option<Value>,    // arbitrary structured data
) -> impl Stream<Item = TrakEvent>

async fn ref_remove(
    &self,
    id: String,
    url: String,
) -> impl Stream<Item = TrakEvent>

async fn refs(
    &self,
    id: String,
    kind: Option<String>,       // filter by kind
) -> impl Stream<Item = TrakEvent>
```

### Events

```rust
RefAdded { node_id, ref: ExternalRef }
RefRemoved { node_id, url }
RefList { node_id, refs: Vec<ExternalRef>, total: u32 }
```

### Ref kinds (open vocabulary, but common patterns)

| Kind | Example URL | Metadata |
|------|-------------|----------|
| `pr` | `https://github.com/hypermemetic/hyperforge/pull/42` | `{ state: "open", branch: "feat/x" }` |
| `commit` | `https://github.com/hypermemetic/hyperforge/commit/abc123` | `{ sha: "abc123", message: "..." }` |
| `forge_issue` | `https://github.com/hypermemetic/hyperforge/issues/10` | `{ state: "open" }` |
| `url` | any URL | `{ description: "..." }` |
| `doc` | path or URL to documentation | `{}` |
| `container` | `ghcr.io/hypermemetic/builder:latest` | `{ dockerfile: "...", env: {...} }` |

### Auto-extraction (future)

When a PR URL is added, an adapter could auto-populate metadata
(title, state, branch) by querying the forge API. This is NOT in v1 —
refs are user-supplied or set by automation.

## Tests

### `test_ref_add_pr`
Create node, add PR ref. Assert RefAdded with correct kind and URL.

### `test_ref_remove`
Add ref, remove by URL. Assert RefRemoved. Get refs. Assert empty.

### `test_refs_filter_by_kind`
Add 3 refs (pr, commit, url). Get refs kind="pr". Assert 1 returned.

### `test_ref_with_metadata`
Add ref with metadata JSON. Get refs. Assert metadata preserved.

### `test_ref_history`
Add ref. Check history. Assert "ref_added" entry with url in diff.
