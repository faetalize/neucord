//! Telling the gateway what we show as: the presence update (op 3) the
//! official client sends whenever its local presence changes (`presenceUpdate`
//! in 868, and the reporter around it in 225123).
//!
//! The client identifies with the status `unknown`, which makes Discord take
//! the status setting; the update then keeps the session's presence in step
//! with the setting, which another client may change at any time. It
//! remembers the presence it last sent and sends only a different one, after
//! RESUMED it sends that one again, and at most five go out in any 20
//! seconds: the sixth waits for the oldest to leave the window, and a newer
//! presence replaces one that waits. Nothing here sends or sets a timer; the
//! caller does.
//!
//! Neucord has no games to report and does not watch for the device going
//! idle, so a presence is just its status (`since` 0, no activities, not
//! away).

use std::collections::VecDeque;
use std::time::Duration;

use crate::settings::UserStatus;

/// The updates allowed in [`WINDOW`].
const BURST: usize = 5;
const WINDOW_MS: u64 = 20_000;

/// What to do with a presence the reporter was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    Now(UserStatus),
    /// Try again after the wait, with this presence unless a newer one has
    /// replaced it.
    After(Duration, UserStatus),
}

pub struct PresenceReporter {
    /// The presence last handed out, which the client starts at Online: an
    /// account whose status is Online is never told so.
    committed: UserStatus,
    /// When the updates of the last [`WINDOW`] leave it, oldest first.
    leaving: VecDeque<u64>,
}

impl Default for PresenceReporter {
    fn default() -> Self {
        PresenceReporter { committed: UserStatus::Online, leaving: VecDeque::new() }
    }
}

impl PresenceReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The presence is now `status`. `None` if that is what was last sent.
    pub fn changed(&mut self, status: UserStatus, now_ms: u64) -> Option<Send> {
        if self.committed == status {
            return None;
        }
        self.committed = status;
        Some(self.admit(status, now_ms))
    }

    /// The session resumed: the client sends the presence it last committed
    /// again, whatever it was.
    pub fn resumed(&mut self, now_ms: u64) -> Send {
        self.admit(self.committed, now_ms)
    }

    /// A [`Send::After`] came due: the same decision again, as the client
    /// calls its throttled function again.
    pub fn admit(&mut self, status: UserStatus, now_ms: u64) -> Send {
        while self.leaving.front().is_some_and(|leaves| *leaves <= now_ms) {
            self.leaving.pop_front();
        }
        match self.leaving.front() {
            Some(leaves) if self.leaving.len() >= BURST => Send::After(Duration::from_millis(leaves - now_ms), status),
            _ => {
                self.leaving.push_back(now_ms + WINDOW_MS);
                Send::Now(status)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_is_what_a_new_session_already_is() {
        let mut reporter = PresenceReporter::new();
        assert_eq!(reporter.changed(UserStatus::Online, 0), None);
        assert_eq!(reporter.changed(UserStatus::DoNotDisturb, 0), Some(Send::Now(UserStatus::DoNotDisturb)));
        assert_eq!(reporter.changed(UserStatus::DoNotDisturb, 1), None, "only a different presence goes out");
        assert_eq!(reporter.changed(UserStatus::Online, 2), Some(Send::Now(UserStatus::Online)));
    }

    #[test]
    fn a_resumed_session_hears_the_last_presence_again() {
        let mut reporter = PresenceReporter::new();
        assert_eq!(reporter.resumed(0), Send::Now(UserStatus::Online), "even the one nobody sent");
        reporter.changed(UserStatus::Invisible, 1);
        assert_eq!(reporter.resumed(2), Send::Now(UserStatus::Invisible));
    }

    #[test]
    fn five_in_twenty_seconds_and_the_sixth_waits_for_the_first_to_leave() {
        let mut reporter = PresenceReporter::new();
        let cycle = [UserStatus::Idle, UserStatus::Online];
        for n in 0..5 {
            let now = n * 1000;
            assert_eq!(reporter.changed(cycle[n as usize % 2], now), Some(Send::Now(cycle[n as usize % 2])));
        }
        // The first leaves at 20 000, 15 000 from now.
        let waits = reporter.changed(UserStatus::Online, 5000);
        assert_eq!(waits, Some(Send::After(Duration::from_millis(15_000), UserStatus::Online)));
        // When it is time the decision is made again, and the window has room.
        assert_eq!(reporter.admit(UserStatus::Online, 20_000), Send::Now(UserStatus::Online));
        // The next one is still held by the second of the five.
        assert_eq!(
            reporter.admit(UserStatus::Online, 20_500),
            Send::After(Duration::from_millis(500), UserStatus::Online)
        );
    }
}
