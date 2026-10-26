//! Message history per channel, kept for recently viewed channels so going
//! back to one is instant. Each channel has its own `ListStore`; the message
//! list shows whichever is open by swapping its model.
//!
//! Stores hold `BoxedAnyObject<Message>`, oldest first. They are filled from
//! three directions: the newest page when a channel opens (merged into what
//! is cached), older pages as the user scrolls up, and gateway events.

use std::collections::HashMap;

use discord::{Message, Snowflake};
use gtk::gio;
use gtk::prelude::*;

/// Channels kept in memory. The open channel is never evicted.
const CAPACITY: usize = 12;

pub struct Timeline {
    pub store: gio::ListStore,
    /// The oldest message of the channel is loaded; there is nothing to page.
    pub complete: bool,
    /// An older page is being fetched.
    pub loading_older: bool,
    last_used: u64,
}

#[derive(Default)]
pub struct Timelines {
    channels: HashMap<Snowflake, Timeline>,
    clock: u64,
}

impl Timelines {
    /// The channel's timeline, created empty if missing. Marks it as most
    /// recently used and evicts the least recently used beyond capacity.
    pub fn open(&mut self, channel: Snowflake) -> &mut Timeline {
        self.clock += 1;
        let clock = self.clock;
        if !self.channels.contains_key(&channel) && self.channels.len() >= CAPACITY {
            let oldest = self.channels.iter().min_by_key(|(_, t)| t.last_used).map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                self.channels.remove(&oldest);
            }
        }
        let timeline = self.channels.entry(channel).or_insert_with(|| Timeline {
            store: gio::ListStore::new::<glib::BoxedAnyObject>(),
            complete: false,
            loading_older: false,
            last_used: 0,
        });
        timeline.last_used = clock;
        timeline
    }

    pub fn get(&self, channel: Snowflake) -> Option<&Timeline> {
        self.channels.get(&channel)
    }

    pub fn get_mut(&mut self, channel: Snowflake) -> Option<&mut Timeline> {
        self.channels.get_mut(&channel)
    }

    pub fn clear(&mut self) {
        self.channels.clear();
    }
}

pub fn boxed(message: Message) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(message)
}

pub fn message_at(store: &gio::ListStore, position: u32) -> Option<Message> {
    store.item(position).and_downcast::<glib::BoxedAnyObject>().map(|o| o.borrow::<Message>().clone())
}

fn id_at(store: &gio::ListStore, position: u32) -> Option<Snowflake> {
    store.item(position).and_downcast::<glib::BoxedAnyObject>().map(|o| o.borrow::<Message>().id)
}

pub fn find(store: &gio::ListStore, id: Snowflake) -> Option<(u32, Message)> {
    // Recent messages are the likely targets of edits and deletes: search from the end.
    (0..store.n_items()).rev().find_map(|position| {
        let message = message_at(store, position)?;
        (message.id == id).then_some((position, message))
    })
}

pub fn oldest_id(store: &gio::ListStore) -> Option<Snowflake> {
    id_at(store, 0)
}

/// Merge the newest page (oldest first) into a store that may hold an older
/// copy of the channel. Where they overlap, the page replaces the cached tail,
/// which also applies edits and deletes we missed. If the page does not reach
/// back to the cached messages there is a gap, and the store is replaced.
/// Returns whether the store was replaced (so its scrollback was reset).
pub fn merge_newest(store: &gio::ListStore, page: Vec<Message>) -> bool {
    let n = store.n_items();
    let objects: Vec<glib::BoxedAnyObject> = page.iter().cloned().map(boxed).collect();
    let cached_newest = n.checked_sub(1).and_then(|last| id_at(store, last));
    let overlap_from = match (page.first(), cached_newest) {
        (Some(first), Some(newest)) if first.id <= newest => {
            // First cached message at or after the page's oldest.
            (0..n).find(|&p| id_at(store, p).is_some_and(|id| id >= first.id))
        }
        _ => None,
    };
    match overlap_from {
        Some(from) => {
            store.splice(from, n - from, &objects);
            // The message before the splice may now group differently.
            if from > 0 && !objects.is_empty() {
                store.items_changed(from, 1, 1);
            }
            false
        }
        None => {
            store.splice(0, n, &objects);
            true
        }
    }
}

/// Prepend an older page (oldest first).
pub fn prepend(store: &gio::ListStore, page: Vec<Message>) {
    let count = page.len() as u32;
    let objects: Vec<glib::BoxedAnyObject> = page.into_iter().map(boxed).collect();
    store.splice(0, 0, &objects);
    // The previous first message now has one before it: re-bind it so its
    // header groups with the page's last message when it should.
    if count > 0 && store.n_items() > count {
        store.items_changed(count, 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: u64, content: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "channel_id": "1", "author": {"id": "5", "username": "a"},
            "content": content, "timestamp": "2026-10-26T00:00:00+00:00"
        }))
        .unwrap()
    }

    fn ids(store: &gio::ListStore) -> Vec<u64> {
        (0..store.n_items()).map(|p| id_at(store, p).unwrap().0).collect()
    }

    fn store_with(ids: &[u64]) -> gio::ListStore {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for &id in ids {
            store.append(&boxed(msg(id, "")));
        }
        store
    }

    #[test]
    fn merge_overlapping_page_keeps_scrollback() {
        let store = store_with(&[1, 2, 3, 4, 5]);
        // 4 was deleted, 5 edited, 6 and 7 are new.
        let replaced = merge_newest(&store, vec![msg(3, ""), msg(5, "edited"), msg(6, ""), msg(7, "")]);
        assert!(!replaced);
        assert_eq!(ids(&store), [1, 2, 3, 5, 6, 7]);
        assert_eq!(find(&store, Snowflake(5)).unwrap().1.content, "edited");
    }

    #[test]
    fn merge_with_gap_replaces() {
        let store = store_with(&[1, 2, 3]);
        assert!(merge_newest(&store, vec![msg(10, ""), msg(11, "")]));
        assert_eq!(ids(&store), [10, 11]);
    }

    #[test]
    fn merge_into_empty_and_empty_page() {
        let store = store_with(&[]);
        merge_newest(&store, vec![msg(1, ""), msg(2, "")]);
        assert_eq!(ids(&store), [1, 2]);
        merge_newest(&store, vec![]);
        assert_eq!(ids(&store), Vec::<u64>::new(), "channel emptied (all deleted)");
    }

    #[test]
    fn prepend_older_page() {
        let store = store_with(&[5, 6]);
        prepend(&store, vec![msg(3, ""), msg(4, "")]);
        assert_eq!(ids(&store), [3, 4, 5, 6]);
        assert_eq!(oldest_id(&store), Some(Snowflake(3)));
    }

    #[test]
    fn eviction_is_least_recently_used() {
        let mut timelines = Timelines::default();
        for id in 0..CAPACITY as u64 {
            timelines.open(Snowflake(id));
        }
        timelines.open(Snowflake(0)); // touch 0, so 1 is now the oldest
        timelines.open(Snowflake(100));
        assert!(timelines.get(Snowflake(0)).is_some());
        assert!(timelines.get(Snowflake(1)).is_none());
        assert!(timelines.get(Snowflake(100)).is_some());
    }
}
