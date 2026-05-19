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
