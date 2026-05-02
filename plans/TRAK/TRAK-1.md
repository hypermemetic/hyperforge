# TRAK-1: Epic Overview — Recursive Work Tracker

blocked_by: []
unlocks: [TRAK-2, TRAK-3, TRAK-4, TRAK-5, TRAK-6, TRAK-7, TRAK-8]

## Problem

Fixed hierarchies (epic → feature → ticket) assume bounded scope. With
agentic development at scale, we don't know a priori whether a unit of
work is a ten-minute fix or a six-month initiative. The structure must
be fractal — every node is the same kind of thing, composable to
arbitrary depth.

## Core Abstraction: Node

Everything is a **Node**. A node can contain other nodes. A node can
link to other nodes. A node has a linear history. That's it.

```
Node {
    id:          UUID
    parent:      Option<UUID>           // containment (tree)
    title:       String
    body:        Option<String>         // markdown
    status:      String                 // user-defined, e.g. "open", "in_progress", "done"
    labels:      Vec<String>            // arbitrary tags
    assignee:    Option<String>
    priority:    Option<i32>            // numeric, lower = higher priority
    created_at:  DateTime
    updated_at:  DateTime
    
    // — relationships —
    links:       Vec<Link>              // typed edges to other nodes
    refs:        Vec<ExternalRef>       // PR URLs, commit SHAs, forge links
    
    // — conversation —
    comments:    Vec<Comment>           // linear thread per node
    
    // — audit —
    history:     Vec<HistoryEntry>      // linear log of all mutations
    
    // — extensible —
    metadata:    Map<String, Value>     // arbitrary KV for adapters
}

Link {
    target:      UUID
    kind:        String                 // "depends_on", "blocks", "relates_to", "duplicates", ...
    created_at:  DateTime
    created_by:  Option<String>
}

ExternalRef {
    kind:        String                 // "pr", "commit", "url", "forge_issue"
    url:         String
    provider:    Option<String>         // "github", "codeberg", "linear", ...
    title:       Option<String>
    metadata:    Option<Value>
}

Comment {
    id:          UUID
    author:      Option<String>
    body:        String
    created_at:  DateTime
    updated_at:  Option<DateTime>
}

HistoryEntry {
    id:          UUID
    timestamp:   DateTime
    actor:       Option<String>
    action:      String                 // "created", "status_changed", "linked", "commented", ...
    diff:        Value                  // what changed, structured
}
```

## Design Principles

1. **Recursive containment**: parent-child is a tree. Depth is unbounded.
   A "project" is just a node with children. An "epic" is just a node
   under a project. A "task" is just a node under an epic. The system
   doesn't distinguish — humans and views do.

2. **Typed links as metadata**: dependency graphs, blocking relationships,
   and cross-references are all the same mechanism — typed edges between
   nodes. The system stores them; interpreters (views, adapters) give
   them meaning.

3. **Linear history**: every mutation to a node appends to its history.
   No branching, no rewriting. You can always reconstruct the node's
   state at any point in time.

4. **Views are projections**: the system stores the graph. "Show me all
   blocked nodes", "show me the dependency tree for this release",
   "what's the critical path" — these are queries over the graph, not
   separate data structures.

5. **Adapters are external**: rendering (TUI, web), container generation,
   CI integration, forge issue sync — all external. The system exposes
   enough structure for any adapter to extract what it needs.

6. **Status is a string, not an enum**: different workflows need different
   statuses. A kanban board might use "backlog/todo/doing/done". A
   release tracker might use "planned/in_progress/testing/shipped".
   The system doesn't enforce a state machine.

## Crate Structure

Standalone crate: `plexus-trak/`

```
plexus-trak/
  src/
    lib.rs              // re-exports
    hub.rs              // TrakHub — Plexus activation
    types.rs            // Node, Link, Comment, HistoryEntry, ExternalRef
    storage.rs          // SQLite-backed persistence
    queries.rs          // graph traversal, search, filtering
    events.rs           // TrakEvent enum for RPC streaming
  Cargo.toml
```

Registers on the lforge dynamic hub as namespace `trak`.

## Activation Surface

```
trak.create         --title --parent? --status? --labels? --body? --assignee? --priority? --metadata?
trak.get            --id
trak.update         --id --title? --status? --body? --labels? --assignee? --priority? --metadata?
trak.delete         --id --cascade?
trak.move           --id --new_parent?
trak.list           --parent? --status? --label? --assignee? --depth? --limit?
trak.search         --query --status? --label?

trak.link           --from --to --kind
trak.unlink         --from --to --kind?
trak.links          --id --kind? --direction?

trak.comment        --id --body --author?
trak.comments       --id

trak.history        --id --limit?

trak.ref_add        --id --kind --url --provider? --title?
trak.ref_remove     --id --url
trak.refs           --id

trak.tree           --id? --depth? --status?
trak.graph          --id --kind? --depth?          // dependency graph from a node
trak.critical_path  --id                           // longest dep chain to completion
trak.blocked        --status?                      // all nodes blocked by unresolved deps
trak.context        --id --depth?                  // full context dump for container/agent use
```

## Dependency DAG

```
TRAK-2 (types + storage)
  ├── TRAK-3 (CRUD hub methods)
  │     ├── TRAK-5 (links + graph queries)
  │     ├── TRAK-6 (comments + history)
  │     └── TRAK-7 (external refs)
  ├── TRAK-4 (search + list + tree views)
  └── TRAK-8 (context export for adapters)
        needs: TRAK-5, TRAK-6, TRAK-7
```

Parallelizable: TRAK-3 and TRAK-4 after TRAK-2.
TRAK-5, TRAK-6, TRAK-7 after TRAK-3.
TRAK-8 is terminal.

## Non-Goals (v1)

- State machine enforcement (status transitions)
- Permissions / multi-tenancy
- Real-time collaboration / websocket push
- Forge issue sync (GitHub Issues ↔ trak) — future adapter
- Rendering / TUI / web UI — external
- Container generation — external adapter, trak.context provides the data
