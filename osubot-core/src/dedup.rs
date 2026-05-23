use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

pub struct RequestDedup<K, V, E> {
    entries: Mutex<HashMap<K, Arc<Entry<V, E>>>>,
}

struct Entry<V, E> {
    result: std::sync::Mutex<Option<Result<V, E>>>,
    done: Semaphore,
    claimed: AtomicBool,
}

impl<K, V, E> RequestDedup<K, V, E>
where
    K: Eq + Hash + Clone,
    V: Clone,
    E: Clone,
{
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub async fn run_or_wait<F, Fut>(&self, key: K, f: F) -> Result<V, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        let entry = {
            let mut map = self.entries.lock().await;
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
            let work_result = f().await;
            {
                let mut guard = entry.result.lock().unwrap();
                *guard = Some(work_result.clone());
            }
            entry.done.close();
            self.entries.lock().await.remove(&key);
            work_result
        } else {
            let _ = entry.done.acquire().await;
            let guard = entry.result.lock().unwrap();
            guard
                .as_ref()
                .expect("result must be set by creator; panicking closure poisons the entry")
                .clone()
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
                    .run_or_wait(i, || {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        async { Ok(i) }
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
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let dedup_clone = dedup.clone();
        let call_count_clone = call_count.clone();
        let barrier_clone = barrier.clone();
        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;
            dedup_clone
                .run_or_wait(1, || {
                    let call_count = call_count_clone.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        Err("fail".to_string())
                    }
                })
                .await
        });

        barrier.wait().await;
        let call_count_clone2 = call_count.clone();
        let waiter_result = dedup
            .run_or_wait(1, || {
                let call_count = call_count_clone2.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
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
}
