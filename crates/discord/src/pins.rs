//! Pinning a message: what the client says when Discord refuses it
//! (`pinMessage` in 687709) and what it asks first (145530), and the list of
//! a channel's pins, which `ChannelPinsStore` (966833) keeps and the pins
//! popout (945830) shows.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{Message, MessageEdit, Snowflake};

/// How many pins a page holds (`fetchPins` in 687709).
pub const PAGE: u8 = 25;

/// One pinned message, and when it was pinned (Discord's own text).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Pin {
    pub pinned_at: String,
    pub message: Message,
}

/// A page of the answer: `{items, has_more}`.
#[derive(Debug, Deserialize)]
pub struct Page {
    pub items: Vec<Pin>,
    #[serde(default)]
    pub has_more: bool,
}

/// The request's query, in the order `fetchPins` gives it: `before` is left
/// out when there is none.
pub fn query(limit: u8, before: Option<&str>) -> String {
    match before {
        Some(before) => format!("limit={limit}&before={}", glib::uri_escape_string(before, None::<&str>, false)),
        None => format!("limit={limit}"),
    }
}

/// `at` as JavaScript's `toISOString` writes it (the millisecond, and Z),
/// which is what the client sends as `before`.
pub fn iso_millis(at: &str) -> Option<String> {
    let time = glib::DateTime::from_iso8601(at, Some(&glib::TimeZone::utc())).ok()?.to_utc().ok()?;
    Some(format!("{}.{:03}Z", time.format("%Y-%m-%dT%H:%M:%S").ok()?, time.microsecond() / 1000))
}

/// Where a channel's list stands (`ChannelPinsStore`'s `state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Loading,
    Failed,
    /// Loaded, and there are older pins to ask for.
    HasMore,
    Finished,
}

#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub state: State,
    /// Newest pinned first.
    pub items: Vec<Pin>,
}

/// The pins of the channels we looked at, which a new session forgets.
#[derive(Debug, Default)]
pub struct Pins {
    channels: HashMap<Snowflake, List>,
}

impl Pins {
    pub fn get(&self, channel: Snowflake) -> Option<&List> {
        self.channels.get(&channel)
    }

    /// Whether to ask for a page now (`fetchPins`): never one in flight, none
    /// after the last, one that failed again, and the next only from the end
    /// of what we have (`before` is that pin's time).
    pub fn wants(&self, channel: Snowflake, before: Option<&str>) -> bool {
        let Some(list) = self.channels.get(&channel) else { return true };
        match list.state {
            State::Failed => true,
            State::Loading | State::Finished => false,
            State::HasMore => match before {
                None => list.items.is_empty(),
                // The same moment, whichever way it is written.
                Some(before) => list.items.last().is_some_and(|pin| {
                    pin.pinned_at == before || iso_millis(&pin.pinned_at).as_deref() == Some(before)
                }),
            },
        }
    }

    /// A request is sent. `reset` starts the list over.
    pub fn started(&mut self, channel: Snowflake, reset: bool) {
        match self.channels.get_mut(&channel) {
            Some(list) if !reset => list.state = State::Loading,
            _ => {
                self.channels.insert(channel, List { state: State::Loading, items: Vec::new() });
            }
        }
    }

    pub fn loaded(&mut self, channel: Snowflake, page: Page) {
        if let Some(list) = self.channels.get_mut(&channel) {
            list.items.extend(page.items);
            list.state = if page.has_more { State::HasMore } else { State::Finished };
        }
    }

    pub fn failed(&mut self, channel: Snowflake) {
        if let Some(list) = self.channels.get_mut(&channel) {
            list.state = State::Failed;
        }
    }

    /// The `before` for the next page: where the list ends.
    pub fn before(&self, channel: Snowflake) -> Option<String> {
        iso_millis(&self.channels.get(&channel)?.items.last()?.pinned_at)
    }

    /// A message is gone (`MESSAGE_DELETE`).
    pub fn deleted(&mut self, channel: Snowflake, id: Snowflake) {
        if let Some(list) = self.channels.get_mut(&channel) {
            list.items.retain(|pin| pin.message.id != id);
        }
    }

    /// A message changed (`MESSAGE_UPDATE`). One unpinned leaves the list; one
    /// pinned that isn't in it can't be added from what an update says, so
    /// the list is dropped to be asked for again.
    pub fn edited(&mut self, channel: Snowflake, edit: &MessageEdit) {
        let Some(list) = self.channels.get_mut(&channel) else { return };
        let at = list.items.iter().position(|pin| pin.message.id == edit.id);
        match (at, edit.pinned) {
            (Some(at), Some(false)) => {
                list.items.remove(at);
            }
            (Some(at), _) => edit.clone().apply(&mut list.items[at].message),
            (None, Some(true)) => {
                self.channels.remove(&channel);
            }
            (None, _) => {}
        }
    }

    /// The channel is gone, or its list is no good.
    pub fn forget(&mut self, channel: Snowflake) {
        self.channels.remove(&channel);
    }

    pub fn clear(&mut self) {
        self.channels.clear();
    }

    /// Whether a pin holds a link that has expired, which the client asks the
    /// list again for (945830, with `$r` in 679164).
    pub fn expired(&self, channel: Snowflake, now_ms: u64) -> bool {
        self.channels
            .get(&channel)
            .is_some_and(|l| l.items.iter().any(|p| crate::cdn::links_expired(&p.message, now_ms)))
    }
}

impl Page {
    /// From the answer's JSON.
    pub fn from_value(value: Value) -> Option<Page> {
        serde_json::from_value(value).ok()
    }
}

/// How many messages a channel can have pinned (`maxPins` in 687709).
const MAX_PINS: u32 = 250;

/// An alert: its title and its text.
#[derive(Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
}

fn alert(title: &str, body: impl Into<String>) -> Alert {
    Alert { title: title.to_owned(), body: body.into() }
}

/// Why pinning a message was refused, by Discord's error `code`. `private`
/// says the channel is a DM, `channel` its name, and `said` what Discord's
/// own message was, which an unlisted code shows.
pub fn refused(code: Option<u64>, private: bool, channel: &str, said: Option<&str>) -> Alert {
    const UNKNOWN_MESSAGE: u64 = 10008;
    const TOO_MANY_PINS: u64 = 30003;
    const INVALID_ACCESS: u64 = 50001;
    const INVALID_CHANNEL: u64 = 50019;
    const SYSTEM_MESSAGE: u64 = 50021;
    const THREAD_ARCHIVED: u64 = 50083;
    match code {
        None => alert("Error", "An error occurred, please try again."),
        Some(TOO_MANY_PINS) if private => alert(
            "The Pin Broke",
            format!(
                "Discord is unable to pin that message. You may have hit the limit of {MAX_PINS} pins in this channel."
            ),
        ),
        Some(TOO_MANY_PINS) => alert(
            "The Pin Broke",
            format!(
                "Discord is unable to pin that message. You may have hit the limit of {MAX_PINS} pins in channel #{channel}."
            ),
        ),
        Some(INVALID_ACCESS) => alert("No Access", "You do not have permission to pin messages in this channel."),
        Some(INVALID_CHANNEL) => alert("Invalid Channel Type", "This message cannot be pinned in this channel."),
        Some(THREAD_ARCHIVED) => {
            alert("Invalid Thread Archive State", "This message cannot be pinned in an archived thread.")
        }
        Some(SYSTEM_MESSAGE) => alert("Invalid Message Type", "This message is a system message and cannot be pinned."),
        Some(UNKNOWN_MESSAGE) => alert("Unknown Message", "The message you are trying to pin does not exist."),
        Some(_) => alert("The Pin Broke", said.unwrap_or("An error occurred, please try again.")),
    }
}

/// What the client says when unpinning failed, with a way to try again.
pub const UNPIN_FAILED: (&str, &str) =
    ("The Pin Is Stuck!", "Discord failed to unpin that message for some reason. Sorry about that.");

/// The question before pinning, unless Shift skips it (`confirmPin`, 145530):
/// its heading, text and button.
pub fn pin_question(private: bool, channel: &str) -> (&'static str, String, &'static str) {
    let text = if private {
        "Hey, just double checking that you want to pin this message to the current channel for posterity and greatness?"
            .to_owned()
    } else {
        format!(
            "Hey, just double checking that you want to pin this message to #{channel} for posterity and greatness?"
        )
    };
    ("Pin It. Pin It Good.", text, "Oh yeah. Pin it")
}

/// The question before unpinning (`confirmUnpin`): heading, text, button and
/// the hint that Shift skips it.
pub const UNPIN_QUESTION: (&str, &str, &str, &str) = (
    "Unpin Message",
    "You sure you want to remove this pinned message?",
    "Remove it please!",
    "You can hold down Shift when clicking Unpin Message to skip this confirmation.",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_what_the_clients_does() {
        assert_eq!(refused(None, false, "x", None).title, "Error");
        let full = refused(Some(30003), false, "general", None);
        assert_eq!(full.title, "The Pin Broke");
        assert!(full.body.ends_with("250 pins in channel #general."));
        assert!(refused(Some(30003), true, "ignored", None).body.ends_with("250 pins in this channel."));
        assert_eq!(refused(Some(50001), false, "", None).title, "No Access");
        assert_eq!(refused(Some(50083), false, "", None).title, "Invalid Thread Archive State");
        assert_eq!(refused(Some(99999), false, "", Some("Nope.")).body, "Nope.", "Discord's own words");
        assert_eq!(refused(Some(99999), false, "", None).body, "An error occurred, please try again.");
    }

    fn pin(id: u64, at: &str) -> Pin {
        let message = serde_json::json!({"id": id.to_string(), "channel_id": "1", "author": {"id": "5", "username": "a"},
                                         "content": "hi", "timestamp": "2026-10-26T00:00:00+00:00", "pinned": true});
        Pin { pinned_at: at.to_owned(), message: serde_json::from_value(message).unwrap() }
    }

    fn list(pins: &[(u64, &str)], has_more: bool) -> Pins {
        let mut all = Pins::default();
        all.started(Snowflake(1), false);
        all.loaded(Snowflake(1), Page { items: pins.iter().map(|(id, at)| pin(*id, at)).collect(), has_more });
        all
    }

    const C: Snowflake = Snowflake(1);

    #[test]
    fn a_page_is_asked_for_by_the_clients_rule() {
        let mut pins = Pins::default();
        assert!(pins.wants(C, None), "never asked");
        pins.started(C, false);
        assert!(!pins.wants(C, None), "in flight");
        pins.failed(C);
        assert!(pins.wants(C, None), "failed, so again");
        let pins = list(&[(2, "b"), (1, "a")], true);
        assert!(!pins.wants(C, None) && pins.wants(C, Some("a")), "the next from the end");
        assert!(!pins.wants(C, Some("b")), "not from the middle");
        assert!(!list(&[(1, "a")], false).wants(C, Some("a")), "none after the last");
    }

    #[test]
    fn the_next_page_starts_at_the_last_pin_the_way_javascript_writes_a_date() {
        assert_eq!(iso_millis("2026-10-26T12:34:56.789012+00:00").as_deref(), Some("2026-10-26T12:34:56.789Z"));
        assert_eq!(iso_millis("2026-10-26T14:34:56+02:00").as_deref(), Some("2026-10-26T12:34:56.000Z"));
        assert_eq!(query(25, None), "limit=25");
        assert_eq!(query(25, Some("2026-10-26T12:34:56.789Z")), "limit=25&before=2026-10-26T12%3A34%3A56.789Z");
        let pins = list(&[(2, "2026-10-26T12:34:56.789012+00:00")], true);
        let before = pins.before(C);
        assert_eq!(before.as_deref(), Some("2026-10-26T12:34:56.789Z"));
        assert!(pins.wants(C, before.as_deref()), "the page after the last pin is the one asked for");
    }

    #[test]
    fn pages_add_up_and_a_reset_starts_over() {
        let mut pins = list(&[(2, "b")], true);
        pins.started(C, false);
        pins.loaded(C, Page { items: vec![pin(1, "a")], has_more: false });
        assert_eq!(pins.get(C).unwrap().items.len(), 2);
        assert_eq!(pins.get(C).unwrap().state, State::Finished);
        pins.started(C, true);
        assert!(pins.get(C).unwrap().items.is_empty());
    }

    #[test]
    fn events_keep_the_list_current() {
        let edit = |id: u64, pinned: Option<bool>, content: Option<&str>| MessageEdit {
            id: Snowflake(id),
            channel_id: C,
            content: content.map(str::to_owned),
            pinned,
            ..MessageEdit::default()
        };
        let mut pins = list(&[(2, "b"), (1, "a")], false);
        pins.edited(C, &edit(2, None, Some("edited")));
        assert_eq!(pins.get(C).unwrap().items[0].message.content, "edited");
        pins.edited(C, &edit(1, Some(false), None));
        assert_eq!(pins.get(C).unwrap().items.len(), 1, "unpinned");
        pins.deleted(C, Snowflake(2));
        assert!(pins.get(C).unwrap().items.is_empty(), "deleted");
        let mut pins = list(&[(2, "b")], false);
        pins.edited(C, &edit(9, Some(true), None));
        assert!(pins.get(C).is_none(), "one we don't know is pinned: ask again");
    }

    #[test]
    fn the_question_names_the_channel_unless_it_is_a_dm() {
        assert!(pin_question(false, "general").1.contains("#general"));
        assert!(pin_question(true, "general").1.contains("the current channel"));
    }
}
