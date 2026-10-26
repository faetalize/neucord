//! A cache bounded by the total size of what it holds, evicting the least
//! recently used entries. Small enough (hundreds of entries) that a linear
//! scan to find the oldest is fine.

use std::collections::HashMap;

pub struct Lru<V> {
    entries: HashMap<String, (V, u64, u64)>,
    budget: u64,
    total: u64,
    clock: u64,
}

impl<V: Clone> Lru<V> {
    pub fn new(budget: u64) -> Self {
        Lru { entries: HashMap::new(), budget, total: 0, clock: 0 }
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.2 = self.clock;
        Some(entry.0.clone())
    }

    /// Insert `value`, which costs `size`, and evict until within budget.
    /// Something bigger than the whole budget is not kept.
    pub fn insert(&mut self, key: &str, value: V, size: u64) {
        if let Some((_, old, _)) = self.entries.remove(key) {
            self.total -= old;
        }
        if size > self.budget {
            return;
        }
        self.clock += 1;
        self.total += size;
        self.entries.insert(key.to_owned(), (value, size, self.clock));
        while self.total > self.budget {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| k.clone()) else { break };
            if let Some((_, size, _)) = self.entries.remove(&oldest) {
                self.total -= size;
            }
        }
    }
}

/// Which files to delete so the rest fit in `budget`: oldest first, by
/// last use. Takes `(name, size, last used)`.
pub fn over_budget<T: Clone>(mut files: Vec<(T, u64, std::time::SystemTime)>, budget: u64) -> Vec<T> {
    files.sort_by_key(|f| std::cmp::Reverse(f.2));
    let mut kept = 0;
    files
        .into_iter()
        .filter_map(|(name, size, _)| {
            kept += size;
            (kept > budget).then_some(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn evicts_least_recently_used() {
        let mut lru = Lru::new(10);
        lru.insert("a", 1, 4);
        lru.insert("b", 2, 4);
        assert_eq!(lru.get("a"), Some(1)); // b is now the oldest
        lru.insert("c", 3, 4);
        assert_eq!((lru.get("a"), lru.get("b"), lru.get("c")), (Some(1), None, Some(3)));
        lru.insert("huge", 4, 11);
        assert_eq!(lru.get("huge"), None, "bigger than the budget");
        lru.insert("a", 5, 2);
        assert_eq!(lru.total, 6, "replacing frees the old size");
    }

    #[test]
    fn oldest_files_go_first() {
        let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = vec![("old", 50, at(1)), ("new", 50, at(3)), ("mid", 50, at(2))];
        assert_eq!(over_budget(files.clone(), 100), ["old"]);
        assert_eq!(over_budget(files.clone(), 40), ["new", "mid", "old"]);
        assert!(over_budget(files, 150).is_empty());
    }
}
