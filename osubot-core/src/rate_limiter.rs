use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::time::{self, timeout, Duration};

#[derive(Debug)]
pub struct RateLimitError;

pub struct RateLimiter {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
    _refill: tokio::task::AbortHandle,
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
        Self::with_config(60, 60)
    }

    pub fn with_config(burst: u32, per_minute: u32) -> Self {
        let burst_f = burst as f64;
        let state = Arc::new(Mutex::new(State { tokens: burst_f }));
        let notify = Arc::new(Notify::new());

        let state_clone = state.clone();
        let notify_clone = notify.clone();
        let handle = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(200));
            let increment = per_minute as f64 / 300.0;
            loop {
                interval.tick().await;
                let mut s = state_clone.lock().await;
                let old_whole = s.tokens.floor() as u32;
                s.tokens = (s.tokens + increment).min(burst_f);
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
            _refill: handle.abort_handle(),
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

impl Drop for RateLimiter {
    fn drop(&mut self) {
        self._refill.abort();
    }
}
