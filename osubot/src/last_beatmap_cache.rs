use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// 群最近查询的谱面缓存，24 小时 TTL。
/// `get` 命中过期时惰性删除，防止慢速内存泄漏。
#[derive(Clone)]
pub struct LastBeatmapCache {
    inner: Arc<Mutex<HashMap<i64, (u32, Instant)>>>,
    ttl: Duration,
}

impl LastBeatmapCache {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(86400))
    }

    /// 主要用于测试。生产用 `new()`。
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    pub fn get(&self, group_id: i64) -> Option<u32> {
        let mut map = self.inner.lock().ok()?;
        let (bid, time) = map.get(&group_id).copied()?;
        if time.elapsed() < self.ttl {
            Some(bid)
        } else {
            map.remove(&group_id);
            None
        }
    }

    pub fn set(&self, group_id: i64, beatmap_id: u32) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(group_id, (beatmap_id, Instant::now()));
        }
    }
}

impl Default for LastBeatmapCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_for_missing() {
        let cache = LastBeatmapCache::new();
        assert_eq!(cache.get(1), None);
    }

    #[test]
    fn returns_set_value_within_ttl() {
        let cache = LastBeatmapCache::new();
        cache.set(1, 42);
        assert_eq!(cache.get(1), Some(42));
    }

    #[test]
    fn returns_none_after_ttl_expires_and_evicts() {
        let cache = LastBeatmapCache::with_ttl(Duration::from_millis(1));
        cache.set(1, 42);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(cache.get(1), None);

        cache.set(1, 99);
        assert_eq!(cache.get(1), Some(99));
    }

    #[test]
    fn evicted_entry_does_not_resurrect() {
        let cache = LastBeatmapCache::with_ttl(Duration::from_millis(1));
        cache.set(1, 42);
        std::thread::sleep(Duration::from_millis(5));
        let _ = cache.get(1);
        let map = cache.inner.lock().unwrap();
        assert!(!map.contains_key(&1), "expired entry not evicted");
    }
}
