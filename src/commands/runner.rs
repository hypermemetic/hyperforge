//! WorkspaceRunner — reusable concurrency abstraction for workspace-level batch operations.
//!
//! Eliminates duplicated JoinSet/chunking boilerplate across workspace methods.
//! All functions return `Vec<(usize, R)>` where usize is the original index,
//! so callers can correlate results with inputs if needed.

use std::future::Future;
use tokio::task::JoinSet;

/// Split a Vec into chunks without requiring Clone.
fn chunk_vec<T>(items: Vec<T>, chunk_size: usize) -> Vec<Vec<T>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    for item in items {
        current.push(item);
        if current.len() == chunk_size {
            result.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Run a batch of blocking operations with bounded concurrency.
///
/// Each item is dispatched via `tokio::task::spawn_blocking`. Items are
/// processed in chunks of `concurrency` to bound resource usage. Results
/// are returned in completion order within each chunk.
///
/// Use this for git CLI operations and other synchronous work.
pub async fn run_batch_blocking<T, R, F>(
    items: Vec<T>,
    concurrency: usize,
    op: F,
) -> Vec<Result<R, String>>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + Clone + 'static,
{
    let len = items.len();
    let mut results = Vec::with_capacity(len);
    let chunk_size = if concurrency == 0 { len.max(1) } else { concurrency };

    let chunked = chunk_vec(items, chunk_size);

    for chunk in chunked {
        let mut join_set = JoinSet::new();

        for item in chunk {
            let op = op.clone();
            join_set.spawn(tokio::task::spawn_blocking(move || op(item)));
        }

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(value)) => results.push(Ok(value)),
                Ok(Err(e)) => results.push(Err(format!("spawn_blocking panic: {}", e))),
                Err(e) => results.push(Err(format!("JoinSet error: {}", e))),
            }
        }
    }

    results
}

/// Run a batch of async operations with bounded concurrency.
///
/// Items are processed in chunks of `concurrency`. Use `concurrency = 0`
/// for unbounded (all items spawned at once).
///
/// Use this for forge API calls and other async work.
pub async fn run_batch<T, R, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    op: F,
) -> Vec<Result<R, String>>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    let len = items.len();
    let mut results = Vec::with_capacity(len);
    let chunk_size = if concurrency == 0 { len.max(1) } else { concurrency };

    for chunk in chunk_vec(items, chunk_size) {
        let mut join_set = JoinSet::new();

        for item in chunk {
            let op = op.clone();
            join_set.spawn(async move { op(item).await });
        }

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(value) => results.push(Ok(value)),
                Err(e) => results.push(Err(format!("JoinSet error: {}", e))),
            }
        }
    }

    results
}

/// Discover workspace or return an error event.
///
/// Replaces the 11+ instances of:
/// ```ignore
/// let ctx = match discover_workspace(&path) {
///     Ok(ctx) => ctx,
///     Err(e) => { yield HyperforgeEvent::Error { ... }; return; }
/// };
/// ```
pub fn discover_or_bail(
    path: &std::path::Path,
) -> Result<crate::commands::workspace::WorkspaceContext, crate::hub::HyperforgeEvent> {
    crate::commands::workspace::discover_workspace(path).map_err(|e| {
        crate::hub::HyperforgeEvent::Error {
            message: format!("Discovery failed: {}", e),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_batch_blocking_basic() {
        let items: Vec<i32> = vec![1, 2, 3, 4, 5];
        let results = run_batch_blocking(items, 2, |x| x * 2).await;

        assert_eq!(results.len(), 5);
        let mut values: Vec<i32> = results.into_iter().map(|r| r.unwrap()).collect();
        values.sort();
        assert_eq!(values, vec![2, 4, 6, 8, 10]);
    }

    #[tokio::test]
    async fn test_run_batch_async_basic() {
        let items: Vec<i32> = vec![1, 2, 3, 4, 5];
        let results = run_batch(items, 2, |x| async move { x * 3 }).await;

        assert_eq!(results.len(), 5);
        let mut values: Vec<i32> = results.into_iter().map(|r| r.unwrap()).collect();
        values.sort();
        assert_eq!(values, vec![3, 6, 9, 12, 15]);
    }

    #[tokio::test]
    async fn test_run_batch_blocking_unbounded() {
        let items: Vec<i32> = (0..20).collect();
        let results = run_batch_blocking(items, 0, |x| x + 1).await;

        assert_eq!(results.len(), 20);
        let mut values: Vec<i32> = results.into_iter().map(|r| r.unwrap()).collect();
        values.sort();
        assert_eq!(values, (1..21).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn test_run_batch_empty() {
        let items: Vec<i32> = vec![];
        let results = run_batch_blocking(items, 4, |x| x).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_run_batch_async_unbounded() {
        let items: Vec<i32> = (0..10).collect();
        let results = run_batch(items, 0, |x| async move { x * x }).await;

        assert_eq!(results.len(), 10);
        let mut values: Vec<i32> = results.into_iter().map(|r| r.unwrap()).collect();
        values.sort();
        assert_eq!(values, vec![0, 1, 4, 9, 16, 25, 36, 49, 64, 81]);
    }
}
