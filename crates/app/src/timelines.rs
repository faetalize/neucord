//! Message history per channel, kept for recently viewed channels so going
//! back to one is instant. Each channel has its own `ListStore`; the message
//! list shows whichever is open by swapping its model.
//!
//! Stores hold `BoxedAnyObject<Message>`, oldest first. They are filled from
//! three directions: the newest page when a channel opens (merged into what
//! is cached), older pages as the user scrolls up, and gateway events.
//!
//! Jumping to an old message (a reply's original) replaces the store with the
//! page around it. Until scrolling down pages back to the newest message, the
//! store is detached: live messages would leave a gap, so they are not added.

use std::collections::HashMap;

use discord::{Message, Referenced, Snowflake};
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
    /// The store does not reach the newest message (after a jump).
    pub detached: bool,
    /// A newer page is being fetched.
    pub loading_newer: bool,
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
            detached: false,
            loading_newer: false,
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

/// Make the list re-bind `len` rows from `start`, so they redraw from current
/// state. `items_changed` alone is not enough: the list view keeps rows whose
/// item is the same object, so each message gets a fresh box. One at a time,
/// so the view's scroll anchor never falls inside a replaced range.
pub fn rebind(store: &gio::ListStore, start: u32, len: u32) {
    for position in start..(start + len).min(store.n_items()) {
        if let Some(message) = message_at(store, position) {
            store.splice(position, 1, &[boxed(message)]);
        }
    }
}

/// Consecutive messages matching `pred` (say, from blocked users), which the
/// list collapses into one bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub start: u32,
    pub len: u32,
    /// The first message's id, which names the run while messages around it come and go.
    pub first: Snowflake,
}

/// The run of messages matching `pred` that includes `position`, if any.
pub fn run_at(store: &gio::ListStore, position: u32, pred: impl Fn(&Message) -> bool) -> Option<Run> {
    let matches =
        |p: u32| store.item(p).and_downcast::<glib::BoxedAnyObject>().is_some_and(|o| pred(&o.borrow::<Message>()));
    if !matches(position) {
        return None;
    }
    let start = (0..position).rev().take_while(|&p| matches(p)).last().unwrap_or(position);
    let end = (position + 1..store.n_items()).take_while(|&p| matches(p)).last().unwrap_or(position);
    Some(Run { start, len: end - start + 1, first: id_at(store, start)? })
}

pub fn oldest_id(store: &gio::ListStore) -> Option<Snowflake> {
    id_at(store, 0)
}

pub fn newest_id(store: &gio::ListStore) -> Option<Snowflake> {
    store.n_items().checked_sub(1).and_then(|last| id_at(store, last))
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

/// Drop all but the newest `keep` messages. The timeline can page back again,
/// and an older page still in flight will no longer line up, so it is dropped.
pub fn keep_newest(timeline: &mut Timeline, keep: u32) {
    let n = timeline.store.n_items();
    if n > keep {
        timeline.store.splice(0, n - keep, &[] as &[glib::BoxedAnyObject]);
        timeline.complete = false;
    }
}

/// Replace everything with a page around a message we jumped to.
pub fn replace(timeline: &mut Timeline, page: Vec<Message>, detached: bool) {
    let objects: Vec<glib::BoxedAnyObject> = page.into_iter().map(boxed).collect();
    timeline.store.splice(0, timeline.store.n_items(), &objects);
    timeline.complete = false;
    timeline.detached = detached;
}

/// Append a newer page (oldest first), scrolling down from a jump.
pub fn append(store: &gio::ListStore, page: Vec<Message>) {
    let objects: Vec<glib::BoxedAnyObject> = page.into_iter().map(boxed).collect();
    store.splice(store.n_items(), 0, &objects);
}

/// Apply `change` to the copy of `original` that each reply to it carries,
/// so replies follow edits and deletes of what they answer.
pub fn update_replies(store: &gio::ListStore, original: Snowflake, change: impl Fn(&mut Referenced)) {
    for position in 0..store.n_items() {
        let Some(object) = store.item(position).and_downcast::<glib::BoxedAnyObject>() else { continue };
        if object.borrow::<Message>().reply_to() != Some(original) {
            continue;
        }
        let mut message = object.borrow::<Message>().clone();
        change(&mut message.referenced_message);
        store.splice(position, 1, &[boxed(message)]);
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
        rebind(store, count, 1);
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

    #[test]
    fn runs_of_matching_messages() {
        let store = store_with(&[1, 2, 3, 4, 5, 6]);
        let middle = |m: &Message| (2..=4).contains(&m.id.0);
        assert_eq!(run_at(&store, 0, middle), None);
        let run = Run { start: 1, len: 3, first: Snowflake(2) };
        assert_eq!([1, 2, 3].map(|p| run_at(&store, p, middle)), [Some(run); 3]);
        assert_eq!(run_at(&store, 5, |m| m.id.0 >= 5), Some(Run { start: 4, len: 2, first: Snowflake(5) }));
        assert_eq!(run_at(&store, 0, |_| true).map(|r| r.len), Some(6));
    }

    #[test]
    fn rebind_swaps_in_fresh_objects() {
        let store = store_with(&[1, 2, 3]);
        let before: Vec<_> = (0..3).map(|p| store.item(p).unwrap()).collect();
        rebind(&store, 1, 5);
        assert_eq!(ids(&store), [1, 2, 3], "same messages, same order; the range is clamped");
        assert_eq!(store.item(0).unwrap(), before[0]);
        assert!(store.item(1).unwrap() != before[1] && store.item(2).unwrap() != before[2]);
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
    fn jump_detaches_until_newest_merges() {
        let mut timelines = Timelines::default();
        let timeline = timelines.open(Snowflake(1));
        for id in 50..=60 {
            timeline.store.append(&boxed(msg(id, "")));
        }
        replace(timeline, vec![msg(10, ""), msg(11, ""), msg(12, "")], true);
        assert!(timeline.detached && !timeline.complete);
        append(&timeline.store, vec![msg(13, ""), msg(14, "")]);
        assert_eq!(ids(&timeline.store), [10, 11, 12, 13, 14]);
        assert_eq!(newest_id(&timeline.store), Some(Snowflake(14)));
        // The newest page does not reach back to the jump: a gap, so replaced.
        assert!(merge_newest(&timeline.store, vec![msg(60, ""), msg(61, "")]));
    }

    #[test]
    fn replies_follow_their_original() {
        let reply: Message = serde_json::from_value(serde_json::json!({
            "id": "3", "channel_id": "1", "author": {"id": "5", "username": "a"}, "timestamp": "t", "type": 19,
            "message_reference": {"message_id": "2"},
            "referenced_message": {"id": "2", "channel_id": "1", "author": {"id": "6", "username": "b"},
                                   "content": "before", "timestamp": "t"}
        }))
        .unwrap();
        let store = store_with(&[1, 2]);
        store.append(&boxed(reply));
        let original = || find(&store, Snowflake(3)).unwrap().1.referenced_message;

        update_replies(&store, Snowflake(2), |r| {
            if let Referenced::Message(m) = r {
                m.content = "after".into();
            }
        });
        assert!(matches!(original(), Referenced::Message(m) if m.content == "after"));
        update_replies(&store, Snowflake(2), |r| *r = Referenced::Deleted);
        assert!(matches!(original(), Referenced::Deleted));
        assert_eq!(ids(&store), [1, 2, 3]);
    }

    #[test]
    fn leaving_a_channel_keeps_the_newest() {
        let mut timelines = Timelines::default();
        let timeline = timelines.open(Snowflake(1));
        for id in 1..=10 {
            timeline.store.append(&boxed(msg(id, "")));
        }
        timeline.complete = true;
        keep_newest(timeline, 4);
        assert_eq!(ids(&timeline.store), [7, 8, 9, 10]);
        assert!(!timeline.complete, "trimmed history can be paged again");
        keep_newest(timeline, 40);
        assert_eq!(timeline.store.n_items(), 4, "nothing to trim");
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
