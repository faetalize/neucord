//! Telling Discord when we are idle or away: what counts as the user being
//! there and the periodic check are `discord::idle`'s; this feeds it input
//! and reports the presence it changes (`IdleStore`, 885576, which in a
//! browser listens to `mouseup`, `wheel` and `keypress` on the window).

use std::rc::Rc;
use std::time::Duration;

use discord::idle::{self, Debounced};
use gtk::prelude::*;
use gtk::{gdk, glib};

use super::App;
use super::state::now_ms;

impl App {
    /// Listen for the input the client counts, and start the check.
    pub(super) fn watch_idle(self: &Rc<Self>) {
        // Seen, never taken: the controller only looks at events on their way down.
        let legacy = gtk::EventControllerLegacy::new();
        legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = Rc::downgrade(self);
        legacy.connect_event(move |_, event| {
            if counts(event)
                && let Some(this) = this.upgrade()
            {
                this.idle_input();
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(legacy);

        let every = idle::debug_idle_after().map_or(idle::CHECK_EVERY, |_| Duration::from_secs(1));
        let this = Rc::downgrade(self);
        glib::timeout_add_local(every, move || {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            let afk = this.account.state.borrow().settings.afk_timeout();
            if this.idle.borrow_mut().check(now_ms() as u64, afk) {
                this.report_presence();
            }
            glib::ControlFlow::Continue
        });
    }

    /// The user did something the client counts at once: a channel opened,
    /// the window's focus changed (`CHANNEL_SELECT` and `WINDOW_FOCUS`).
    pub(super) fn idle_active(self: &Rc<Self>) {
        let afk = self.account.state.borrow().settings.afk_timeout();
        if self.idle.borrow_mut().active(now_ms() as u64, afk) {
            self.report_presence();
        }
    }

    /// Input arrived: it counts once it has been quiet for half a second.
    fn idle_input(self: &Rc<Self>) {
        if self.idle.borrow_mut().input(now_ms() as u64) {
            self.idle_debounce(Duration::from_millis(500));
        }
    }

    fn idle_debounce(self: &Rc<Self>, wait: Duration) {
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(wait, move || {
            let Some(this) = this.upgrade() else { return };
            let afk = this.account.state.borrow().settings.afk_timeout();
            let debounced = this.idle.borrow_mut().debounced(now_ms() as u64, afk);
            match debounced {
                Debounced::Wait(wait) => this.idle_debounce(wait),
                Debounced::Active { changed } if changed => this.report_presence(),
                Debounced::Active { .. } => {}
            }
        });
    }
}

/// A mouse button released, the wheel turned, or a character typed: the
/// events a browser reports as `mouseup`, `wheel` and `keypress` (a key
/// that makes no character, such as an arrow or Escape, makes no `keypress`;
/// Enter does). A touch ends as a click does.
fn counts(event: &gdk::Event) -> bool {
    match event.event_type() {
        gdk::EventType::ButtonRelease | gdk::EventType::Scroll | gdk::EventType::TouchEnd => true,
        gdk::EventType::KeyPress => event.downcast_ref::<gdk::KeyEvent>().is_some_and(|key| {
            let keyval = key.keyval();
            keyval == gdk::Key::Return
                || keyval == gdk::Key::KP_Enter
                || keyval.to_unicode().is_some_and(|c| !c.is_control())
        }),
        _ => false,
    }
}
