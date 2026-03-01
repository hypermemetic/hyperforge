# CONF-4: Extract WorkspaceRunner Concurrency Abstraction

**blocked_by**: [CONF-2]
**unlocks**: [CONF-6]

## Scope

Extract the duplicated parallel execution patterns from `workspace.rs` into a reusable `WorkspaceRunner` that handles chunking, JoinSet management, spawn_blocking vs async dispatch, result collection, and event yielding.

## Current Duplication

6+ methods reimplement the same chunked-parallel-JoinSet pattern:

| Method | Lines | Concurrency | Blocking? | Chunk Size |
|--------|-------|-------------|-----------|------------|
| check | 424-494 | Parallel | spawn_blocking (git) | 8 |
| push_all | 612-685 | Parallel | spawn_blocking (git) | 8 |
| sync phase 6 (diff) | ~1235-1303 | Parallel | async (forge API) | unbounded |
| sync phase 7 (apply) | ~1305-1446 | Parallel | async (forge API) | unbounded |
| sync phase 8 (push) | 1747-1809 | Parallel | spawn_blocking (git) | 8 |
| set_default_branch | ~1850-2021 | Parallel | async (forge API) | 8 |
| exec | 2780-2850 | Parallel | async (Command) | unbounded |
| clone | 2950-3060 | Parallel | async (git) | configurable |

Each copy-pastes:
```rust
for chunk in items.chunks(N) {
    let mut join_set = JoinSet::new();
    for item in chunk {
        join_set.spawn(/* blocking or async */);
    }
    while let Some(result) = join_set.join_next().await {
        // extract, yield event, update counter
    }
}
```

## Design

### Core Abstraction

```rust
/// How to dispatch per-item work
pub enum Dispatch {
    /// Async tasks (forge API, async git, shell commands)
    Async(usize),        // max concurrency (0 = unbounded)
    /// spawn_blocking tasks (synchronous git CLI)
    Blocking(usize),     // max concurrency
    /// No parallelism
    Sequential,
}

/// Run a batch operation across items with controlled concurrency.
///
/// Returns results in completion order. Caller yields events from results.
pub async fn run_batch<T, R, F, Fut>(
    items: Vec<T>,
    dispatch: Dispatch,
    op: F,
) -> Vec<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = R> + Send + 'static,
```

For `Dispatch::Blocking`, the function wraps `op` in `spawn_blocking`. For `Dispatch::Async`, it spawns directly. For `Sequential`, it awaits inline.

Chunking is handled internally based on the concurrency limit.

### Location

New file: `src/commands/runner.rs`

### Blocking Variant

For git operations that use synchronous `Git::` methods, we need a blocking-compatible signature:

```rust
pub async fn run_batch_blocking<T, R, F>(
    items: Vec<T>,
    concurrency: usize,
    op: F,
) -> Vec<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + Clone + 'static,
```

This calls `tokio::task::spawn_blocking` internally.

### Usage Example — `check` method (before/after)

**Before** (~70 lines):
```rust
for chunk in git_repos.chunks(8) {
    let mut join_set = JoinSet::new();
    for repo in chunk {
        let path = repo.path.clone();
        let name = repo.dir_name.clone();
        let expected = expected_branch.clone();
        join_set.spawn(tokio::task::spawn_blocking(move || {
            let branch = Git::current_branch(&path).unwrap_or("?".into());
            let status = Git::repo_status(&path);
            let is_clean = status.map(|s| s.is_clean()).unwrap_or(false);
            (name, path, branch, expected, is_clean)
        }));
    }
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok((name, path, branch, expected, clean))) => {
                // ... yield RepoCheck, update counters ...
            }
            _ => { /* error handling */ }
        }
    }
}
```

**After** (~20 lines):
```rust
let results = run_batch_blocking(git_repos, 8, |repo| {
    let branch = Git::current_branch(&repo.path).unwrap_or("?".into());
    let status = Git::repo_status(&repo.path);
    let is_clean = status.map(|s| s.is_clean()).unwrap_or(false);
    CheckResult { name: repo.dir_name, path: repo.path, branch, is_clean }
}).await;

for r in results {
    yield HyperforgeEvent::RepoCheck { /* from r */ };
    // update counters
}
```

### Usage Example — `diff` method (async forge API)

**Before** (~40 lines of JoinSet boilerplate):
```rust
let mut join_set = JoinSet::new();
for (org, forge_str) in &pairs {
    let adapter = make_adapter(...)?;
    let local = state.get_local_forge(org).await;
    let sync = state.sync_service.clone();
    join_set.spawn(async move {
        sync.diff(&local, &adapter, org).await
    });
}
while let Some(result) = join_set.join_next().await { ... }
```

**After** (~15 lines):
```rust
let tasks: Vec<_> = pairs.iter().map(|(org, forge)| {
    DiffTask { org, forge, adapter: make_adapter(...)?, local: ... }
}).collect();

let results = run_batch(tasks, Dispatch::Async(0), |task| async move {
    sync.diff(&task.local, &task.adapter, &task.org).await
}).await;
```

## Additional Helpers

### Discovery Helper

```rust
/// Discover workspace or yield error event. Used by 11+ methods.
pub fn discover_or_bail(path: &Path) -> Result<WorkspaceContext, HyperforgeEvent> {
    discover_workspace(path).map_err(|e| HyperforgeEvent::Error {
        message: format!("Discovery failed: {}", e),
    })
}
```

Usage in stream:
```rust
let ctx = match discover_or_bail(&workspace_path) {
    Ok(ctx) => ctx,
    Err(event) => { yield event; return; }
};
```

### Summary Builder

```rust
pub struct SummaryBuilder {
    total: usize,
    success: usize,
    failed: usize,
}

impl SummaryBuilder {
    pub fn new(total: usize) -> Self { ... }
    pub fn record_success(&mut self) { self.success += 1; }
    pub fn record_failure(&mut self) { self.failed += 1; }
    pub fn into_workspace_summary(self) -> HyperforgeEvent { ... }
}
```

## Acceptance Criteria

- [ ] `run_batch` handles async dispatch with configurable concurrency
- [ ] `run_batch_blocking` handles spawn_blocking with chunking
- [ ] `Sequential` dispatch awaits inline without spawning
- [ ] `discover_or_bail` replaces 11+ discovery-error blocks
- [ ] At least one workspace method converted to use `run_batch` as proof-of-concept
- [ ] No behavior change — same events yielded in same order
- [ ] `cargo build --release` succeeds
- [ ] Existing tests pass

## Notes

- Result ordering: `run_batch` returns results in completion order (not input order). This matches current behavior where JoinSet yields in completion order.
- The `stream!` macro makes it hard to return iterators from helper functions (can't yield from inside a non-stream closure). The runner returns a collected `Vec` that the caller iterates and yields from.
- Don't over-abstract: the goal is eliminating boilerplate, not building a framework. Keep the API surface small.
