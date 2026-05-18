use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::time::{self, Duration, timeout};

#[derive(Debug)]
pub struct RateLimitError;

pub struct RateLimiter {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
    _refill: tokio::task::JoinHandle<()>,
}

struct State {
    tokens: f64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(State { tokens: 60.0 }));
        let notify = Arc::new(Notify::new());

        let state_clone = state.clone();
        let notify_clone = notify.clone();
        let _refill = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(200));
            loop {
                interval.tick().await;
                let mut s = state_clone.lock().await;
                let old_whole = s.tokens.floor() as u32;
                s.tokens = (s.tokens + 0.2).min(60.0);
                let new_whole = s.tokens.floor() as u32;
                if new_whole > old_whole {
                    drop(s);
                    for _ in 0..(new_whole - old_whole) {
                        notify_clone.notify_one();
                    }
                }
            }
        });

        Self {
            state,
            notify,
            _refill,
        }
    }

    pub async fn acquire(&self) -> Result<(), RateLimitError> {
        loop {
            {
                let mut s = self.state.lock().await;
                if s.tokens >= 1.0 {
                    s.tokens -= 1.0;
                    return Ok(());
                }
            }

            match timeout(Duration::from_secs(10), self.notify.notified()).await {
                Ok(_) => continue,
                Err(_) => return Err(RateLimitError),
            }
        }
    }

    pub async fn try_acquire(&self) -> bool {
        let mut s = self.state.lock().await;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_try_acquire_exhaustion() {
        let rl = RateLimiter::new();
        for _ in 0..60 {
            assert!(rl.try_acquire().await);
        }
        assert!(!rl.try_acquire().await);
    }

    #[tokio::test]
    async fn test_acquire_waits_for_refill() {
        let rl = RateLimiter::new();
        for _ in 0..60 {
            rl.try_acquire().await;
        }
        let start = std::time::Instant::now();
        rl.acquire().await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200),
            "should wait at least 200ms, got {:?}", elapsed
        );
        assert!(
            elapsed <= Duration::from_secs(2),
            "should succeed within 2s, took {:?}", elapsed
        );
    }

    #[tokio::test]
    async fn test_acquire_timeout_under_load() {
        let rl = Arc::new(RateLimiter::new());
        for _ in 0..60 {
            rl.try_acquire().await;
        }

        let mut handles = Vec::new();
        for _ in 0..61 {
            let rl = rl.clone();
            handles.push(tokio::spawn(async move { rl.acquire().await }));
        }

        let mut ok = 0;
        let mut err = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(()) => ok += 1,
                Err(_) => err += 1,
            }
        }

        assert!(ok > 0, "some should succeed: {ok} ok, {err} err");
        assert!(err > 0, "some should timeout: {ok} ok, {err} err");
    }

    #[tokio::test]
    async fn test_token_cap_never_exceeds_60() {
        let rl = RateLimiter::new();
        tokio::time::sleep(Duration::from_secs(2)).await;

        let mut count = 0;
        for _ in 0..65 {
            if rl.try_acquire().await {
                count += 1;
            } else {
                break;
            }
        }
        assert!(count <= 62, "should not exceed 60 + small refill, got {count}");
        assert!(count >= 58, "should have ~60 tokens, got {count}");
    }
}
