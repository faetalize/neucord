//! The most recently used few of something, by key.

use std::collections::VecDeque;

/// Holds up to `capacity` values; making room drops the one used longest ago.
pub struct Kept<K, V> {
    /// Newest first.
    entries: VecDeque<(K, V)>,
    capacity: usize,
}

impl<K: PartialEq, V> Kept<K, V> {
    pub fn new(capacity: usize) -> Self {
        Kept { entries: VecDeque::new(), capacity }
    }

    /// Keep `value` under `key`, in place of what that key had.
    pub fn put(&mut self, key: K, value: V) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push_front((key, value));
        self.entries.truncate(self.capacity);
    }

    /// Hand back what `key` holds, if `usable` says it still is. Either way
    /// the entry is gone: a taker that gets it owns it, and one that
    /// doesn't has found it out of date.
    pub fn take(&mut self, key: &K, usable: impl FnOnce(&V) -> bool) -> Option<V> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        let (_, value) = self.entries.remove(at)?;
        usable(&value).then_some(value)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_one_used_longest_ago() {
        let mut kept = Kept::new(2);
        kept.put(1, "a");
        kept.put(2, "b");
        kept.put(3, "c");
        assert_eq!(kept.take(&1, |_| true), None);
        assert_eq!(kept.take(&2, |_| true), Some("b"));
        assert_eq!(kept.take(&3, |_| true), Some("c"));
    }

    #[test]
    fn putting_again_counts_as_use() {
        let mut kept = Kept::new(2);
        kept.put(1, "a");
        kept.put(2, "b");
        kept.put(1, "a2");
        kept.put(3, "c");
        assert_eq!(kept.take(&2, |_| true), None, "2 was the one unused longest");
        assert_eq!(kept.take(&1, |_| true), Some("a2"));
    }

    #[test]
    fn taking_removes_even_what_is_out_of_date() {
        let mut kept = Kept::new(2);
        kept.put(1, "a");
        assert_eq!(kept.take(&1, |_| false), None);
        assert_eq!(kept.take(&1, |_| true), None, "gone, not left for the next taker");
    }
}
