//! 全局 shutdown 信号：`Notify` + `AtomicBool` 双保险，无竞态。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use tokio::sync::Notify;

/// 全局 shutdown 通知。
pub static SHUTDOWN_NOTIFY: LazyLock<Notify> = LazyLock::new(Notify::const_new);

/// 等待 shutdown 信号。先检查 `AtomicBool` 避免 `notify_waiters` 竞态：
/// 若信号已在 `notified().await` 之前发出，bool 已为 true，直接返回。
pub async fn wait_for_shutdown(shutdown: &AtomicBool) {
    if shutdown.load(Ordering::Acquire) {
        return;
    }
    SHUTDOWN_NOTIFY.notified().await;
}
