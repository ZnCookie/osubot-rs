use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex, Notify};
use tokio::time::{self, timeout, Duration};

/// Error returned when acquiring a token times out, indicating the rate limit has been exceeded.
#[derive(Debug)]
pub struct RateLimitError;

/// Token-bucket rate limiter with configurable burst capacity and per-minute refill rate.
pub struct RateLimiter {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
    burst: u32,
    per_minute: u32,
    refill_handle: StdMutex<tokio::task::JoinHandle<()>>,
    refill_respawning: Arc<AtomicBool>,
}

struct State {
    tokens: f64,
}

/// Spawn the token refill background task.
/// Returns a JoinHandle so callers can detect if the task panicked and respawn.
fn spawn_refill(
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
    burst_f: f64,
    per_minute: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(200));
        let increment = per_minute as f64 / 300.0;
        loop {
            interval.tick().await;
            let mut s = state.lock().await;
            let old_whole = s.tokens.floor() as u32;
            s.tokens = (s.tokens + increment).min(burst_f);
            let new_whole = s.tokens.floor() as u32;
            if new_whole > old_whole {
                drop(s);
                for _ in 0..(new_whole - old_whole) {
                    notify.notify_one();
                }
            }
        }
    })
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    /// Creates a new `RateLimiter` with default configuration (burst=60, per_minute=60).
    pub fn new() -> Self {
        Self::with_config(60, 60)
    }

    /// Creates a new `RateLimiter` with custom burst capacity and per-minute refill rate.
    pub fn with_config(burst: u32, per_minute: u32) -> Self {
        let burst_f = burst as f64;
        let state = Arc::new(Mutex::new(State { tokens: burst_f }));
        let notify = Arc::new(Notify::new());

        let refill_handle = StdMutex::new(spawn_refill(
            state.clone(),
            notify.clone(),
            burst_f,
            per_minute,
        ));

        Self {
            state,
            notify,
            burst,
            per_minute,
            refill_handle,
            refill_respawning: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the refill task is alive and respawn if it panicked.
    fn ensure_refill_alive(&self) {
        if let Ok(mut guard) = self.refill_handle.lock() {
            if guard.is_finished()
                && self
                    .refill_respawning
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                tracing::error!("rate limiter refill task panicked, respawning");
                *guard = spawn_refill(
                    self.state.clone(),
                    self.notify.clone(),
                    self.burst as f64,
                    self.per_minute,
                );
                self.refill_respawning.store(false, Ordering::Release);
            }
        }
    }

    /// Acquires a token, waiting up to 10 seconds if none are available.
    /// Returns `Ok(())` on success or `Err(RateLimitError)` on timeout.
    pub async fn acquire(&self) -> Result<(), RateLimitError> {
        loop {
            {
                let mut s = self.state.lock().await;
                if s.tokens >= 1.0 {
                    s.tokens -= 1.0;
                    return Ok(());
                }
            }

            self.ensure_refill_alive();

            match timeout(Duration::from_secs(10), self.notify.notified()).await {
                Ok(_) => continue,
                Err(_) => return Err(RateLimitError),
            }
        }
    }

    /// Attempts to acquire a token without waiting. Returns `true` if successful, `false` otherwise.
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
        if let Ok(guard) = self.refill_handle.lock() {
            guard.abort();
        }
    }
}
