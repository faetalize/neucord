//! Which channels' loaded messages can be shown again without asking
//! Discord: the client's rules, since a request for messages it already holds
//! is one it never makes (#90).
//!
//! The client keeps a `ready` and a `cached` flag on every channel's messages
//! (`ChannelMessages`, 755364). It fetches the newest page when a channel
//! opens unless the channel is `ready` and not `cached`, and trusts the
//! gateway to keep what it holds current. Neucord's `fresh` is both flags:
//! set when a page arrives that no connection change crossed, cleared for
//! every channel by READY (`CONNECTION_OPEN`) and for one that we lose View
//! Channel on. A resume clears nothing: Discord replays what was missed.

use std::collections::HashSet;

use crate::Snowflake;

/// The gateway connection as `GatewayConnectionStore` keeps it: whether a
/// session is established, and `lastTimeConnectedChanged`, which READY and
/// every close move. A counter does for a timestamp, as nothing else reads it.
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
}

#[derive(Debug, Default)]
pub struct Held {
    link: Link,
    fresh: HashSet<Snowflake>,
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

    /// The connection closed, whether or not it can resume.
    pub fn closed(&mut self) {
        self.link.established = false;
        self.link.changed += 1;
    }

    /// A request is about to be sent.
    pub fn started(&self) -> Started {
        Started { established: self.link.established, changed: self.link.changed }
    }

    /// The newest page of `channel` arrived for a request that `started`. A
    /// page asked for without a session, or across a connection change, may
    /// have missed what came while the connection was down: `isStale` in
    /// `fetchMessages` (138399).
    pub fn loaded(&mut self, channel: Snowflake, started: Started) {
        if started.established && started.changed == self.link.changed {
            self.fresh.insert(channel);
        } else {
            self.fresh.remove(&channel);
        }
    }

    /// An older or newer page arrived. A stale one takes the trust in the
    /// channel away, as it does in the client; a current one gives none to a
    /// channel whose newest page hasn't been asked for.
    pub fn paged(&mut self, channel: Snowflake, started: Started) {
        if self.fresh.contains(&channel) {
            self.loaded(channel, started);
        }
    }

    /// We may no longer see the channels that `lost` picks out. Nothing tells
    /// us what they miss meanwhile, so the client marks them `cached` (`Y` in
    /// 596344).
    pub fn lost_sight(&mut self, mut lost: impl FnMut(Snowflake) -> bool) {
        self.fresh.retain(|&channel| !lost(channel));
    }

    /// The channel's messages are gone.
    pub fn forget(&mut self, channel: Snowflake) {
        self.fresh.remove(&channel);
    }

    /// Whether opening `channel` asks for its newest page (`M` in 123363).
    /// A load in flight is never doubled, which the client's system DM case
    /// would.
    pub fn fetch_on_open(&self, channel: Snowflake, opening: Opening) -> bool {
        !opening.loading && (opening.system_dm_unread || !self.fresh.contains(&channel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Snowflake = Snowflake(1);
    const B: Snowflake = Snowflake(2);
    const OPEN: Opening = Opening { loading: false, system_dm_unread: false };

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
    fn a_page_that_crossed_a_connection_change_is_cached() {
        let mut held = Held::default();
        held.opened();
        let started = held.started();
        held.closed();
        held.resumed();
        held.loaded(A, started);
        assert!(held.fetch_on_open(A, OPEN), "it may have missed what came while the connection was down");
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
        held.closed();
        held.resumed();
        held.loaded(A, stale);
        let started = held.started();
        held.loaded(A, started);
        assert!(!held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_stale_older_page_takes_the_trust_away() {
        let mut held = session(&[A]);
        let started = held.started();
        held.closed();
        held.resumed();
        held.paged(A, started);
        assert!(held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_current_older_page_trusts_nothing_new() {
        let mut held = session(&[]);
        let started = held.started();
        held.paged(A, started);
        assert!(held.fetch_on_open(A, OPEN), "its newest page was never asked for");
        held.loaded(A, started);
        held.paged(A, started);
        assert!(!held.fetch_on_open(A, OPEN));
    }

    #[test]
    fn a_channel_we_lose_sight_of_is_cached() {
        let mut held = session(&[A, B]);
        held.lost_sight(|channel| channel == A);
        assert!(held.fetch_on_open(A, OPEN), "nothing told us what it missed");
        assert!(!held.fetch_on_open(B, OPEN));
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
}
