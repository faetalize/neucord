//! The most recently used few of something, by key.

use std::collections::VecDeque;

use discord::Message;

/// Something made for a message, and what it was made from: it stands in
/// for making it again only while all of that is as it was.
pub struct Made<T> {
    pub message: Message,
    /// The `row_epoch` it was made at, which counts changes to everything
    /// else a message is drawn from: names, settings, colours.
    pub epoch: u64,
    pub made: T,
}

impl<T> Made<T> {
    /// Whether it is what `message` would be drawn as at `epoch`. A message
    /// is compared whole, so an edit, a link preview arriving, a vote or
    /// anything else it carries ends the use of what was made.
    pub fn fits(&self, message: &Message, epoch: u64) -> bool {
        self.epoch == epoch && self.message == *message
    }
}

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
    use discord::MessageEdit;
    use serde_json::{Value, json};

    fn message(extra: Value) -> Message {
        let mut message = json!({"id": "3", "channel_id": "9", "type": 0, "content": "hi",
                                 "author": {"id": "5", "username": "a"}, "timestamp": "2026-10-26T00:00:00+00:00"});
        message.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(message).unwrap()
    }

    fn poll() -> Value {
        json!({"poll": {"question": {"text": "q"}, "answers": [{"answer_id": 1, "poll_media": {"text": "a"}}]}})
    }

    fn made(message: &Message) -> Made<()> {
        Made { message: message.clone(), epoch: 4, made: () }
    }

    #[test]
    fn the_same_message_fits() {
        let shown = message(poll());
        assert!(made(&shown).fits(&shown.clone(), 4));
    }

    #[test]
    fn a_new_epoch_doesnt_fit() {
        let shown = message(json!({}));
        assert!(!made(&shown).fits(&shown, 5));
    }

    #[test]
    fn an_edit_doesnt_fit() {
        let shown = message(json!({}));
        let kept = made(&shown);
        let mut edited = shown.clone();
        MessageEdit { content: Some("hi there".into()), edited: true, ..Default::default() }.apply(&mut edited);
        assert!(!kept.fits(&edited, 4));
    }

    #[test]
    fn a_link_preview_arriving_later_doesnt_fit() {
        // It comes as an update with only `embeds`.
        let shown = message(json!({"content": "https://example.com"}));
        let kept = made(&shown);
        let embed =
            serde_json::from_value(json!({"type": "link", "url": "https://example.com", "title": "t"})).unwrap();
        let mut updated = shown.clone();
        MessageEdit { embeds: Some(vec![embed]), ..Default::default() }.apply(&mut updated);
        assert!(!kept.fits(&updated, 4));
    }

    #[test]
    fn a_vote_doesnt_fit() {
        let shown = message(poll());
        let kept = made(&shown);
        let mut voted = shown.clone();
        voted.poll.as_mut().unwrap().vote(1, false, false);
        assert!(!kept.fits(&voted, 4));
    }

    #[test]
    fn a_reaction_arriving_or_going_doesnt_fit() {
        use discord::model::Emoji;
        use discord::reactions::Kind;
        let fire = Emoji { id: None, name: Some("🔥".into()), animated: false };
        let shown = message(json!({}));
        let kept = made(&shown);
        let mut reacted = shown.clone();
        reacted.add_reaction(&fire, false, Kind::Normal, &[], false);
        assert!(!kept.fits(&reacted, 4), "a pill to add");

        let kept = made(&reacted);
        let mut counted = reacted.clone();
        counted.add_reaction(&fire, true, Kind::Normal, &[], false);
        assert!(!kept.fits(&counted, 4), "a count and a pill we lit");
        let mut gone = reacted.clone();
        gone.remove_reaction(&fire, false, Kind::Normal);
        assert!(!kept.fits(&gone, 4), "a pill to take away");
        assert!(kept.fits(&reacted.clone(), 4), "no change, no rebuild");
    }

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
