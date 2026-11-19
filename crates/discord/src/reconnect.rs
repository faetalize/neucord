//! Coming back after the gateway drops, as the client's socket does (the
//! class in 868 with `_handleClose`, `_doResumeOrIdentify` and the
//! `Backoff(1000, 60000)` of 158390): how long to wait, whether the session
//! is still worth resuming, how long to wait for HELLO, and the one close
//! code that ends the account.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::backoff::Backoff;

/// How long a connection attempt waits for HELLO, from the moment it starts
/// (`tu`, 868). Past it the attempt counts as a failed close.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// A session is resumed only if the last heartbeat ACK, or the last time a
/// connection opened, is no older than this (`t_`, 868). Discord keeps a
/// dropped session for a few minutes at most.
pub const RESUME_WINDOW: Duration = Duration::from_secs(3 * 60);

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Failures in a row after which the client lets go of the session and
/// identifies afresh (`_handleClose` in 868: more than four).
const FORGET_SESSION_AFTER: u32 = 4;

/// What a connection that is told it has an invalid session may be answered
/// before Neucord gives up on it and reconnects: the client answers without
/// limit, which a faulty answer from either side could turn into a loop of
/// payloads.
pub const MAX_INVALID_SESSIONS: u32 = 5;

/// Connection attempts the storm guard tolerates in [`STORM_WINDOW`] before it
/// holds every further one back by the longest wait.
const STORM_ATTEMPTS: usize = 8;
const STORM_WINDOW: Duration = Duration::from_secs(120);
/// Attempts in [`STORM_WINDOW`] up to which Discord's RECONNECT is answered
/// at once, as the client does; after them it waits like a failure does.
const IMMEDIATE_ATTEMPTS: usize = 3;

/// What to send on a connection that is open: RESUME or IDENTIFY.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Resume,
    Identify,
}

/// `_doResumeOrIdentify` (868): resume when there is a session and the last
/// ACK (or opening) is recent, else identify afresh.
pub fn answer(has_session: bool, last_ack: Instant, now: Instant) -> Answer {
    if has_session && now.saturating_duration_since(last_ack) <= RESUME_WINDOW {
        Answer::Resume
    } else {
        Answer::Identify
    }
}

/// `_handleClose` (868) ends the account on 4004, authentication failed, and
/// on no other code: every other one is reconnected after.
pub fn rejects_token(code: u16) -> bool {
    code == 4004
}

/// What the network coming or going asks of the gateway connection, as the
/// client does on the browser's `online` and `offline` events (225123's
/// callbacks into `networkStateChange` and `expeditedHeartbeat`, 868).
#[derive(Debug, PartialEq, Eq)]
pub enum NetworkAction {
    /// Heartbeat now, and reconnect unless it is acknowledged within this.
    Heartbeat(Duration),
    /// Connect now, waiting no longer and starting the backoff over
    /// (`resetBackoff`).
    ReconnectNow,
    Nothing,
}

/// The network went `online` or offline while the connection was
/// `connected` or not: online checks a live connection within 5 s and ends
/// the wait of one that is down; offline checks a live one within 15 s and
/// leaves one that is down to its backoff.
pub fn on_network(online: bool, connected: bool) -> NetworkAction {
    match (online, connected) {
        (true, true) => NetworkAction::Heartbeat(Duration::from_secs(5)),
        (false, true) => NetworkAction::Heartbeat(Duration::from_secs(15)),
        (true, false) => NetworkAction::ReconnectNow,
        (false, false) => NetworkAction::Nothing,
    }
}

/// The waits between connection attempts, from the client's `gatewayBackoff`,
/// with a guard the client doesn't have: a faulty loop (a connection that
/// reaches READY and drops again at once, which resets the backoff each
/// time) can never connect more than [`STORM_ATTEMPTS`] times in
/// [`STORM_WINDOW`] before every further wait is the longest.
#[derive(Debug)]
pub struct Reconnect {
    backoff: Backoff,
    attempts: VecDeque<Instant>,
}

impl Default for Reconnect {
    fn default() -> Self {
        Reconnect { backoff: Backoff::new(BACKOFF_MIN, BACKOFF_MAX), attempts: VecDeque::new() }
    }
}

impl Reconnect {
    /// READY, RESUMED or READY_SUPPLEMENTAL arrived: the next failure starts over.
    pub fn succeed(&mut self) {
        self.backoff.succeed();
    }

    /// A connection attempt starts now.
    pub fn attempt(&mut self, now: Instant) {
        self.recent(now);
        self.attempts.push_back(now);
    }

    /// The connection failed or closed: how long to wait. `random` is in
    /// `0.0..1.0`.
    pub fn fail(&mut self, random: f64, now: Instant) -> Duration {
        let wait = self.backoff.fail(random);
        if self.recent(now) >= STORM_ATTEMPTS { wait.max(BACKOFF_MAX) } else { wait }
    }

    /// Discord asked for a reconnect (op 7): the client connects at once and
    /// counts no failure. Past a few such attempts in a short while, it waits
    /// as for a failure.
    pub fn requested(&mut self, random: f64, now: Instant) -> Duration {
        if self.recent(now) < IMMEDIATE_ATTEMPTS { Duration::ZERO } else { self.fail(random, now) }
    }

    /// The network came back while waiting to reconnect (`resetBackoff`):
    /// whether to connect now, which starts the backoff over. Not while the
    /// storm guard holds: then the wait already running stands.
    pub fn reconnect_now(&mut self, now: Instant) -> bool {
        if self.recent(now) >= STORM_ATTEMPTS {
            return false;
        }
        self.backoff.succeed();
        true
    }

    /// After this many failures the client forgets the session and identifies
    /// afresh, whatever the last ACK says.
    pub fn forgets_session(&self) -> bool {
        self.backoff.fails() > FORGET_SESSION_AFTER
    }

    /// Attempts in the storm window, dropping the older ones.
    fn recent(&mut self, now: Instant) -> usize {
        while self.attempts.front().is_some_and(|at| now.saturating_duration_since(*at) > STORM_WINDOW) {
            self.attempts.pop_front();
        }
        self.attempts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_session_is_resumed_only_while_its_last_ack_is_recent() {
        let now = Instant::now();
        let ago = |d: Duration| now - d;
        assert_eq!(answer(true, ago(secs(10)), now), Answer::Resume);
        assert_eq!(answer(true, ago(RESUME_WINDOW), now), Answer::Resume, "three minutes exactly still resumes");
        assert_eq!(answer(true, ago(RESUME_WINDOW + Duration::from_millis(1)), now), Answer::Identify);
        assert_eq!(answer(true, ago(secs(3600)), now), Answer::Identify);
        assert_eq!(answer(false, ago(secs(1)), now), Answer::Identify, "no session: nothing to resume");
    }

    #[test]
    fn only_authentication_failing_ends_the_account() {
        assert!(rejects_token(4004));
        for code in [1000, 1001, 1006, 4000, 4003, 4007, 4009, 4010, 4011, 4012, 4013, 4014] {
            assert!(!rejects_token(code), "{code}");
        }
    }

    #[test]
    fn waits_are_the_clients_one_to_sixty_seconds_with_jitter() {
        let now = Instant::now();
        let mut r = Reconnect::default();
        assert_eq!(r.fail(0.0, now), secs(1), "no jitter: the minimum");
        assert_eq!(r.fail(0.5, now), secs(2));
        assert_eq!(r.fail(0.5, now), secs(4));
        let mut r = Reconnect::default();
        assert_eq!(r.fail(0.99, now), Duration::from_millis(2980), "the first wait is at most three seconds");
        for _ in 0..12 {
            r.fail(0.9, now);
        }
        assert_eq!(r.fail(0.9, now), secs(60), "and never more than a minute");
    }

    #[test]
    fn a_success_starts_the_waits_and_the_count_over() {
        let now = Instant::now();
        let mut r = Reconnect::default();
        for _ in 0..3 {
            r.fail(0.5, now);
        }
        r.succeed();
        assert_eq!(r.fail(0.5, now), secs(2));
        assert!(!r.forgets_session());
    }

    #[test]
    fn the_session_is_forgotten_from_the_fifth_failure_in_a_row() {
        let now = Instant::now();
        let mut r = Reconnect::default();
        for n in 1..=4 {
            r.fail(0.0, now);
            assert!(!r.forgets_session(), "failure {n}");
        }
        r.fail(0.0, now);
        assert!(r.forgets_session(), "more than four");
        r.succeed();
        assert!(!r.forgets_session());
    }

    #[test]
    fn the_network_coming_and_going_as_the_client_answers_it() {
        assert_eq!(on_network(true, true), NetworkAction::Heartbeat(secs(5)));
        assert_eq!(on_network(false, true), NetworkAction::Heartbeat(secs(15)));
        assert_eq!(on_network(true, false), NetworkAction::ReconnectNow);
        assert_eq!(on_network(false, false), NetworkAction::Nothing);
    }

    #[test]
    fn the_network_back_ends_the_wait_and_starts_the_backoff_over() {
        let (mut r, now) = (Reconnect::default(), Instant::now());
        r.attempt(now);
        for _ in 0..3 {
            r.fail(0.99, now);
        }
        assert!(r.reconnect_now(now));
        assert!(r.fail(0.0, now) <= secs(1), "the backoff starts over");
    }

    #[test]
    fn the_network_back_is_no_way_round_the_storm_guard() {
        let (mut r, now) = (Reconnect::default(), Instant::now());
        for _ in 0..STORM_ATTEMPTS {
            r.attempt(now);
        }
        assert!(!r.reconnect_now(now), "the wait already running stands");
    }

    #[test]
    fn a_requested_reconnect_comes_at_once_and_counts_no_failure() {
        let now = Instant::now();
        let mut r = Reconnect::default();
        assert_eq!(r.requested(0.5, now), Duration::ZERO);
        assert_eq!(r.fail(0.0, now), secs(1), "the backoff is untouched");
        assert!(!r.forgets_session());
    }

    #[test]
    fn a_requested_reconnect_over_and_over_waits_as_a_failure_does() {
        let now = Instant::now();
        let mut r = Reconnect::default();
        for _ in 0..IMMEDIATE_ATTEMPTS {
            assert_eq!(r.requested(0.0, now), Duration::ZERO);
            r.attempt(now);
        }
        assert_eq!(r.requested(0.0, now), secs(1), "the fourth in a short while");
    }

    #[test]
    fn a_loop_of_quick_connections_is_held_back_to_the_longest_wait() {
        // Each connection reaches READY, which starts the backoff over, and
        // drops at once: without the guard every wait would be a second or so.
        let start = Instant::now();
        let mut r = Reconnect::default();
        let mut waits = Vec::new();
        for n in 0..STORM_ATTEMPTS as u64 + 2 {
            let now = start + secs(n);
            r.attempt(now);
            let wait = r.fail(0.0, now);
            r.succeed();
            waits.push(wait);
        }
        assert!(waits[..STORM_ATTEMPTS - 1].iter().all(|w| *w == secs(1)), "{waits:?}");
        assert!(waits[STORM_ATTEMPTS - 1..].iter().all(|w| *w == secs(60)), "{waits:?}");
    }

    #[test]
    fn the_storm_guard_forgets_attempts_after_its_window() {
        let start = Instant::now();
        let mut r = Reconnect::default();
        for _ in 0..STORM_ATTEMPTS {
            r.attempt(start);
        }
        assert_eq!(r.fail(0.0, start), secs(60));
        r.succeed();
        let later = start + STORM_WINDOW + secs(1);
        assert_eq!(r.fail(0.0, later), secs(1), "a quiet two minutes clears it");
    }
}
