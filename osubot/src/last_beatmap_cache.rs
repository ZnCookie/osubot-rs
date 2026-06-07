use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// 群最近查询的谱面缓存，6 小时 TTL
#[derive(Clone)]
pub struct LastBeatmapCache {
    inner: Arc<Mutex<HashMap<i64, (u32, Instant)>>>,
}

impl LastBeatmapCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&self, group_id: i64) -> Option<u32> {
        let map = self.inner.lock().ok()?;
        map.get(&group_id).and_then(|(bid, time)| {
            if time.elapsed() < Duration::from_secs(21600) {
                Some(*bid)
            } else {
                None
            }
        })
    }

    pub fn set(&self, group_id: i64, beatmap_id: u32) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(group_id, (beatmap_id, Instant::now()));
        }
    }
}
