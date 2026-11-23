use std::collections::HashMap;
use std::sync::Mutex;

use crate::paths::now_secs;

#[derive(Default)]
pub struct Throttle {
    hits: Mutex<HashMap<String, Vec<i64>>>,
}

impl Throttle {
    pub fn allow(&self, key: &str, limit: usize, window_secs: i64) -> bool {
        let now = now_secs();
        let Ok(mut hits) = self.hits.lock() else {
            return false;
        };
        let entry = hits.entry(key.to_string()).or_default();
        entry.retain(|stamp| now.saturating_sub(*stamp) < window_secs);
        if entry.len() >= limit {
            return false;
        }
        entry.push(now);
        true
    }
}

#[derive(Default)]
pub struct BasicCache {
    inner: Mutex<HashMap<String, (i64, i64)>>,
}

impl BasicCache {
    pub fn get(&self, key: &str) -> Option<i64> {
        let now = now_secs();
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        match inner.get(key).copied() {
            Some((id, exp)) if exp > now => Some(id),
            Some(_) => {
                inner.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn insert(&self, key: String, user_id: i64, ttl: i64) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.len() > 64 {
            inner.clear();
        }
        inner.insert(key, (user_id, now_secs() + ttl));
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.clear();
        }
    }
}
