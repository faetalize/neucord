//! Telling the gateway what we show as: the presence update (op 3) the
//! official client sends whenever its local presence changes (`presenceUpdate`
//! in 868, and the reporter around it in 225123).
//!
//! The client identifies with the status `unknown`, which makes Discord take
//! the status setting; the update then keeps the session's presence in step
//! with the setting, which another client may change at any time, and with
//! whether the user is idle or away (see [`crate::idle`]). It remembers the
//! presence it last sent and sends only a different one, after RESUMED it
//! sends that one again, and at most five go out in any 20 seconds: the
//! sixth waits for the oldest to leave the window, and a newer presence
//! replaces one that waits. Nothing here sends or sets a timer; the caller
//! does.
//!
//! Neucord has no games to report, so a presence is its status, since when
//! the user has been idle (0 if not), and whether they are away, with no
//! activities.

use std::collections::VecDeque;
use std::time::Duration;

use crate::settings::UserStatus;

/// The updates allowed in [`WINDOW`].
const BURST: usize = 5;
const WINDOW_MS: u64 = 20_000;

/// What we show as: the fields of op 3 Neucord fills in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub status: UserStatus,
    /// When the user went idle, in ms since the epoch, or 0.
    pub since: u64,
    pub away: bool,
}

impl Local {
    /// What a session starts as, and what the client compares the first
    /// presence with: an account that is Online, active and present is never
    /// told so.
    pub const INITIAL: Local = Local { status: UserStatus::Online, since: 0, away: false };

    /// The presence of an account whose status setting is `status` while
    /// the user is idle since `idle_since` (if they are) and `away`: an
    /// Online account that is idle shows as Idle, and no other status
    /// changes (`SelfPresenceStore`, 894768).
    pub fn of(status: UserStatus, idle_since: Option<u64>, away: bool) -> Self {
        let since = idle_since.unwrap_or(0);
        let status = if status == UserStatus::Online && since > 0 { UserStatus::Idle } else { status };
        Local { status, since, away }
    }
}

/// What to do with a presence the reporter was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    Now(Local),
    /// Try again after the wait, with this presence unless a newer one has
    /// replaced it.
    After(Duration, Local),
}

pub struct PresenceReporter {
    /// The presence last handed out.
    committed: Local,
    /// When the updates of the last [`WINDOW`] leave it, oldest first.
    leaving: VecDeque<u64>,
}

impl Default for PresenceReporter {
    fn default() -> Self {
        PresenceReporter { committed: Local::INITIAL, leaving: VecDeque::new() }
    }
}

impl PresenceReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The presence is now `local`. `None` if that is what was last sent.
    pub fn changed(&mut self, local: Local, now_ms: u64) -> Option<Send> {
        if self.committed == local {
            return None;
        }
        self.committed = local;
        Some(self.admit(local, now_ms))
    }

    /// A session started (READY). What it shows is the status setting, as
    /// [`Local::INITIAL`] says of an Online account, so whatever else we show
    /// is told again, however it compared with the last session's
    /// (`handleConnectionOpen` in 225123 compares with the initial state).
    pub fn opened(&mut self, local: Local, now_ms: u64) -> Option<Send> {
        self.committed = Local::INITIAL;
        self.changed(local, now_ms)
    }

    /// The session resumed: the client sends the presence it last committed
    /// again, whatever it was.
    pub fn resumed(&mut self, now_ms: u64) -> Send {
        self.admit(self.committed, now_ms)
    }

    /// A [`Send::After`] came due: the same decision again, as the client
    /// calls its throttled function again.
    pub fn admit(&mut self, local: Local, now_ms: u64) -> Send {
        while self.leaving.front().is_some_and(|leaves| *leaves <= now_ms) {
            self.leaving.pop_front();
        }
        match self.leaving.front() {
            Some(leaves) if self.leaving.len() >= BURST => Send::After(Duration::from_millis(leaves - now_ms), local),
            _ => {
                self.leaving.push_back(now_ms + WINDOW_MS);
                Send::Now(local)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: UserStatus) -> Local {
        Local { status, ..Local::INITIAL }
    }

    #[test]
    fn online_is_what_a_new_session_already_is() {
        let mut reporter = PresenceReporter::new();
        assert_eq!(reporter.changed(Local::INITIAL, 0), None);
        let dnd = status(UserStatus::DoNotDisturb);
        assert_eq!(reporter.changed(dnd, 0), Some(Send::Now(dnd)));
        assert_eq!(reporter.changed(dnd, 1), None, "only a different presence goes out");
        assert_eq!(reporter.changed(Local::INITIAL, 2), Some(Send::Now(Local::INITIAL)));
    }

    #[test]
    fn going_away_or_idle_is_a_different_presence() {
        let mut reporter = PresenceReporter::new();
        let away = Local { away: true, ..Local::INITIAL };
        assert_eq!(reporter.changed(away, 0), Some(Send::Now(away)), "away alone is told, with the status as it was");
        let idle = Local::of(UserStatus::Online, Some(5000), true);
        assert_eq!(reporter.changed(idle, 1), Some(Send::Now(idle)));
        assert_eq!(reporter.changed(Local::of(UserStatus::Online, Some(5000), true), 2), None);
        assert_eq!(
            reporter.changed(Local::of(UserStatus::Online, Some(9000), true), 3),
            Some(Send::Now(Local::of(UserStatus::Online, Some(9000), true))),
            "idle since another time is another presence"
        );
    }

    #[test]
    fn only_an_online_account_shows_idle_on_its_own() {
        let idle = Some(5000);
        let shown = |s| Local::of(s, idle, true);
        assert_eq!(shown(UserStatus::Online), Local { status: UserStatus::Idle, since: 5000, away: true });
        for kept in [UserStatus::Idle, UserStatus::DoNotDisturb, UserStatus::Invisible] {
            assert_eq!(shown(kept), Local { status: kept, since: 5000, away: true }, "{kept:?} stays, `since` is told");
        }
        assert_eq!(Local::of(UserStatus::Online, None, false), Local::INITIAL);
        assert_eq!(Local::of(UserStatus::Online, Some(0), false).status, UserStatus::Online, "0 is no time at all");
    }

    #[test]
    fn a_new_session_is_told_whatever_is_not_the_initial_presence_again() {
        let mut reporter = PresenceReporter::new();
        let idle = Local::of(UserStatus::Online, Some(5000), true);
        assert_eq!(reporter.opened(idle, 0), Some(Send::Now(idle)));
        assert_eq!(reporter.opened(idle, 1), Some(Send::Now(idle)), "the next session knows nothing of it");
        assert_eq!(reporter.opened(Local::INITIAL, 2), None, "active and Online is what it starts as");
        assert_eq!(reporter.changed(idle, 3), Some(Send::Now(idle)), "and that is what it is compared with after");
    }

    #[test]
    fn a_resumed_session_hears_the_last_presence_again() {
        let mut reporter = PresenceReporter::new();
        assert_eq!(reporter.resumed(0), Send::Now(Local::INITIAL), "even the one nobody sent");
        let invisible = status(UserStatus::Invisible);
        reporter.changed(invisible, 1);
        assert_eq!(reporter.resumed(2), Send::Now(invisible));
    }

    #[test]
    fn five_in_twenty_seconds_and_the_sixth_waits_for_the_first_to_leave() {
        let mut reporter = PresenceReporter::new();
        let cycle = [status(UserStatus::Idle), Local::INITIAL];
        for n in 0..5 {
            let now = n * 1000;
            assert_eq!(reporter.changed(cycle[n as usize % 2], now), Some(Send::Now(cycle[n as usize % 2])));
        }
        // The first leaves at 20 000, 15 000 from now.
        let waits = reporter.changed(Local::INITIAL, 5000);
        assert_eq!(waits, Some(Send::After(Duration::from_millis(15_000), Local::INITIAL)));
        // When it is time the decision is made again, and the window has room.
        assert_eq!(reporter.admit(Local::INITIAL, 20_000), Send::Now(Local::INITIAL));
        // The next one is still held by the second of the five.
        assert_eq!(reporter.admit(Local::INITIAL, 20_500), Send::After(Duration::from_millis(500), Local::INITIAL));
    }
}
