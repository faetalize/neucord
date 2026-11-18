//! The older messages of a channel we left, kept in memory so scrolling back
//! asks for nothing the client wouldn't: its message cache (755364) takes
//! what `truncateTop` cuts, at most 150, and `loadFromCache` serves the next
//! page from it before any request.

use crate::{Message, Snowflake};

/// What the client keeps on each side of a channel.
const CAPACITY: usize = 150;

#[derive(Debug, Default)]
pub struct Scrollback {
    /// Oldest first, directly before the messages the channel still shows.
    messages: Vec<Message>,
    /// The channel's first message is the first of these: nothing is older.
    /// Lost once the cache overflows or is dropped.
    at_edge: bool,
}

impl Scrollback {
    /// Keep `cut` (oldest first), the messages just taken off the top of a
    /// channel. `at_edge` says the channel's own start was among them. When
    /// more than the capacity is held, the oldest are let go.
    pub fn keep(&mut self, mut cut: Vec<Message>, at_edge: bool) {
        if self.messages.is_empty() {
            self.at_edge = at_edge;
        }
        if self.messages.len() + cut.len() > CAPACITY {
            self.at_edge = false;
            let room = CAPACITY.saturating_sub(cut.len());
            self.messages.drain(..self.messages.len() - room.min(self.messages.len()));
            cut.drain(..cut.len().saturating_sub(CAPACITY));
        }
        self.messages.extend(cut);
    }

    /// The newest `count` messages, oldest first, for a page of scrollback.
    pub fn take(&mut self, count: usize) -> Vec<Message> {
        self.messages.split_off(self.messages.len().saturating_sub(count))
    }

    /// Empty, and the channel's start went with it: nothing is older.
    pub fn exhausted(&self) -> bool {
        self.messages.is_empty() && self.at_edge
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// The messages no longer line up with what is shown, or can't be shown.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.at_edge = false;
    }

    /// Whether any message is `pred`.
    pub fn any(&self, pred: impl Fn(&Message) -> bool) -> bool {
        self.messages.iter().any(pred)
    }

    /// Change the message `id` if it is kept: the client applies edits,
    /// reactions and votes to its cache too. Says whether it was.
    pub fn update(&mut self, id: Snowflake, change: impl FnOnce(&mut Message)) -> bool {
        self.messages.iter_mut().find(|m| m.id == id).map(change).is_some()
    }

    /// Drop the message `id` if it is kept.
    pub fn remove(&mut self, id: Snowflake) {
        self.messages.retain(|m| m.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ids: std::ops::RangeInclusive<u64>) -> Vec<Message> {
        ids.map(|id| {
            serde_json::from_value(serde_json::json!({
                "id": id.to_string(), "channel_id": "1", "author": {"id": "5", "username": "a"},
                "content": "", "timestamp": "2026-10-26T00:00:00+00:00"
            }))
            .unwrap()
        })
        .collect()
    }

    fn ids(messages: &[Message]) -> Vec<u64> {
        messages.iter().map(|m| m.id.0).collect()
    }

    #[test]
    fn pages_come_newest_first_from_what_was_cut() {
        let mut kept = Scrollback::default();
        kept.keep(run(1..=100), false);
        assert_eq!(ids(&kept.take(50)), (51..=100).collect::<Vec<_>>());
        assert_eq!(ids(&kept.take(80)), (1..=50).collect::<Vec<_>>());
        assert!(kept.is_empty());
    }

    #[test]
    fn only_a_cache_that_reached_the_start_of_the_channel_is_the_end_of_it() {
        let mut kept = Scrollback::default();
        kept.keep(run(1..=3), true);
        assert!(!kept.exhausted());
        kept.take(3);
        assert!(kept.exhausted());
        kept.keep(run(4..=6), false);
        kept.take(3);
        assert!(!kept.exhausted(), "there was more behind it");
    }

    #[test]
    fn the_oldest_past_a_hundred_and_fifty_are_let_go() {
        let mut kept = Scrollback::default();
        kept.keep(run(101..=200), true);
        kept.keep(run(201..=260), false);
        assert_eq!(kept.messages.len(), 150);
        assert_eq!(kept.messages[0].id.0, 111);
        assert!(!kept.at_edge, "the start of the channel is gone");
        kept.keep(run(1..=400), false);
        assert_eq!(ids(&kept.messages), (251..=400).collect::<Vec<_>>());
    }

    #[test]
    fn a_kept_message_follows_what_happens_to_it() {
        let mut kept = Scrollback::default();
        kept.keep(run(1..=3), false);
        assert!(kept.update(Snowflake(2), |m| m.content = "edited".into()));
        assert!(!kept.update(Snowflake(9), |_| unreachable!()));
        kept.remove(Snowflake(3));
        assert_eq!(ids(&kept.take(9)), [1, 2]);
    }
}
