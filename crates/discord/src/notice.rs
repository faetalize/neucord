//! What takes the message box's place where we can't post, and whether the
//! attach button stays beside it. The official client turns the box off and
//! keeps the button where we may still create threads (`nS` and the
//! `(!disabled || canCreateThreads)` test in 989161), so Create Thread stays
//! reachable in a channel we can read but not write in.

use crate::verification::Gate;

/// Why the message box is replaced, most pressing first.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The system DM, where nobody writes.
    System,
    /// Our timeout in the server, with the seconds left.
    Timeout(i64),
    /// A message request, filed as likely spam or not.
    Request { spam: bool },
    /// The server's verification asks for more.
    Gate(Gate),
    /// No Send Messages, or a thread nobody may write in.
    Denied,
}

impl Notice {
    /// The notice that stands where the box would be, or `None` where the
    /// box shows.
    pub fn of(
        system: bool,
        request: Option<bool>,
        timeout: Option<i64>,
        gate: Option<Gate>,
        denied: bool,
    ) -> Option<Self> {
        if system {
            return Some(Self::System);
        }
        if let Some(left) = timeout {
            return Some(Self::Timeout(left));
        }
        if let Some(spam) = request {
            return Some(Self::Request { spam });
        }
        gate.map(Self::Gate).or(denied.then_some(Self::Denied))
    }

    /// Whether the attach button stays beside it: only where the box is off
    /// for want of Send Messages, since Create Thread needs none. The rest
    /// have no box in the client either, or hold everything we send back.
    pub fn keeps_attach_button(&self) -> bool {
        matches!(self, Self::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_pressing_reason_wins() {
        assert_eq!(Notice::of(true, Some(true), Some(5), Some(Gate::Phone), true), Some(Notice::System));
        assert_eq!(Notice::of(false, Some(true), Some(5), Some(Gate::Phone), true), Some(Notice::Timeout(5)));
        assert_eq!(Notice::of(false, Some(true), None, Some(Gate::Phone), true), Some(Notice::Request { spam: true }));
        assert_eq!(Notice::of(false, None, None, Some(Gate::Phone), true), Some(Notice::Gate(Gate::Phone)));
        assert_eq!(Notice::of(false, None, None, None, true), Some(Notice::Denied));
        assert_eq!(Notice::of(false, None, None, None, false), None, "the box shows");
    }

    #[test]
    fn the_button_stays_only_where_we_merely_lack_send_messages() {
        // Create Thread wants no Send Messages, so a channel that is read-only for us still offers it.
        assert!(Notice::Denied.keeps_attach_button());
        // The rest have no box in the client either, or hold back everything we send.
        for notice in [
            Notice::System,
            Notice::Timeout(5),
            Notice::Request { spam: false },
            Notice::Request { spam: true },
            Notice::Gate(Gate::Phone),
        ] {
            assert!(!notice.keeps_attach_button(), "{notice:?}");
        }
    }
}
