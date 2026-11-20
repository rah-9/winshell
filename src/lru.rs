//! Byte-budgeted LRU map (PLAN.md section 6, `IconCache`: hard ceiling 8 MB).

use std::collections::HashMap;

pub struct Lru<V> {
    entries: HashMap<String, (V, usize, u64)>,
    capacity_bytes: usize,
    current_bytes: usize,
    tick: u64,
}

impl<V> Lru<V> {
    pub fn new(capacity_bytes: usize) -> Self {
        Self { entries: HashMap::new(), capacity_bytes, current_bytes: 0, tick: 0 }
    }

    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Look up and mark as most recently used.
    pub fn get(&mut self, key: &str) -> Option<&V> {
        self.tick += 1;
        let t = self.tick;
        self.entries.get_mut(key).map(|e| {
            e.2 = t;
            &e.0
        })
    }

    /// Insert (replacing any previous value), evicting least recently used
    /// entries until the budget fits. Returns the evicted keys. An entry
    /// larger than the whole budget is not stored.
    pub fn insert(&mut self, key: String, value: V, bytes: usize) -> Vec<String> {
        self.remove(&key);
        if bytes > self.capacity_bytes {
            return Vec::new();
        }
        let mut evicted = Vec::new();
        while self.current_bytes + bytes > self.capacity_bytes {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| k.clone()) else { break };
            self.remove(&oldest);
            evicted.push(oldest);
        }
        self.tick += 1;
        self.current_bytes += bytes;
        self.entries.insert(key, (value, bytes, self.tick));
        evicted
    }

    pub fn remove(&mut self, key: &str) -> Option<V> {
        self.entries.remove(key).map(|(v, b, _)| {
            self.current_bytes -= b;
            v
        })
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut c = Lru::new(30);
        c.insert("a".into(), 1, 10);
        c.insert("b".into(), 2, 10);
        c.insert("c".into(), 3, 10);
        c.get("a"); // b is now the oldest
        let ev = c.insert("d".into(), 4, 10);
        assert_eq!(ev, ["b"]);
        assert!(c.contains("a") && c.contains("c") && c.contains("d"));
        assert_eq!(c.current_bytes(), 30);
    }

    #[test]
    fn replacing_updates_byte_count() {
        let mut c = Lru::new(100);
        c.insert("a".into(), 1, 40);
        c.insert("a".into(), 2, 10);
        assert_eq!(c.current_bytes(), 10);
        assert_eq!(c.get("a"), Some(&2));
    }

    #[test]
    fn oversized_entry_is_not_stored() {
        let mut c = Lru::new(10);
        c.insert("a".into(), 1, 5);
        assert!(c.insert("big".into(), 2, 11).is_empty());
        assert!(!c.contains("big"));
        assert!(c.contains("a"));
    }

    #[test]
    fn evicts_several_to_fit() {
        let mut c = Lru::new(30);
        c.insert("a".into(), 1, 10);
        c.insert("b".into(), 2, 10);
        c.insert("c".into(), 3, 10);
        let ev = c.insert("d".into(), 4, 25);
        assert_eq!(ev.len(), 3);
        assert_eq!(c.len(), 1);
        assert_eq!(c.current_bytes(), 25);
    }
}
