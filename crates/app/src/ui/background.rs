//! Running with no window: the tray icon, hiding the window instead of
//! closing it, and starting with the session.
//!
//! What the system side does is in [`crate::tray`] and [`crate::background`];
//! this decides when to use it.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::App;
use crate::background;
use crate::config;
use crate::tray::{Alert, Command, Event};

/// A hidden start shows its window after all if no panel has picked the
/// icon up by then: without one, nothing else would bring it back.
const SHOW_ANYWAY_AFTER: Duration = Duration::from_secs(10);

impl App {
    /// Put the icon in the panel and settle the autostart entry. `hidden`
    /// keeps the window away until the icon is up.
    pub(super) fn start_background(self: &Rc<Self>, hidden: bool) {
        let this = Rc::downgrade(self);
        self.tray.start(move |event| {
            let Some(this) = this.upgrade() else { return };
            match event {
                Event::Command(Command::Open) => this.window.present(),
                Event::Command(Command::Quit) => {
                    if let Some(app) = this.window.application() {
                        app.quit();
                    }
                }
                // The panel that held the icon is gone, and so is the way back in.
                Event::Shown(false) if !this.window.is_visible() => this.window.present(),
                Event::Shown(_) => {}
            }
        });
        if hidden {
            let this = Rc::downgrade(self);
            glib::timeout_add_local_once(SHOW_ANYWAY_AFTER, move || {
                if let Some(this) = this.upgrade().filter(|this| !this.tray.shown()) {
                    this.window.present();
                }
            });
        }
        self.sync_background(true);
    }

    /// Whether closing the window hides it. Not when nothing shows the icon,
    /// or the system refused: the app would be running where nobody could
    /// reach it.
    pub(super) fn keeps_running(&self) -> bool {
        self.config.get(&config::MINIMIZE_TO_TRAY) && self.tray.shown() && !self.background_refused.get()
    }

    /// The icon says what the rail does, unless the option is off.
    pub(super) fn refresh_tray(&self) {
        let alert = if self.config.get(&config::UNREAD_BADGE) {
            self.account.state.borrow().tray_alert()
        } else {
            Alert::Quiet
        };
        self.tray.set_alert(alert);
    }

    /// Make the system agree with the options. At `startup` only what is
    /// wanted is asked for, so an entry the user made by hand is not removed
    /// on every launch; a change in the settings window sets both ways.
    pub(super) fn sync_background(self: &Rc<Self>, startup: bool) {
        let autostart = self.config.get(&config::OPEN_ON_STARTUP);
        let wanted = autostart || self.config.get(&config::MINIMIZE_TO_TRAY);
        if !wanted {
            self.background_refused.set(false);
        }
        if !wanted && startup {
            return;
        }
        if background::sandboxed() {
            let this = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let answer = background::request(autostart).await;
                let Some(this) = this.upgrade() else { return };
                match answer {
                    Ok(grant) => {
                        this.background_refused.set(wanted && !grant.background);
                        if autostart && !grant.autostart {
                            this.settings_toast("The system didn't allow starting Neucord with your session");
                        }
                    }
                    Err(e) => eprintln!("background portal: {e}"),
                }
            });
        } else if (autostart || !startup)
            && let Err(e) = background::set_autostart(autostart)
        {
            eprintln!("autostart: {e}");
            self.settings_toast("Could not change whether Neucord starts with your session");
        }
    }
}
