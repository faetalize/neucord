//! When the user counts as idle and as away, as the client's `IdleStore`
//! (885576) decides it for a browser: no input for ten minutes is idle, and
//! none for the account's AFK timeout (a minute unless it says otherwise,
//! at most ten) is away, which keeps Discord from sending push
//! notifications to a phone while we are at the keyboard. Both come from
//! one check, made every 30 seconds and after each bit of activity.
//!
//! Activity is a mouse button released, the wheel, a typed character (all
//! counted once nothing follows for half a second), a channel opened, and
//! the window gaining or losing focus. Nothing here reads a clock or sets a
//! timer; the caller passes the time and keeps the timers.

use std::time::Duration;

/// Inactivity after which the user is idle.
const IDLE_AFTER_MS: u64 = 10 * 60 * 1000;
/// The account's AFK timeout when it sets none (`afkTimeout`, 253932).
pub const DEFAULT_AFK_TIMEOUT: u32 = 60;
/// How often the check runs on its own.
pub const CHECK_EVERY: Duration = Duration::from_secs(30);
/// Input counts once it has been quiet this long (a debounce, 885576).
const DEBOUNCE_MS: u64 = 500;

/// Debug aid: with `NEUCORD_DEBUG_IDLE=<seconds>`, the user is idle after
/// that many seconds without input, and the check runs every second.
pub fn debug_idle_after() -> Option<Duration> {
    std::env::var("NEUCORD_DEBUG_IDLE").ok()?.parse().ok().map(Duration::from_secs)
}

/// What a quiet spell of input comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Debounced {
    /// More input came since: ask again after this long.
    Wait(Duration),
    /// It has been quiet: the user was active; `changed` says whether idle
    /// or away did.
    Active { changed: bool },
}

#[derive(Debug)]
pub struct Idle {
    /// When the user was last active, in ms since the epoch.
    active_at: u64,
    idle: bool,
    away: bool,
    /// The last input not counted yet.
    input_at: Option<u64>,
    /// The inactivity that is idle: ten minutes, or what a debug run says.
    idle_after_ms: u64,
}

impl Idle {
    pub fn new(now_ms: u64) -> Self {
        Self::after(now_ms, IDLE_AFTER_MS)
    }

    /// Idle after `idle_after_ms` of inactivity instead of ten minutes, for
    /// `NEUCORD_DEBUG_IDLE`.
    pub fn after(now_ms: u64, idle_after_ms: u64) -> Self {
        Idle { active_at: now_ms, idle: false, away: false, input_at: None, idle_after_ms }
    }

    /// The idle check (`N`, 885576). `afk_timeout` is the account's, in
    /// seconds; 0 keeps the user away. Whether idle or away changed.
    pub fn check(&mut self, now_ms: u64, afk_timeout: u32) -> bool {
        let quiet = now_ms.saturating_sub(self.active_at);
        let before = (self.idle, self.away);
        self.idle = quiet > self.idle_after_ms;
        self.away = afk_timeout == 0 || quiet > (u64::from(afk_timeout) * 1000).min(self.idle_after_ms);
        before != (self.idle, self.away)
    }

    /// The user did something worth counting at once (a channel opened, the
    /// window's focus changed). Whether idle or away changed.
    pub fn active(&mut self, now_ms: u64, afk_timeout: u32) -> bool {
        self.active_at = now_ms;
        self.check(now_ms, afk_timeout)
    }

    /// Input arrived. True if nothing was waiting to be counted, so the
    /// caller starts the half second's wait ([`Idle::debounced`]).
    pub fn input(&mut self, now_ms: u64) -> bool {
        self.input_at.replace(now_ms).is_none()
    }

    /// The half second the caller waited is over.
    pub fn debounced(&mut self, now_ms: u64, afk_timeout: u32) -> Debounced {
        let Some(at) = self.input_at else { return Debounced::Active { changed: false } };
        let quiet = now_ms.saturating_sub(at);
        if quiet < DEBOUNCE_MS {
            return Debounced::Wait(Duration::from_millis(DEBOUNCE_MS - quiet));
        }
        self.input_at = None;
        Debounced::Active { changed: self.active(now_ms, afk_timeout) }
    }

    /// Since when the user has been idle, if they are.
    pub fn idle_since(&self) -> Option<u64> {
        self.idle.then_some(self.active_at)
    }

    pub fn away(&self) -> bool {
        self.away
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: u64 = 60_000;

    #[test]
    fn nothing_is_idle_or_away_before_the_first_check() {
        let idle = Idle::new(1000);
        assert_eq!((idle.idle_since(), idle.away()), (None, false));
    }

    #[test]
    fn a_minute_without_input_is_away_and_ten_are_idle() {
        let mut idle = Idle::new(0);
        assert!(!idle.check(MINUTE, 60), "a minute exactly is not past it");
        assert!(idle.check(MINUTE + 1, 60));
        assert_eq!((idle.idle_since(), idle.away()), (None, true));
        assert!(!idle.check(9 * MINUTE, 60), "nothing changes between");
        assert!(!idle.check(10 * MINUTE, 60));
        assert!(idle.check(10 * MINUTE + 1, 60));
        assert_eq!((idle.idle_since(), idle.away()), (Some(0), true), "since when it was last active");
    }

    #[test]
    fn the_afk_timeout_is_the_accounts_up_to_ten_minutes() {
        let mut idle = Idle::new(0);
        idle.check(299_999, 300);
        assert!(!idle.away());
        idle.check(300_001, 300);
        assert!(idle.away());
        let mut idle = Idle::new(0);
        idle.check(10 * MINUTE, 3600);
        assert!(!idle.away(), "an hour's timeout is capped at ten minutes, which isn't past yet");
        idle.check(10 * MINUTE + 1, 3600);
        assert!(idle.away());
    }

    #[test]
    fn a_timeout_of_zero_keeps_the_user_away_even_when_active() {
        let mut idle = Idle::new(0);
        assert!(idle.check(1, 0));
        assert!(idle.away());
        assert!(!idle.active(5, 0), "still away");
        assert!(idle.away());
    }

    #[test]
    fn activity_ends_both_and_starts_the_ten_minutes_again() {
        let mut idle = Idle::new(0);
        idle.check(11 * MINUTE, 60);
        assert!(idle.active(11 * MINUTE + 5, 60));
        assert_eq!((idle.idle_since(), idle.away()), (None, false));
        assert!(idle.check(20 * MINUTE, 60), "away again after a minute");
        assert_eq!(idle.idle_since(), None, "but not idle yet");
        idle.check(21 * MINUTE + 6, 60);
        assert_eq!(idle.idle_since(), Some(11 * MINUTE + 5));
    }

    #[test]
    fn input_counts_once_it_has_been_quiet_for_half_a_second() {
        let mut idle = Idle::new(0);
        idle.check(2 * MINUTE, 60);
        assert!(idle.away());
        assert!(idle.input(2 * MINUTE), "the first starts the wait");
        assert!(!idle.input(2 * MINUTE + 200), "later ones only push it back");
        assert_eq!(idle.debounced(2 * MINUTE + 500, 60), Debounced::Wait(Duration::from_millis(200)));
        assert!(idle.away(), "not active yet");
        assert_eq!(idle.debounced(2 * MINUTE + 700, 60), Debounced::Active { changed: true });
        assert!(!idle.away());
        assert!(idle.input(3 * MINUTE), "and a new wait starts again");
    }

    #[test]
    fn a_wait_with_nothing_to_count_changes_nothing() {
        let mut idle = Idle::new(0);
        assert_eq!(idle.debounced(5, 60), Debounced::Active { changed: false });
    }

    #[test]
    fn a_debug_run_goes_idle_sooner() {
        let mut idle = Idle::after(0, 5000);
        assert!(idle.check(5001, 60));
        assert_eq!(idle.idle_since(), Some(0));
        assert!(idle.away(), "the timeout is capped by the same limit");
    }
}
