//! A URL that answered 429 or 503 is left alone until it says it is ready,
//! as the client's HTTP layer does (`cleanupRequestEntry` and `makeRequest`
//! in 620632): the next request to the same URL waits for the larger of
//! the answer's `retry_after` (5 s when it names none) and a backoff that
//! grows while the URL keeps refusing (`Backoff(1000, 60000)`, 158390).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::backoff::Backoff;

/// What the client assumes when a 429 or 503 says nothing about how long.
const DEFAULT_RETRY_AFTER: f64 = 5.0;

/// The longest hold an answer can ask for. The client takes any number; here
/// one huge `retry_after` would panic `Duration` or park an upload for good.
const MAX_HOLD: Duration = Duration::from_secs(60 * 60);

/// The statuses that hold a URL back (`A`, 620632).
pub fn holds(status: u32) -> bool {
    matches!(status, 429 | 503)
}

/// How long an answer says to wait, in seconds (`h`, 620632): the whole
/// seconds of a `Retry-After` header (`parseInt`: a date, or a fraction
/// under one, says nothing), else the body's `retry_after` number. Only
/// more than zero counts.
pub fn retry_after(header: Option<&str>, body: &[u8]) -> Option<f64> {
    let from_header = header.and_then(|header| {
        let digits: String = header.trim_start().chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok().filter(|seconds| *seconds > 0).map(f64::from)
    });
    from_header.or_else(|| {
        let seconds = serde_json::from_slice::<Value>(body).ok()?["retry_after"].as_f64()?;
        (seconds.is_finite() && seconds > 0.0).then_some(seconds)
    })
}

struct Entry {
    until: Instant,
    backoff: Backoff,
    /// A request waited on it, which keeps it past `until` for the request
    /// that follows (the client's queue).
    waited: bool,
}

/// The URLs being held back, and until when.
#[derive(Default)]
pub struct RateHolds {
    entries: HashMap<String, Entry>,
}

impl RateHolds {
    /// How long a request to `url` waits before it is sent, if at all.
    pub fn wait(&mut self, url: &str, now: Instant) -> Option<Duration> {
        let entry = self.entries.get_mut(url)?;
        if entry.until > now {
            entry.waited = true;
            return Some(entry.until - now);
        }
        // Over: the client's timer drops an entry nobody queued behind.
        if !std::mem::take(&mut entry.waited) {
            self.entries.remove(url);
        }
        None
    }

    /// What `url` answered, with the seconds it named ([`retry_after`]).
    /// `random` is in `0.0..1.0`.
    pub fn answered(&mut self, url: &str, status: u32, retry_after: Option<f64>, random: f64, now: Instant) {
        if !holds(status) {
            // An answer after the hold is over lets the entry go.
            if self.entries.get(url).is_some_and(|entry| entry.until <= now) {
                self.entries.remove(url);
            }
            return;
        }
        let entry = self.entries.entry(url.to_owned()).or_insert_with(|| Entry {
            until: now,
            backoff: Backoff::new(Duration::from_secs(1), Duration::from_secs(60)),
            waited: false,
        });
        let backoff = entry.backoff.fail(random);
        let asked = Duration::try_from_secs_f64(retry_after.unwrap_or(DEFAULT_RETRY_AFTER)).unwrap_or(MAX_HOLD);
        let wait = backoff.max(asked).min(MAX_HOLD);
        // A hold that already runs longer stays.
        entry.until = entry.until.max(now + wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: f64) -> Duration {
        Duration::from_secs_f64(n)
    }

    #[test]
    fn what_an_answer_says_to_wait() {
        assert_eq!(retry_after(Some("7"), b""), Some(7.0));
        assert_eq!(retry_after(Some(" 12 "), b""), Some(12.0));
        assert_eq!(retry_after(Some("0.5"), b""), None, "parseInt reads 0, which says nothing");
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT"), b""), None);
        assert_eq!(retry_after(Some("0"), br#"{"retry_after": 2.5}"#), Some(2.5), "the body when the header says none");
        assert_eq!(retry_after(Some("3"), br#"{"retry_after": 2.5}"#), Some(3.0), "the header first");
        assert_eq!(retry_after(None, br#"{"retry_after": 0.4}"#), Some(0.4));
        assert_eq!(retry_after(None, br#"{"retry_after": 0}"#), None);
        assert_eq!(retry_after(None, b"<xml>Too Many Requests</xml>"), None);
    }

    #[test]
    fn only_429_and_503_hold_a_url() {
        assert!(holds(429) && holds(503));
        assert!(!holds(500) && !holds(502) && !holds(200) && !holds(403));
    }

    #[test]
    fn a_url_that_said_429_is_waited_for_until_its_retry_after() {
        let now = Instant::now();
        let mut holds = RateHolds::default();
        assert_eq!(holds.wait("u", now), None);
        holds.answered("u", 429, Some(30.0), 0.0, now);
        assert_eq!(holds.wait("u", now + secs(10.0)), Some(secs(20.0)));
        assert_eq!(holds.wait("other", now), None, "only that URL");
        assert_eq!(holds.wait("u", now + secs(30.0)), None, "over");
    }

    #[test]
    fn with_nothing_said_it_is_five_seconds_and_a_503_holds_too() {
        let now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 503, None, 0.0, now);
        assert_eq!(holds.wait("u", now), Some(secs(5.0)));
    }

    #[test]
    fn a_short_retry_after_waits_the_backoff_instead() {
        let now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(0.2), 0.0, now);
        assert_eq!(holds.wait("u", now), Some(secs(1.0)), "the first backoff wait is a second");
    }

    #[test]
    fn a_url_refusing_again_while_waited_on_waits_longer_each_time() {
        let mut now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(0.2), 0.5, now);
        let first = holds.wait("u", now).unwrap();
        assert_eq!(first, secs(2.0), "1 s plus half of double");
        now += first;
        assert_eq!(holds.wait("u", now), None, "released");
        holds.answered("u", 429, Some(0.2), 0.5, now);
        assert_eq!(holds.wait("u", now), Some(secs(4.0)), "the entry stayed, so its backoff did");
    }

    #[test]
    fn a_hold_nobody_waited_on_is_forgotten_with_its_backoff() {
        let mut now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(0.2), 0.5, now);
        now += secs(10.0);
        assert_eq!(holds.wait("u", now), None);
        holds.answered("u", 429, Some(0.2), 0.5, now);
        assert_eq!(holds.wait("u", now), Some(secs(2.0)), "starts over");
    }

    #[test]
    fn an_answer_after_the_hold_lets_it_go() {
        let mut now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(2.0), 0.0, now);
        assert_eq!(holds.wait("u", now), Some(secs(2.0)));
        now += secs(2.0);
        assert_eq!(holds.wait("u", now), None);
        holds.answered("u", 200, None, 0.0, now);
        holds.answered("u", 429, Some(0.2), 0.5, now);
        assert_eq!(holds.wait("u", now), Some(secs(2.0)), "not 4 s: the 200 ended it");
    }

    #[test]
    fn a_longer_hold_is_not_cut_short_by_a_shorter_one() {
        let now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(60.0), 0.0, now);
        holds.answered("u", 429, Some(2.0), 0.0, now);
        assert_eq!(holds.wait("u", now), Some(secs(60.0)));
    }

    #[test]
    fn a_huge_retry_after_is_held_for_an_hour_at_most() {
        let now = Instant::now();
        let mut holds = RateHolds::default();
        holds.answered("u", 429, Some(1e30), 0.0, now);
        assert_eq!(holds.wait("u", now), Some(MAX_HOLD));
    }
}
