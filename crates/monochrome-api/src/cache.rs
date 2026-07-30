use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct Cache<V> {
    entries: HashMap<String, Entry<V>>,
    order: Vec<String>,
    capacity: usize,
    max_bytes: usize,
    bytes: usize,
    ttl: Duration,
}

struct Entry<V> {
    stored: Instant,
    weight: usize,
    value: V,
}

impl<V: Clone> Cache<V> {
    pub fn new(capacity: usize, max_bytes: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            capacity,
            max_bytes,
            bytes: 0,
            ttl,
        }
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        let expired = self.entries.get(key)?.stored.elapsed() > self.ttl;
        if expired {
            self.remove(key);
            return None;
        }
        self.touch(key);
        self.entries.get(key).map(|entry| entry.value.clone())
    }

    pub fn insert(&mut self, key: String, value: V, weight: usize) {
        if weight > self.max_bytes {
            return;
        }
        self.remove(&key);
        self.order.push(key.clone());
        self.bytes += weight;
        self.entries.insert(
            key,
            Entry {
                stored: Instant::now(),
                weight,
                value,
            },
        );
        while self.order.len() > self.capacity || self.bytes > self.max_bytes {
            let Some(oldest) = self.order.first().cloned() else {
                break;
            };
            self.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.weight);
        }
        self.order.retain(|entry| entry != key);
    }

    fn touch(&mut self, key: &str) {
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            let entry = self.order.remove(position);
            self.order.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> Cache<u32> {
        Cache::new(2, 1024, Duration::from_secs(60))
    }

    #[test]
    fn evicts_the_least_recently_used_entry() {
        let mut cache = cache();
        cache.insert("a".into(), 1, 1);
        cache.insert("b".into(), 2, 1);
        assert_eq!(cache.get("a"), Some(1));
        cache.insert("c".into(), 3, 1);
        assert_eq!(cache.get("b"), None);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("c"), Some(3));
    }

    #[test]
    fn expired_entries_are_dropped() {
        let mut cache = Cache::new(4, 1024, Duration::from_millis(1));
        cache.insert("a".into(), 1, 1);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.get("a"), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn entry_count_is_never_exceeded() {
        let mut cache = Cache::new(3, 1 << 20, Duration::from_secs(60));
        for i in 0..50 {
            cache.insert(format!("k{i}"), i, 1);
        }
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn byte_budget_is_never_exceeded() {
        let mut cache = Cache::new(1000, 100, Duration::from_secs(60));
        for i in 0..50 {
            cache.insert(format!("k{i}"), i, 30);
        }
        assert!(cache.bytes() <= 100, "{} bytes", cache.bytes());
    }

    #[test]
    fn oversized_entries_are_not_stored() {
        let mut cache = Cache::new(10, 100, Duration::from_secs(60));
        cache.insert("big".into(), 1, 500);
        assert!(cache.is_empty());
    }

    #[test]
    fn reinserting_a_key_does_not_double_count_bytes() {
        let mut cache = Cache::new(10, 1000, Duration::from_secs(60));
        cache.insert("a".into(), 1, 40);
        cache.insert("a".into(), 2, 40);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 40);
        assert_eq!(cache.get("a"), Some(2));
    }
}
