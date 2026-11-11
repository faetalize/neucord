//! Which channels' loaded messages can be shown again without asking
//! Discord: the client's rules, since a request for messages it already holds
//! is one it never makes (#90).
//!
//! The client keeps a `ready` and a `cached` flag on every channel's messages
//! (`ChannelMessages`, 755364). It fetches the newest page when a channel
//! opens unless the channel is `ready` and not `cached`, and trusts the
//! gateway to keep what it holds current. Neucord's `fresh` is both flags:
//! set when a page arrives that no new session crossed, cleared for every
//! channel by READY (`CONNECTION_OPEN`). A resume clears nothing: Discord
//! replays what was missed.
//!
//! Only DMs and group DMs are held. The client is subscribed to every server
//! it has held channels of (`threads` stays on even when it goes idle,
//! 661670), and Neucord subscribes the same way ([`crate::subscriptions`]),
//! so the client's rule could cover servers too, with the channels we lose
//! View Channel on marked stale (`Y` in 596344). A DM has no subscription on
//! either side.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::Snowflake;

/// The gateway connection as `GatewayConnectionStore` keeps it: whether a
/// session is established, and `lastTimeConnectedChanged`, which READY moves,
/// and a reset (`CONNECTION_CLOSED`: a rejected token, or the fifth failed
/// reconnect in a row), which ends the session, so the READY after it moves
/// it anyway. An ordinary close is `CONNECTION_INTERRUPTED`, which doesn't
/// move it. A counter does for a timestamp, as nothing else reads it.
#[derive(Debug, Default)]
struct Link {
    established: bool,
    changed: u64,
}

/// What a request saw of the connection when it was sent.
#[derive(Clone, Copy, Debug)]
pub struct Started {
    established: bool,
    changed: u64,
}

/// What opening a channel depends on besides the messages held for it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opening {
    /// A request for the channel's newest page is already in flight.
    pub loading: bool,
    /// The channel is the DM with Discord's own account, and has unread
    /// messages.
    pub system_dm_unread: bool,
    /// A DM or group DM, the only channels held.
    pub private: bool,
}

/// How soon after a page a stale one is asked for again (`b` in 123363).
const REFETCH_GAP: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub struct Held {
    link: Link,
    fresh: HashSet<Snowflake>,
    /// When each channel last had a page arrive that was let through.
    landed: HashMap<Snowflake, Instant>,
}

impl Held {
    /// READY arrived: a new session, which knows nothing of what the old one
    /// held.
    pub fn opened(&mut self) {
        self.link.established = true;
        self.link.changed += 1;
        self.fresh.clear();
    }

    /// RESUMED arrived.
    pub fn resumed(&mut self) {
        self.link.established = true;
    }

    /// The connection closed. The client counts the stretch between
    /// reconnecting and RESUMED as established; we can't see it, so a request
    /// sent then counts as made without a session.
    pub fn closed(&mut self) {
        self.link.established = false;
    }

    /// A request is about to be sent.
    pub fn started(&self) -> Started {
        Started { established: self.link.established, changed: self.link.changed }
    }

    /// A page of `channel` arrived for a request that `started`. One asked
    /// for without a session, or across a new one, may have missed what came
    /// meanwhile: `isStale` in `fetchMessages` (138399). Any page does this in
    /// the client (`loadComplete`, 755364), scrollback too.
    pub fn loaded(&mut self, channel: Snowflake, started: Started) -> bool {
        let stale = self.is_stale(started);
        if stale {
            self.fresh.remove(&channel);
        } else {
            self.fresh.insert(channel);
        }
        stale
    }

    fn is_stale(&self, started: Started) -> bool {
        !started.established || started.changed != self.link.changed
    }

    /// Whether a page that just arrived for `channel`, `stale` or not, is
    /// asked for again at once (`$` in 123363): only the open channel, only
    /// while connected, and not within ten seconds of the last page that got
    /// this far, which the client times for every page, stale or not.
    pub fn refetch(&mut self, channel: Snowflake, stale: bool, open: bool, now: Instant) -> bool {
        if self.landed.get(&channel).is_some_and(|&last| now.saturating_duration_since(last) < REFETCH_GAP) {
            return false;
        }
        self.landed.insert(channel, now);
        stale && open && self.link.established
    }

    /// Whether a message that arrived live goes into the channel's held
    /// messages. Not into one that missed events: the client skips
    /// `MESSAGE_CREATE` for a channel that isn't `ready` (596344), and kept
    /// apart from the messages before it, the one would hide the gap between
    /// them. The open channel is being asked for, and merges.
    pub fn live_messages(&self, channel: Snowflake, open: bool) -> bool {
        open || self.fresh.contains(&channel)
    }

    /// The channel's messages are gone.
    pub fn forget(&mut self, channel: Snowflake) {
        self.fresh.remove(&channel);
    }

    /// Whether opening `channel` asks for its newest page (`P` in 123363).
    /// A load in flight is never doubled, which the client's system DM case
    /// would.
    pub fn fetch_on_open(&self, channel: Snowflake, opening: Opening) -> bool {
        !opening.loading && (opening.system_dm_unread || !opening.private || !self.fresh.contains(&channel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Snowflake = Snowflake(1);
    const B: Snowflake = Snowflake(2);
    const OPEN: Opening = Opening { loading: false, system_dm_unread: false, private: true };

    /// A session whose READY arrived, with `channels` loaded in it.
    fn session(channels: &[Snowflake]) -> Held {
        let mut held = Held::default();
        held.opened();
        for &channel in channels {
            let started = held.started();
            held.loaded(channel, started);
        }
        held
    }

    #[test]
    fn a_channel_never_loaded_is_fetched() {
        let mut held = Held::default();
        held.opened();
        assert!(held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_channel_we_hold_is_not_fetched_again() {
        let held = session(&[A]);
        assert!(!held.fetch_on_open(A, OPEN));
        assert!(held.fetch_on_open(B, OPEN), "only the one we hold");
    }

    #[test]
    fn a_new_session_marks_every_channel_not_ready() {
        let mut held = session(&[A, B]);
        held.closed();
        held.opened();
        assert!(held.fetch_on_open(A, OPEN));
        assert!(held.fetch_on_open(B, OPEN));
    }

    #[test]
    fn a_resume_keeps_what_we_hold() {
        // The gateway replays what the channel missed.
        let mut held = session(&[A]);
        held.closed();
        held.resumed();
        assert!(!held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn an_ordinary_close_moves_nothing() {
        // A close that resumes is `CONNECTION_INTERRUPTED`, which the client's
        // clock doesn't see: only READY and a reset do.
        let mut held = Held::default();
        held.opened();
        let started = held.started();
        held.closed();
        held.resumed();
        assert!(!held.loaded(A, started), "the gateway replayed what the page missed");
        assert!(!held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_page_asked_for_while_disconnected_is_cached() {
        let mut held = session(&[]);
        held.closed();
        let started = held.started();
        held.resumed();
        held.loaded(A, started);
        assert!(held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_page_that_crossed_a_new_session_is_cached() {
        let mut held = session(&[]);
        let started = held.started();
        held.opened();
        held.loaded(A, started);
        assert!(held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_later_page_that_is_current_is_trusted_again() {
        let mut held = Held::default();
        held.opened();
        let stale = held.started();
        held.opened();
        held.loaded(A, stale);
        let started = held.started();
        held.loaded(A, started);
        assert!(!held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_forgotten_channel_is_fetched() {
        let mut held = session(&[A]);
        held.forget(A);
        assert!(held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn the_system_dm_with_unread_messages_is_fetched_anyway() {
        let held = session(&[A]);
        assert!(held.fetch_on_open(A, Opening { system_dm_unread: true, ..OPEN }));
    }

    #[test]
    fn a_channel_being_loaded_is_not_asked_for_twice() {
        let held = session(&[]);
        assert!(!held.fetch_on_open(A, Opening { loading: true, ..OPEN }));
    }

    /// A page for `A` that crossed a new session, which is stale.
    fn stale_page(held: &mut Held) -> bool {
        let started = held.started();
        held.opened();
        held.loaded(A, started)
    }

    #[test]
    fn a_stale_page_of_the_open_channel_is_asked_for_again() {
        let mut held = session(&[]);
        let stale = stale_page(&mut held);
        assert!(stale);
        assert!(held.refetch(A, stale, true, Instant::now()));
    }

    #[test]
    fn a_current_page_or_another_channel_is_not() {
        let mut held = session(&[]);
        let started = held.started();
        let current = held.loaded(A, started);
        assert!(!held.refetch(A, current, true, Instant::now()), "nothing missed");
        let stale = stale_page(&mut held);
        assert!(!held.refetch(B, stale, false, Instant::now()), "not the open channel");
    }

    #[test]
    fn nothing_is_asked_for_while_disconnected() {
        let mut held = session(&[]);
        held.closed();
        let started = held.started();
        let stale = held.loaded(A, started);
        assert!(stale && !held.refetch(A, stale, true, Instant::now()));
    }

    #[test]
    fn a_page_within_ten_seconds_of_the_last_is_let_be() {
        // Every page starts the ten seconds, stale or not, so a refetch that
        // lands stale itself doesn't ask again at once.
        let mut held = session(&[]);
        let now = Instant::now();
        let started = held.started();
        let current = held.loaded(A, started);
        assert!(!held.refetch(A, current, true, now));
        let stale = stale_page(&mut held);
        assert!(!held.refetch(A, stale, true, now + Duration::from_secs(9)));
        assert!(held.refetch(A, stale, true, now + Duration::from_secs(19)), "ten seconds after the one before");
        assert!(!held.refetch(A, stale, true, now + Duration::from_secs(20)), "and that one counts too");
    }

    #[test]
    fn only_the_gateways_own_channels_are_held() {
        // A server's channels sit under subscriptions we don't make.
        let held = session(&[A]);
        assert!(!held.fetch_on_open(A, OPEN));
        assert!(held.fetch_on_open(A, Opening { private: false, ..OPEN }));
    }

    #[test]
    fn a_channel_that_missed_events_takes_no_live_message_unless_open() {
        // The client ignores `MESSAGE_CREATE` for a channel that isn't `ready`: a message
        // kept apart from the ones before it would hide the gap in between.
        let mut held = session(&[A, B]);
        held.opened();
        let started = held.started();
        held.loaded(B, started);
        assert!(!held.live_messages(A, false), "not asked for since READY");
        assert!(held.live_messages(A, true), "the open one is being asked for, and merges");
        assert!(held.live_messages(B, false));
    }
}
