//! Marking channels read (here and on Discord) and keeping unread
//! markers in the rail and sidebar current.

use std::rc::Rc;
use std::time::Duration;

use discord::Snowflake;
use gtk::glib;

use super::App;

/// Acks for a channel within this window are sent as one request.
const ACK_DELAY: Duration = Duration::from_millis(1500);

impl App {
    /// We are looking at `channel`: if it has anything unread, ack its newest
    /// message here and on Discord.
    pub(super) fn mark_read(self: &Rc<Self>, channel: Snowflake) {
        let newest = {
            let mut state = self.state.borrow_mut();
            let Some((_, c)) = state.find_channel(channel) else { return };
            let Some(last) = c.last_message_id else { return };
            let pending = state.is_unread(c) || state.read.mentions(channel) > 0;
            if !pending {
                return;
            }
            state.read.ack(channel, last);
            last
        };
        self.withdraw_notification(channel);
        self.queue_ack(channel, newest);
        self.refresh_badges();
    }

    /// Send an ack after a short delay, folding rapid acks for one channel
    /// (a busy channel we are watching) into a single request.
    pub(super) fn queue_ack(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
        // The client acks no archived thread.
        let ackable = self
            .state
            .borrow()
            .find_channel(channel)
            .is_none_or(|(_, c)| !c.kind.is_thread() || discord::threads::is_ackable(c));
        if !ackable {
            return;
        }
        let first = self.state.borrow_mut().pending_acks.insert(channel, message).is_none();
        if !first {
            return;
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(ACK_DELAY, move || {
            let Some(this) = this.upgrade() else { return };
            let (message, rest) = {
                let mut state = this.state.borrow_mut();
                (state.pending_acks.remove(&channel), state.rest.clone())
            };
            let (Some(message), Some(rest)) = (message, rest) else { return };
            glib::spawn_future_local(async move {
                if let Err(e) = rest.ack(channel, message).await {
                    eprintln!("ack {channel}: {e}");
                }
            });
        });
    }

    /// Redraw rail and sidebar if their unread dots, badges, mutes or DM
    /// order changed. Called on every message, so it skips no-op rebuilds.
    pub(super) fn refresh_badges(self: &Rc<Self>) {
        let (rail, sidebar) = {
            let state = self.state.borrow();
            (state.rail_signature(), state.sidebar_signature())
        };
        let (rail_changed, sidebar_changed) = {
            let drawn = self.drawn.borrow();
            (drawn.0 != rail, drawn.1 != sidebar)
        };
        if rail_changed {
            self.rebuild_rail();
        }
        if sidebar_changed {
            self.rebuild_sidebar();
        }
    }
}
