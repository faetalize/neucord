//! Counting refused requests, to stay clear of Discord's invalid request
//! limit: an IP that gets too many 401, 403 and 429 answers (10,000 in 10
//! minutes, by Discord's docs) is cut off from the API for a while, whatever
//! endpoints and tokens they came from. One loop retrying a bad token or a
//! forbidden call could get there alone and take the whole connection down,
//! so Neucord stops sending well short of it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back refusals count, as Discord's.
const WINDOW: Duration = Duration::from_secs(10 * 60);

/// Refusals in a window that pause sending: a fifth of Discord's limit,
/// since anything else on the same IP, such as a browser tab of the
/// official client, counts toward it too.
pub(crate) const LIMIT: usize = 2_000;

/// Refusals in a window worth a warning in the log.
const WARN_AT: usize = LIMIT / 2;

/// What one more refusal changed.
#[derive(Debug, PartialEq)]
pub(crate) enum Change {
    /// As many refusals as this in the window, half the limit.
    Warn(usize),
    /// Nothing may be sent for this long.
    Pause(Duration),
}

#[derive(Debug, Default)]
pub(crate) struct InvalidRequests {
    /// When each refusal still in the window came, oldest first.
    at: VecDeque<Instant>,
}

impl InvalidRequests {
    /// Whether Discord holds a response with `status` against us; `scope`
    /// is its `X-RateLimit-Scope`. A 429 on a limit many users share isn't.
    pub(crate) fn counts(status: u32, scope: Option<&str>) -> bool {
        match status {
            401 | 403 => true,
            429 => scope != Some("shared"),
            _ => false,
        }
    }

    /// Note a refusal that came at `now`.
    pub(crate) fn record(&mut self, now: Instant) -> Option<Change> {
        self.expire(now);
        self.at.push_back(now);
        match self.at.len() {
            // Every refusal past the limit, from requests already on their
            // way when it was reached, lengthens the pause.
            n if n >= LIMIT => self.paused(now).map(Change::Pause),
            WARN_AT => Some(Change::Warn(WARN_AT)),
            _ => None,
        }
    }

    /// How much longer nothing may be sent: until the window holds fewer
    /// refusals than the limit. `None` if sending may go on.
    pub(crate) fn paused(&mut self, now: Instant) -> Option<Duration> {
        self.expire(now);
        let over = self.at.len().checked_sub(LIMIT)?;
        Some((self.at[over] + WINDOW).saturating_duration_since(now))
    }

    fn expire(&mut self, now: Instant) {
        while self.at.front().is_some_and(|&at| at + WINDOW <= now) {
            self.at.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Change, InvalidRequests, LIMIT, WARN_AT, WINDOW};

    #[test]
    fn what_discord_counts() {
        assert!(InvalidRequests::counts(401, None));
        assert!(InvalidRequests::counts(403, None));
        assert!(InvalidRequests::counts(429, None));
        assert!(InvalidRequests::counts(429, Some("user")));
        assert!(InvalidRequests::counts(429, Some("global")));
        assert!(!InvalidRequests::counts(429, Some("shared")), "a shared limit isn't held against us");
        assert!(!InvalidRequests::counts(404, None), "not in Discord's list");
        assert!(!InvalidRequests::counts(400, None));
        assert!(!InvalidRequests::counts(200, None));
    }

    #[test]
    fn warns_halfway_then_pauses_at_the_limit() {
        let start = Instant::now();
        let mut invalid = InvalidRequests::default();
        let changes: Vec<_> = (0..LIMIT).filter_map(|_| invalid.record(start)).collect();
        assert_eq!(changes, [Change::Warn(WARN_AT), Change::Pause(WINDOW)]);
        assert_eq!(invalid.paused(start), Some(WINDOW));
    }

    #[test]
    fn a_pause_lasts_until_the_window_is_under_the_limit() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let mut invalid = InvalidRequests::default();
        for _ in 0..LIMIT - 1 {
            invalid.record(start);
        }
        let later = start + second * 60;
        assert_eq!(invalid.record(later), Some(Change::Pause(WINDOW - second * 60)), "until the first ones go");
        assert_eq!(invalid.record(later), Some(Change::Pause(WINDOW - second * 60)), "still two too many then");
        assert_eq!(invalid.paused(start + WINDOW - second), Some(second));
        assert_eq!(invalid.paused(start + WINDOW), None);

        // At one a second, the window never holds enough to pause.
        let mut invalid = InvalidRequests::default();
        let changes: Vec<_> = (0..LIMIT as u32).filter_map(|i| invalid.record(start + second * i)).collect();
        assert_eq!(changes, []);
    }

    #[test]
    fn refusals_leave_the_window() {
        let start = Instant::now();
        let mut invalid = InvalidRequests::default();
        for _ in 0..LIMIT - 1 {
            invalid.record(start);
        }
        // Ten minutes on, those no longer count, and neither warn nor pause.
        assert_eq!(invalid.record(start + WINDOW), None);
        assert_eq!(invalid.paused(start + WINDOW), None);
    }
}
