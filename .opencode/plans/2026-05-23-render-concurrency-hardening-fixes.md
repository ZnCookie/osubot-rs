# Render Concurrency Hardening Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix critical and important issues from code review of the render concurrency hardening commit (1ee5088).

**Architecture:** Fix `RequestDedup` panic safety by spawning the creator work and propagating results via `JoinHandle`; move semaphore acquisition past image inlining; preserve `RenderError` context in logs; use `Arc<Vec<u8>>` in dedup to avoid cloning large image payloads.

**Tech Stack:** Rust, tokio, osubot-core, osubot-render

---

### Task 1: Fix RequestDedup panic safety (Critical)

If the creator task panics (e.g., `spawn_blocking` panic), `entry.done.close()` never runs, and all waiters block forever on `entry.done.acquire()`. Fix by using `tokio::spawn` + `JoinHandle` to isolate the creator, closing the semaphore on both success and panic.

**Files:**
- Modify: `osubot-core/src/dedup.rs`

- [ ] **Step 1: Write failing test — creator panic leaves waiters unblocked**

Add a test that verifies waiters receive an error (not deadlock) when the creator panics:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_creator_panic_waiters_recover() {
    use std::panic;

    let dedup = Arc::new(RequestDedup::<u32, String, String>::new());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    let dedup_clone = dedup.clone();
    let barrier_clone = barrier.clone();
    let handle = tokio::spawn(async move {
        barrier_clone.wait().await;
        dedup_clone
            .run_or_wait(1, || async {
                panic!("intentional test panic");
            })
            .await
    });

    barrier.wait().await;
    let waiter_result = dedup
        .run_or_wait(1, || async { Ok("fallback".to_string()) })
        .await;

    assert!(waiter_result.is_err());
    assert!(waiter_result.unwrap_err().contains("panic"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p osubot-core test_creator_panic_waiters_recover -- --nocapture`
Expected: FAIL (deadlock / timeout / panic not propagated)

- [ ] **Step 3: Refactor Entry and run_or_wait — spawn creator via tokio::spawn**

Replace the current creator path (which calls `f().await` directly) with `tokio::spawn(f())` and handle `JoinError` from the `JoinHandle`. Add `StoredResult` enum to track success/error/panicked states. Add `E: From<&'static str>` bound for constructing the "creator panicked" error.

**Full new implementation for `dedup.rs`:**

```rust
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

pub struct RequestDedup<K, V, E> {
    entries: Mutex<HashMap<K, Arc<Entry<V, E>>>>,
}

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
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<V, E>> + Send,
        V: Send + 'static,
        E: From<&'static str> + Send + 'static,
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
            let join_handle = tokio::spawn(f());
            let work_result = match join_handle.await {
                Ok(result) => {
                    let stored = match &result {
                        Ok(v) => StoredResult::Ok(v.clone()),
                        Err(e) => StoredResult::Err(e.clone()),
                    };
                    {
                        let mut guard = entry.result.lock().unwrap();
                        *guard = Some(stored);
                    }
                    entry.done.close();
                    result
                }
                Err(_) => {
                    {
                        let mut guard = entry.result.lock().unwrap();
                        *guard = Some(StoredResult::Panicked);
                    }
                    entry.done.close();
                    Err(E::from("creator panicked"))
                }
            };

            // Safe to remove: waiters hold Arc<Entry> clones, so the entry
            // stays alive even after removal from the map.
            self.entries.lock().await.remove(&key);
            work_result
        } else {
            let _ = entry.done.acquire().await;
            let guard = entry.result.lock().unwrap();
            match guard.as_ref().expect("result must be set by creator") {
                StoredResult::Ok(v) => Ok(v.clone()),
                StoredResult::Err(e) => Err(e.clone()),
                StoredResult::Panicked => Err(E::from("creator panicked")),
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
```

Note: The `Default` impl does NOT include the `E: From<&'static str>` bound — `Default` only requires construction, not `run_or_wait` usage. However, `run_or_wait` has the bound on the method, not the struct. This is fine because `run_or_wait` is the only method and callers will satisfy the bound at the call site.

- [ ] **Step 4: Update existing tests for new trait bounds**

The `run_or_wait` method now requires `F: FnOnce() -> Fut + Send` and `Fut: Future<Output = Result<V, E>> + Send`. Tests that use async closures in `run_or_wait` need to ensure these bounds are met. All current tests use `async { Ok(...) }` / `async { Err(...) }` closures which satisfy `Send`. The key change: `String: From<&'static str>` is required now. Since `String` implements `From<&str>`, this is satisfied.

- [ ] **Step 5: Run all dedup tests to verify they pass**

Run: `cargo test -p osubot-core -- dedup`
Expected: All existing tests pass, plus the new panic recovery test

- [ ] **Step 6: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add osubot-core/src/dedup.rs
git commit -m "fix: make RequestDedup panic-safe via tokio::spawn + JoinHandle

If the creator task panics (e.g. spawn_blocking panic), waiters no longer
deadlock. The creator is spawned in a tokio task; JoinHandle::await catches
panics and propagates a 'creator panicked' error to all waiters."
```

---

### Task 2: Move semaphore acquisition past image inlining (Important)

The `render_semaphore` permit is currently acquired before `cache::inline_external_images` (network I/O), meaning a render slot is consumed during network fetches. Move the permit acquisition to just before `spawn_blocking`.

**Files:**
- Modify: `osubot-render/src/lib.rs`

- [ ] **Step 1: Move `_permit` acquisition past image inlining**

Current code (`lib.rs:35-39`):
```rust
let _permit = render_semaphore()
    .acquire()
    .await
    .expect("render semaphore never closed");
let html_with_inlined_images = cache::inline_external_images(html).await;
```

Change to:
```rust
let html_with_inlined_images = cache::inline_external_images(html).await;
let _permit = render_semaphore()
    .acquire()
    .await
    .expect("render semaphore never closed");
```

The `_permit` must be held past `spawn_blocking` to ensure the semaphore slot is occupied during the render. Since `_permit` is used later (held for the entire render scope), moving it below the image inlining call is correct — `_permit` will still be alive until end of function.

- [ ] **Step 2: Verify existing test still compiles and passes**

Run: `cargo test -p osubot-render`
Expected: All tests pass (the smoke test is `#[ignore]` but compilation should succeed)

- [ ] **Step 3: Commit**

```bash
git add osubot-render/src/lib.rs
git commit -m "perf: move render semaphore past image inlining

Reduces effective concurrency waste by not occupying a render slot
during network I/O for image prefetching."
```

---

### Task 3: Preserve RenderError context in logs (Important)

The `.map_err(|_| "渲染失败，请稍后重试".to_string())` in `main.rs:666` discards the `RenderError`, losing diagnostic info. Log the original error before converting.

**Files:**
- Modify: `osubot/src/main.rs`

- [ ] **Step 1: Log RenderError before discarding**

Current code (`main.rs:666`):
```rust
.map_err(|_| "渲染失败，请稍后重试".to_string())
```

Change to:
```rust
.map_err(|e| {
    warn!(user_id = target_user_id, error = %e, "render failed");
    "渲染失败，请稍后重试".to_string()
})
```

Note: `target_user_id` is already in scope (`i64`). The `warn!` macro and `RenderError` (which implements `Display` via `thiserror`) are already imported.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p osubot`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add osubot/src/main.rs
git commit -m "fix: log RenderError detail before converting to user-facing message

Preserves diagnostic information for debugging while keeping user-facing
message unchanged."
```

---

### Task 4: Use Arc<Vec<u8>> in RequestDedup to avoid cloning large payloads (Minor)

The dedup currently stores `Vec<u8>` (rendered JPEG image), and both creator and waiter clone the full byte vector. Using `Arc<Vec<u8>>` avoids a full copy for the stored result.

**Files:**
- Modify: `osubot/src/main.rs`

- [ ] **Step 1: Update the dedup type and closure**

In `main.rs:166`, change:
```rust
static DEDUP: OnceLock<RequestDedup<i64, Vec<u8>, String>> = OnceLock::new();
```
to:
```rust
static DEDUP: OnceLock<RequestDedup<i64, Arc<Vec<u8>>, String>> = OnceLock::new();
```

`Arc` is already imported (`use std::sync::Arc`).

In `main.rs:638-668`, update the `run_or_wait` closure to wrap the result:
```rust
.map(|bytes| Arc::new(bytes))
```

The success path (`main.rs:671`) uses `jpeg_bytes.len()` and passes `&jpeg_bytes` to `send_group_msg_with_image`. Since `Arc<Vec<u8>>` auto-derefs to `&[u8]`, no change needed there.

- [ ] **Step 2: Verify full workspace compiles and tests pass**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 3: Commit**

```bash
git add osubot/src/main.rs
git commit -m "perf: use Arc<Vec<u8>> as dedup value to avoid cloning rendered images

Rendered JPEG images can be hundreds of KB. Wrapping them in Arc avoids
a full clone when storing in the dedup cache — both creator and waiters
share the same allocation."
```

---

### Task 5: Add constant for semaphore limit (Minor)

`Semaphore::new(3)` is a magic number. Extract it to a named constant with a comment.

**Files:**
- Modify: `osubot-render/src/lib.rs`

- [ ] **Step 1: Add constant and update semaphore initialization**

```rust
/// Maximum concurrent render operations. Render is CPU-intensive (font rasterization,
/// layout, paint), so this limits parallel renders to avoid saturating CPU cores.
const MAX_CONCURRENT_RENDERS: usize = 3;

static RENDER_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn render_semaphore() -> &'static Semaphore {
    RENDER_SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_RENDERS))
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p osubot-render`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add osubot-render/src/lib.rs
git commit -m "refactor: extract render concurrency limit into named constant"
```

---

### Task 6: Document the render timeout change (Minor)

The timeout was changed from 30s to 60s without explanation. Add a comment.

**Files:**
- Modify: `osubot-render/src/lib.rs`

- [ ] **Step 1: Add comment explaining the 60s timeout**

```rust
    // 60s timeout: profile cards with many badges or large user stats can
    // take significant time to render, especially under concurrent load.
    let (mut pixels, mut w, mut h) = tokio::time::timeout(
        std::time::Duration::from_secs(60),
```

- [ ] **Step 2: Commit**

```bash
git add osubot-render/src/lib.rs
git commit -m "docs: add comment explaining 60s render timeout"
```

---

## Task Dependency Graph

```
Task 1 (dedup panic safety) ← Task 4 (Arc<Vec<u8>>) depends on Task 1's final dedup signature
Task 2 (semaphore placement) — independent
Task 3 (RenderError logging) — independent
Task 5 (constant) — independent
Task 6 (timeout comment) — independent
```

Recommended execution order: Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6