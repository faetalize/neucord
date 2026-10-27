//! What Discord holds against the account: a page that covers the window
//! while Discord requires a step, as the official client's verification
//! screen covers it, and the account's standing.

use std::rc::Rc;

use adw::prelude::*;
use discord::account::{self, Level};
use gtk::glib;

use super::{App, bare_page};

impl App {
    pub(super) fn build_blocked(&self) -> gtk::Widget {
        let logout = gtk::Button::builder().label("_Log Out").use_underline(true).action_name("win.logout").build();
        let child = gtk::Box::builder().halign(gtk::Align::Center).build();
        child.append(&logout);
        self.blocked.set_title("Discord Needs Something First");
        self.blocked.set_child(Some(&child));
        bare_page(&self.blocked)
    }

    /// Cover the window while Discord requires a step, and uncover it once
    /// Discord says it's done. The gateway stays connected meanwhile, which
    /// is how that news arrives.
    pub(super) fn apply_required_action(&self) {
        let action = self.state.borrow().required_action.clone();
        match action {
            Some(action) => {
                self.blocked.set_description(Some(&format!(
                    "{} Do it in the official app or at discord.com. Neucord carries on once Discord confirms it.",
                    account::required_action_text(&action)
                )));
                self.stack.set_visible_child_name("blocked");
            }
            None if self.stack.visible_child_name().as_deref() == Some("blocked") => {
                self.stack.set_visible_child_name("main");
            }
            None => {}
        }
    }

    /// Discord's Account Standing page, as a dialog.
    pub(super) fn show_standing(self: &Rc<Self>) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let dialog = match rest.account_standing().await {
                Ok(standing) => standing_dialog(&standing),
                Err(e) => adw::AlertDialog::new(Some("Could Not Load Account Standing"), Some(&e.to_string())),
            };
            dialog.add_response("close", "_Close");
            dialog.present(Some(&this.window));
        });
    }
}

fn standing_dialog(standing: &account::Standing) -> adw::AlertDialog {
    let heading = match standing.level {
        Some(level) => format!("Your account is {}", level.name()),
        None => "Account Standing".to_owned(),
    };
    let mut body = standing
        .level
        .map(Level::description)
        .unwrap_or("Discord reported a standing Neucord doesn't know.")
        .to_owned();
    let now = glib::DateTime::now_utc().map(|t| t.to_unix()).unwrap_or_default();
    let active: Vec<String> = standing
        .violations
        .iter()
        .filter(|v| v.active(now))
        .map(|v| {
            let until = glib::DateTime::from_iso8601(&v.max_expiration_time, None)
                .and_then(|t| t.to_local())
                .and_then(|t| t.format("%-d %B %Y"))
                .map(|date| format!(", until {date}"))
                .unwrap_or_default();
            format!("• You broke the rules for {}{until}", v.description)
        })
        .collect();
    if !active.is_empty() {
        body.push_str(&format!("\n\nActive violations:\n{}", active.join("\n")));
    }
    adw::AlertDialog::new(Some(&heading), Some(&body))
}
