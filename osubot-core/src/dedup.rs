use crate::log_fmt;
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// A request deduplicator that ensures concurrent identical requests (same key) execute only once.
/// Waiters receive the cached result from the in-flight request.
pub struct RequestDedup<K, V, E> {
    entries: std::sync::Mutex<HashMap<K, Arc<Entry<V, E>>>>,
}

#[derive(Debug)]
enum StoredResult<V, E> {
    Ok(V),
    Err(E),
    Panicked,
}

struct Entry<V, E> {
    result: std::sync::Mutex<Option<StoredResult<V, E>>>,
    done: Semaphore,
    claimed: AtomicBool,
}

struct CleanupGuard<'a, K, V, E>
where
    K: Eq + Hash + Clone,
{
    dedup: &'a RequestDedup<K, V, E>,
    key: K,
    entry: Arc<Entry<V, E>>,
}

impl<'a, K, V, E> Drop for CleanupGuard<'a, K, V, E>
where
    K: Eq + Hash + Clone,
{
    fn drop(&mut self) {
        self.entry.done.close();
        let mut map = match self.dedup.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.remove(&self.key);
    }
}

impl<K, V, E> RequestDedup<K, V, E>
where
    K: Eq + Hash + Clone,
    V: Clone,
    E: Clone,
{
    /// Creates a new empty `RequestDedup`.
    pub fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Executes the closure `f` for the given key, or waits for an in-flight request with the same key.
    /// The first caller becomes the creator and runs `f`; subsequent callers wait and receive the cached result.
    pub async fn run_or_wait<F, Fut>(&self, key: K, f: F) -> Result<V, E>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<V, E>> + Send + 'static,
        V: Send + 'static,
        E: From<&'static str> + Send + 'static,
    {
        let entry = {
            let mut map = self.entries.lock().unwrap_or_else(|e| {
                tracing::warn!("dedup mutex was poisoned, recovering");
                e.into_inner()
            });
            map.entry(key.clone())
                .or_insert_with(|| {
                    Arc::new(Entry {
                        result: std::sync::Mutex::new(None),
                        done: Semaphore::new(0),
                        claimed: AtomicBool::new(false),
                    })
                })
                .clone()
        };

        let is_creator = !entry.claimed.swap(true, Ordering::AcqRel);

        if is_creator {
            let _guard = CleanupGuard {
                dedup: self,
                key: key.clone(),
                entry: entry.clone(),
            };

            let join_handle = tokio::spawn(f());
            let work_result = match join_handle.await {
                Ok(result) => {
                    let stored = match &result {
                        Ok(v) => StoredResult::Ok(v.clone()),
                        Err(e) => StoredResult::Err(e.clone()),
                    };
                    {
                        let mut guard = entry.result.lock().unwrap_or_else(|e| e.into_inner());
                        *guard = Some(stored);
                    }
                    entry.done.close();
                    result
                }
                Err(join_err) => {
                    tracing::warn!(
                        "{}",
                        log_fmt!("dedup.creator_task_failed", error = format!("{join_err}"))
                    );
                    {
                        let mut guard = entry.result.lock().unwrap_or_else(|e| e.into_inner());
                        *guard = Some(StoredResult::Panicked);
                    }
                    entry.done.close();
                    Err(E::from("creator panicked"))
                }
            };

            work_result
        } else {
            let _ = entry.done.acquire().await;
            let guard = entry.result.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(StoredResult::Ok(v)) => Ok(v.clone()),
                Some(StoredResult::Err(e)) => Err(e.clone()),
                Some(StoredResult::Panicked) => Err(E::from("creator panicked")),
                None => Err(E::from("creator abandoned")),
            }
        }
    }
}

impl<K, V, E> Default for RequestDedup<K, V, E>
where
    K: Eq + Hash + Clone,
    V: Clone,
    E: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn test_single_request() {
        let dedup: RequestDedup<u32, String, String> = RequestDedup::new();
        let result = dedup
            .run_or_wait(1, || async { Ok("hello".to_string()) })
            .await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let dedup: RequestDedup<u32, String, String> = RequestDedup::new();
        let result = dedup
            .run_or_wait(1, || async { Err("boom".to_string()) })
            .await;
        assert_eq!(result.unwrap_err(), "boom");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_same_key_single_execution() {
        let dedup = Arc::new(RequestDedup::<u32, String, String>::new());
        let call_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(5));

        let mut handles = vec![];
        for _ in 0..5 {
            let dedup = dedup.clone();
            let call_count = call_count.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                dedup
                    .run_or_wait(1, || {
                        let call_count = call_count.clone();
                        async move {
                            call_count.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Ok("shared".to_string())
                        }
                    })
                    .await
                    .unwrap()
            }));
        }

        for h in handles {
            assert_eq!(h.await.unwrap(), "shared");
        }
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_concurrent_different_keys_independent() {
        let dedup = Arc::new(RequestDedup::<u32, u32, String>::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for i in 0..5 {
            let dedup = dedup.clone();
            let call_count = call_count.clone();
            handles.push(tokio::spawn(async move {
                dedup
                    .run_or_wait(i, move || {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        async move { Ok(i) }
                    })
                    .await
                    .unwrap()
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(call_count.load(Ordering::SeqCst), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_error_shared_to_waiters() {
        let dedup = Arc::new(RequestDedup::<u32, String, String>::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        // Start the creator first, give it time to claim the entry
        let dedup_clone = dedup.clone();
        let call_count_clone = call_count.clone();
        let handle = tokio::spawn(async move {
            dedup_clone
                .run_or_wait(1, || {
                    let call_count = call_count_clone.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        Err("fail".to_string())
                    }
                })
                .await
        });

        // Small delay to ensure creator has claimed the entry
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Now the waiter arrives — it should see the creator's error
        let waiter_result = dedup
            .run_or_wait(1, || {
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    Err("fail2".to_string())
                }
            })
            .await;

        assert_eq!(handle.await.unwrap().unwrap_err(), "fail");
        assert_eq!(waiter_result.unwrap_err(), "fail");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_after_completion() {
        let dedup: RequestDedup<u32, u32, String> = RequestDedup::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let call_count_clone = call_count.clone();
        let result1 = dedup
            .run_or_wait(1, || {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(42) }
            })
            .await;
        assert_eq!(result1.unwrap(), 42);

        let call_count_clone = call_count.clone();
        let result2 = dedup
            .run_or_wait(1, || {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(99) }
            })
            .await;
        assert_eq!(result2.unwrap(), 99);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_creator_panic_waiters_recover() {
        let dedup = Arc::new(RequestDedup::<u32, String, String>::new());

        let dedup_clone = dedup.clone();
        let creator_handle = tokio::spawn(async move {
            dedup_clone
                .run_or_wait(1, || async {
                    // Keep the creator alive so the waiter has time to join
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    panic!("intentional test panic");
                })
                .await
        });

        // Wait for creator to claim the entry
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let waiter_result = dedup
            .run_or_wait(1, || async { Ok("fallback".to_string()) })
            .await;

        assert!(creator_handle.await.unwrap().unwrap_err().contains("panic"));
        assert!(waiter_result.unwrap_err().contains("panic"));
    }

    #[tokio::test]
    async fn test_creator_drop_cleans_map() {
        let dedup = Arc::new(RequestDedup::<u32, String, String>::new());
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let dedup_clone = dedup.clone();
        let barrier_clone = barrier.clone();
        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;
            dedup_clone
                .run_or_wait(1, || async {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    Ok("never".to_string())
                })
                .await
        });

        barrier.wait().await;
        // Give the creator a moment to claim the entry
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        handle.abort();
        let _ = handle.await;

        // After abort, the map should be empty because CleanupGuard::drop runs
        let map = dedup.entries.lock().unwrap();
        assert!(
            !map.contains_key(&1),
            "map should not contain key 1 after creator is aborted"
        );
    }
}
