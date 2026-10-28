//! Which channels have unread messages, and how many mentions wait in each.
//!
//! Discord keeps, per channel, the last message we acknowledged ("acked") and
//! a mention count. A channel is unread when its newest message is newer than
//! the acked one. Channels with no entry count as read: Discord sends entries
//! only for channels we have opened or been mentioned in, and treating the
//! rest as unread would light up channels we cannot even see.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::Snowflake;

#[derive(Clone, Debug, Default, PartialEq)]
struct Entry {
    /// `None` before we have acked anything here.
    acked: Option<Snowflake>,
    mentions: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ReadStates {
    channels: HashMap<Snowflake, Entry>,
}

impl ReadStates {
    /// From READY's `read_state`: a plain array, or `{entries: [...]}` when the
    /// gateway sends the versioned shape.
    pub fn from_ready(value: &Value) -> Self {
        #[derive(Deserialize)]
        struct Raw {
            id: Snowflake,
            #[serde(default)]
            last_message_id: Option<Value>,
            #[serde(default)]
            mention_count: u32,
        }
        let entries = value.get("entries").unwrap_or(value);
        let channels = entries
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|raw| Raw::deserialize(raw).ok())
            .map(|raw| {
                // Sent as a string id, or 0 / "0" for "never acked".
                let acked = raw
                    .last_message_id
                    .as_ref()
                    .and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string())))
                    .and_then(|s| s.parse().ok())
                    .filter(|&n: &u64| n != 0)
                    .map(Snowflake);
                (raw.id, Entry { acked, mentions: raw.mention_count })
            })
            .collect();
        ReadStates { channels }
    }

    /// Whether `channel`, whose newest message is `last_message`, has anything unread.
    pub fn is_unread(&self, channel: Snowflake, last_message: Option<Snowflake>) -> bool {
        match (self.channels.get(&channel), last_message) {
            (Some(entry), Some(last)) => entry.acked.is_none_or(|acked| last > acked),
            _ => false,
        }
    }

    /// The last message we read in `channel`, if we read any.
    pub fn acked(&self, channel: Snowflake) -> Option<Snowflake> {
        self.channels.get(&channel)?.acked
    }

    pub fn mentions(&self, channel: Snowflake) -> u32 {
        self.channels.get(&channel).map_or(0, |e| e.mentions)
    }

    /// A message arrived that we have not seen. Creates the entry if missing,
    /// so live messages make a channel unread even without prior history.
    pub fn on_message(&mut self, channel: Snowflake, mentioned: bool) {
        let entry = self.channels.entry(channel).or_default();
        if mentioned {
            entry.mentions += 1;
        }
    }

    /// Everything up to `message` is read (we opened the channel, sent a
    /// message, or another client of ours acked it). Never moves backwards.
    pub fn ack(&mut self, channel: Snowflake, message: Snowflake) {
        let entry = self.channels.entry(channel).or_default();
        if entry.acked.is_none_or(|acked| message >= acked) {
            entry.acked = Some(message);
            entry.mentions = 0;
        }
    }
}

/// Whether the message `id` is the first unread one, given the message
/// before it and the last one read: the "new" divider goes above it.
pub fn starts_unread(previous: Option<Snowflake>, id: Snowflake, acked: Snowflake) -> bool {
    id > acked && previous.is_none_or(|previous| previous <= acked)
}

/// Unread messages among `ids` (oldest first), and whether there may be
/// more before them: the oldest loaded is unread too.
pub fn unread_count(ids: &[Snowflake], acked: Snowflake) -> (usize, bool) {
    let count = ids.iter().filter(|&&id| id > acked).count();
    (count, ids.first().is_some_and(|&first| first > acked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn states() -> ReadStates {
        ReadStates::from_ready(&json!([
            {"id": "1", "last_message_id": "100", "mention_count": 2},
            {"id": "2", "last_message_id": 0, "mention_count": 0},
        ]))
    }

    #[test]
    fn unread_compares_against_acked() {
        let s = states();
        assert!(!s.is_unread(Snowflake(1), Some(Snowflake(100))));
        assert!(s.is_unread(Snowflake(1), Some(Snowflake(101))));
        assert!(s.is_unread(Snowflake(2), Some(Snowflake(5))), "never acked");
        assert!(!s.is_unread(Snowflake(3), Some(Snowflake(5))), "no entry counts as read");
        assert!(!s.is_unread(Snowflake(1), None));
        assert_eq!(s.mentions(Snowflake(1)), 2);
    }

    #[test]
    fn where_unread_starts() {
        let acked = Snowflake(10);
        assert!(starts_unread(Some(Snowflake(10)), Snowflake(11), acked));
        assert!(starts_unread(None, Snowflake(11), acked), "the oldest loaded");
        assert!(!starts_unread(Some(Snowflake(11)), Snowflake(12), acked));
        assert!(!starts_unread(Some(Snowflake(9)), Snowflake(10), acked));
        let ids = [9, 10, 11, 12].map(Snowflake);
        assert_eq!(unread_count(&ids, acked), (2, false));
        assert_eq!(unread_count(&ids[2..], acked), (2, true), "there may be more");
        assert_eq!(states().acked(Snowflake(1)), Some(Snowflake(100)));
        assert_eq!(states().acked(Snowflake(2)), None);
    }

    #[test]
    fn versioned_shape() {
        let s = ReadStates::from_ready(&json!({"entries": [{"id": "7", "last_message_id": "9"}], "version": 1}));
        assert!(s.is_unread(Snowflake(7), Some(Snowflake(10))));
    }

    #[test]
    fn live_messages_and_acks() {
        let mut s = states();
        s.on_message(Snowflake(3), true);
        assert!(s.is_unread(Snowflake(3), Some(Snowflake(50))));
        assert_eq!(s.mentions(Snowflake(3)), 1);

        s.ack(Snowflake(3), Snowflake(50));
        assert!(!s.is_unread(Snowflake(3), Some(Snowflake(50))));
        assert_eq!(s.mentions(Snowflake(3)), 0);

        s.ack(Snowflake(1), Snowflake(90));
        assert_eq!(s.mentions(Snowflake(1)), 2, "an older ack changes nothing");
    }
}
