//! What the gateway is asked to send for the servers we look at: the
//! official client's `GuildSubscriptionsStore` (661670).
//!
//! A user account gets no server's events until it subscribes, through op 37
//! (`GUILD_SUBSCRIPTIONS_BULK`; the client never sends op 14). Opening a
//! channel of a server subscribes to its `typing`, `activities` and
//! `threads`, and with the member list showing, to that channel's `channels`
//! ranges: which rows of the list we want, 100 at a time (see
//! [`view_ranges`]). Nothing is ever unsubscribed from while connected but
//! on idle, which Neucord doesn't know. [`Subscriptions`] keeps the same
//! account, so Neucord sends what the client would, when it would.
//!
//! Changes queue up and go out together on [`Subscriptions::flush`], as the
//! client's zero-delay timer batches them. A reconnect sends everything
//! subscribed again ([`Subscriptions::session`]), and a closed connection
//! drops all but `threads` for the servers we aren't looking at
//! ([`Subscriptions::closed`]).

use serde::{Serialize, Serializer, ser::SerializeMap};

use crate::Snowflake;

/// Rows of a member list, as index ranges, both ends included.
pub type Ranges = Vec<(u32, u32)>;

/// How many channels' ranges a server keeps (`36124`).
const CHANNELS: usize = 5;

/// A message holds at most this many characters of subscriptions (`868`).
const CHUNK: usize = 15360;

/// The range asked for before the list has measured itself (`36124`).
pub fn first_page() -> Ranges {
    vec![(0, 99)]
}

/// The ranges the client asks for as a member list scrolls (`361610`): the
/// pages of 100 rows the view touches, with half a view of rows on either
/// side, and always the first page. `y` is how far the list is scrolled and
/// `height` how tall it is, in the same unit as `row_height`.
pub fn view_ranges(y: f64, height: f64, row_height: f64) -> Ranges {
    let row = |px: f64, offset: i64| ((px / row_height).ceil() as i64 + offset).max(0);
    let buffer = row(0.5 * height, 0);
    let mut start = row(y, -buffer);
    let end = row(y + height, buffer);
    let mut ranges = Vec::new();
    let mut page = |from: i64| {
        ranges.push((from as u32, from as u32 + 99));
        from + 100
    };
    if start > 0 {
        start = start.max(page(0));
    }
    start = 100 * (start / 100);
    while start <= end {
        start = page(start);
    }
    ranges
}

/// A server's subscription as the client keeps it (`U.get`).
#[derive(Debug, Default)]
struct Server {
    typing: bool,
    activities: bool,
    threads: bool,
    /// Least recently used first.
    channels: Vec<(Snowflake, Ranges)>,
}

impl Server {
    /// Remember `ranges` for `channel`, whose entry counts as used now.
    /// With `keep`, an entry already there stays as it was. Whether the
    /// entries changed.
    fn subscribe(&mut self, channel: Snowflake, ranges: Ranges, keep: bool) -> bool {
        let known = self.channels.iter().position(|(c, _)| *c == channel).map(|i| self.channels.remove(i));
        let unchanged = known.as_ref().is_some_and(|(_, old)| keep || *old == ranges);
        let ranges = match known {
            Some(entry) if keep => entry.1,
            _ => ranges,
        };
        self.channels.push((channel, ranges));
        if self.channels.len() > CHANNELS {
            self.channels.remove(0);
        }
        !unchanged
    }

    /// Everything the server is subscribed to, as the client sends it when
    /// it sends the server again (`get`).
    fn full(&self) -> Entry {
        Entry::Full(Full {
            typing: self.typing,
            threads: self.threads,
            activities: self.activities,
            members: [],
            member_updates: false,
            channels: Channels(self.channels.clone()),
            thread_member_lists: [],
        })
    }
}

/// What changed since the last flush.
#[derive(Debug, Default)]
struct Pending {
    typing: bool,
    activities: bool,
    threads: bool,
    channels: Option<Vec<(Snowflake, Ranges)>>,
}

/// The ranges of each channel, as the map the client sends.
#[derive(Clone, Debug)]
struct Channels(Vec<(Snowflake, Ranges)>);

impl Serialize for Channels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        // Most recently used first, as the client's cache lists them.
        for (channel, ranges) in self.0.iter().rev() {
            map.serialize_entry(channel, ranges)?;
        }
        map.end()
    }
}

/// What changed, and only that.
#[derive(Debug, Serialize)]
struct Partial {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    typing: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    activities: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    threads: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<Channels>,
}

/// All of it, in the client's field order. Neucord has no member, thread
/// member list or member update subscriptions: those fields stay empty.
#[derive(Debug, Serialize)]
struct Full {
    typing: bool,
    threads: bool,
    activities: bool,
    members: [Snowflake; 0],
    member_updates: bool,
    channels: Channels,
    thread_member_lists: [Snowflake; 0],
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Entry {
    Partial(Partial),
    Full(Full),
}

/// Op 37's payload.
#[derive(Serialize)]
struct Bulk<'a> {
    subscriptions: Servers<'a>,
}

struct Servers<'a>(&'a [(Snowflake, Entry)]);

impl Serialize for Servers<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (guild, entry) in self.0 {
            map.serialize_entry(guild, entry)?;
        }
        map.end()
    }
}

/// `updates` as op 37 messages, split where one would pass the client's size
/// limit (`updateGuildSubscriptions`, 868).
fn bulk_messages(updates: Vec<(Snowflake, Entry)>) -> Vec<String> {
    let mut messages = Vec::new();
    let (mut chunk, mut size) = (Vec::new(), 0);
    let mut send = |chunk: &mut Vec<(Snowflake, Entry)>| {
        let d = Bulk { subscriptions: Servers(chunk) };
        messages.push(crate::gateway::request_text(37, &d));
        chunk.clear();
    };
    for (guild, entry) in updates {
        let length = serde_json::to_string(&(guild, &entry)).unwrap_or_default().len();
        if size + length > CHUNK {
            send(&mut chunk);
            size = 0;
        }
        chunk.push((guild, entry));
        size += length;
    }
    if size > 0 {
        send(&mut chunk);
    }
    messages
}

#[derive(Debug, Default)]
pub struct Subscriptions {
    guilds: std::collections::HashMap<Snowflake, Server>,
    /// Servers sent at least once, in the order they were first sent.
    subscribed: Vec<Snowflake>,
    pending: Vec<(Snowflake, Pending)>,
}

impl Subscriptions {
    fn pending(&mut self, guild: Snowflake) -> &mut Pending {
        let i = match self.pending.iter().position(|(g, _)| *g == guild) {
            Some(i) => i,
            None => {
                self.pending.push((guild, Pending::default()));
                self.pending.len() - 1
            }
        };
        &mut self.pending[i].1
    }

    /// A channel of `guild` opened (`CHANNEL_SELECT`), and `list` is the
    /// channel whose member list is showing: subscribe to the server's
    /// events, and to the first rows of the list unless we ask for it
    /// already.
    pub fn select(&mut self, guild: Snowflake, list: Option<Snowflake>) {
        let state = self.guilds.entry(guild).or_default();
        let (typing, activities, threads) = (!state.typing, !state.activities, !state.threads);
        (state.typing, state.activities, state.threads) = (true, true, true);
        if typing || activities || threads {
            let pending = self.pending(guild);
            pending.typing |= typing;
            pending.activities |= activities;
            pending.threads |= threads;
        }
        if let Some(channel) = list {
            self.channel(guild, channel, first_page(), true);
        }
    }

    /// Whether we're subscribed to `guild`'s threads (`isSubscribedToThreads`).
    pub fn threads(&self, guild: Snowflake) -> bool {
        self.guilds.get(&guild).is_some_and(|s| s.threads)
    }

    /// The member list of `channel` shows these rows now (`GUILD_SUBSCRIPTIONS_CHANNEL`).
    pub fn ranges(&mut self, guild: Snowflake, channel: Snowflake, ranges: Ranges) {
        self.channel(guild, channel, ranges, false);
    }

    fn channel(&mut self, guild: Snowflake, channel: Snowflake, ranges: Ranges, keep: bool) {
        let state = self.guilds.entry(guild).or_default();
        if state.subscribe(channel, ranges, keep) {
            let channels = state.channels.clone();
            self.pending(guild).channels = Some(channels);
        }
    }

    /// The changes since the last flush, as the messages to send. Servers we
    /// don't have any more (`known` says which) are left out.
    pub fn flush(&mut self, known: impl Fn(Snowflake) -> bool) -> Vec<String> {
        let mut updates = Vec::new();
        for (guild, pending) in std::mem::take(&mut self.pending) {
            if !self.subscribed.contains(&guild) {
                self.subscribed.push(guild);
            }
            if known(guild) {
                let Pending { typing, activities, threads, channels } = pending;
                let channels = channels.map(Channels);
                updates.push((guild, Entry::Partial(Partial { typing, activities, threads, channels })));
            }
        }
        bulk_messages(updates)
    }

    /// The connection dropped, with `open` the server we're in. The client
    /// keeps what it asked of that one, and of the others only `threads`.
    pub fn closed(&mut self, open: Option<Snowflake>) {
        self.clear_others(open, false);
    }

    /// READY (`fresh`) or RESUMED: send what we're subscribed to again,
    /// except in a new session, which starts over with the server we're in.
    /// `open` is that server, and `list` its member list's channel; `known`
    /// says which servers we still have.
    pub fn session(
        &mut self,
        fresh: bool,
        open: Option<(Snowflake, Option<Snowflake>)>,
        known: impl Fn(Snowflake) -> bool,
    ) -> Vec<String> {
        if fresh {
            self.clear_others(open.map(|(guild, _)| guild), true);
        }
        if let Some((guild, list)) = open {
            self.select(guild, list);
        }
        let mut resend = Vec::new();
        for guild in self.subscribed.clone() {
            if known(guild) {
                if let Some(state) = self.guilds.get(&guild) {
                    resend.push((guild, state.full()));
                }
            } else {
                self.clear(guild, true);
            }
        }
        let mut messages = bulk_messages(resend);
        messages.extend(self.flush(known));
        messages
    }

    /// Forget every server but `open`'s (`w`), all of it with `all`, else
    /// all but `threads`.
    fn clear_others(&mut self, open: Option<Snowflake>, all: bool) {
        for guild in self.subscribed.clone() {
            if Some(guild) != open {
                self.clear(guild, all);
            }
        }
    }

    fn clear(&mut self, guild: Snowflake, all: bool) {
        let keeps_threads = !all && self.guilds.get(&guild).is_some_and(|g| g.threads);
        if !keeps_threads {
            self.subscribed.retain(|g| *g != guild);
        }
        self.pending.retain(|(g, _)| *g != guild);
        if all {
            self.guilds.remove(&guild);
        } else if let Some(state) = self.guilds.get_mut(&guild) {
            *state = Server { threads: state.threads, ..Server::default() };
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const A: Snowflake = Snowflake(1);
    const B: Snowflake = Snowflake(2);
    const C: Snowflake = Snowflake(9);

    fn all(_: Snowflake) -> bool {
        true
    }

    /// The `d` of each message, which must all be op 37.
    fn sent(messages: Vec<String>) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let m: Value = serde_json::from_str(m).unwrap();
                assert_eq!(m["op"], 37);
                m["d"]["subscriptions"].clone()
            })
            .collect()
    }

    #[test]
    fn opening_a_channel_subscribes_to_the_server() {
        let mut subs = Subscriptions::default();
        subs.select(A, None);
        let messages = subs.flush(all);
        assert_eq!(
            messages,
            [r#"{"op":37,"d":{"subscriptions":{"1":{"typing":true,"activities":true,"threads":true}}}}"#]
        );
        subs.select(A, None);
        assert!(subs.flush(all).is_empty(), "nothing new to say");
        subs.select(B, Some(C));
        assert_eq!(
            sent(subs.flush(all)),
            [json!({"2": {"typing": true, "activities": true, "threads": true, "channels": {"9": [[0, 99]]}}})]
        );
    }

    #[test]
    fn a_channel_already_asked_for_isnt_asked_again() {
        let mut subs = Subscriptions::default();
        subs.select(A, Some(C));
        subs.flush(all);
        subs.ranges(A, C, vec![(0, 99), (100, 199)]);
        subs.flush(all);
        subs.select(A, Some(C));
        assert!(subs.flush(all).is_empty(), "opening it again keeps the ranges it has");
        subs.ranges(A, C, vec![(0, 99), (100, 199)]);
        assert!(subs.flush(all).is_empty(), "the same ranges");
        subs.ranges(A, C, first_page());
        assert_eq!(sent(subs.flush(all)), [json!({"1": {"channels": {"9": [[0, 99]]}}})]);
    }

    #[test]
    fn five_channels_at_most() {
        let mut subs = Subscriptions::default();
        for channel in 10..16 {
            subs.ranges(A, Snowflake(channel), first_page());
        }
        let sent = sent(subs.flush(all));
        let channels = sent[0]["1"]["channels"].as_object().unwrap();
        assert_eq!(channels.len(), 5);
        assert!(!channels.contains_key("10"), "the one used longest ago goes");
    }

    #[test]
    fn a_reconnect_sends_it_all_again() {
        let mut subs = Subscriptions::default();
        subs.select(A, Some(C));
        subs.select(B, None);
        subs.flush(all);
        // Dropped while in B: A keeps `threads` only.
        subs.closed(Some(B));
        let resumed = sent(subs.session(false, Some((B, None)), all));
        assert_eq!(
            resumed,
            [json!({
                "1": {"typing": false, "threads": true, "activities": false, "members": [],
                      "member_updates": false, "channels": {}, "thread_member_lists": []},
                "2": {"typing": true, "threads": true, "activities": true, "members": [],
                      "member_updates": false, "channels": {}, "thread_member_lists": []},
            })]
        );
        // A new session starts over with B.
        let fresh = sent(subs.session(true, Some((B, Some(C))), all));
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0].as_object().unwrap().keys().collect::<Vec<_>>(), ["2"], "A is forgotten");
        assert_eq!(fresh[1], json!({"2": {"channels": {"9": [[0, 99]]}}}));
    }

    #[test]
    fn servers_we_no_longer_have_are_dropped() {
        let mut subs = Subscriptions::default();
        subs.select(A, None);
        subs.select(B, None);
        let messages = subs.flush(|g| g == B);
        assert_eq!(sent(messages).len(), 1);
        let resumed = sent(subs.session(false, None, |g| g == B));
        assert_eq!(resumed[0].as_object().unwrap().keys().collect::<Vec<_>>(), ["2"]);
    }

    #[test]
    fn one_message_holds_so_much() {
        let mut subs = Subscriptions::default();
        for guild in 1..=400u64 {
            subs.select(Snowflake(guild * 1_000_000_000_000_000), None);
        }
        let messages = subs.flush(all);
        assert!(messages.len() > 1, "split");
        assert!(messages.iter().all(|m| m.len() < CHUNK + 100));
        let servers: usize = sent(messages).iter().map(|d| d.as_object().unwrap().len()).sum();
        assert_eq!(servers, 400);
    }

    #[test]
    fn the_rows_a_view_asks_for() {
        // A list 20 rows high at the top, with half a view of rows around it.
        assert_eq!(view_ranges(0.0, 800.0, 40.0), [(0, 99)]);
        assert_eq!(view_ranges(0.0, 0.0, 40.0), [(0, 99)]);
        // 80 rows down: the 30 or so above and below it reach the second page.
        assert_eq!(view_ranges(80.0 * 40.0, 800.0, 40.0), [(0, 99), (100, 199)]);
        // Far down, only the pages it touches, and the first.
        assert_eq!(view_ranges(1000.0 * 40.0, 800.0, 40.0), [(0, 99), (900, 999), (1000, 1099)]);
        // Sizes that don't divide, as the client's own function gives them (361610).
        assert_eq!(view_ranges(123.0, 701.0, 37.0), [(0, 99)]);
        assert_eq!(view_ranges(5000.0, 300.0, 42.0), [(0, 99), (100, 199)]);
        // Near the end of the first page, the buffer reaches the next.
        assert_eq!(view_ranges(90.0 * 40.0, 800.0, 40.0), [(0, 99), (100, 199)]);
    }
}
