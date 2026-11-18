//! The form for a password the account has to change (`update_password` in
//! a login's `required_actions`). The client connects, then asks over the app
//! (165685, form 398177), and logs out if it is closed; it sends the current
//! and the new password (`PATCH /users/@me`, 631670) and takes the token
//! Discord answers with.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::App;

/// What the form checks before it sends anything (398177).
pub(super) fn problem(new: &str, again: &str) -> Option<&'static str> {
    if new.is_empty() {
        Some("Your new password cannot be empty.")
    } else if new != again {
        Some("Passwords do not match!")
    } else {
        None
    }
}

impl App {
    /// Ask for a new password over the app.
    pub(super) fn ask_new_password(self: &Rc<Self>) {
        let dialog =
            adw::Dialog::builder().title("Update your password").content_width(420).follows_content_size(true).build();
        let entry = |title: &str| adw::PasswordEntryRow::builder().title(title).build();
        let (current, new, again) = (entry("Current Password"), entry("New Password"), entry("Confirm New Password"));
        let group = adw::PreferencesGroup::builder()
            .description("Discord has new password requirements. Please update your password now.")
            .build();
        for row in [&current, &new, &again] {
            group.add(row);
        }
        let error = gtk::Label::builder().wrap(true).xalign(0.0).css_classes(["error"]).visible(false).build();
        let cancel = gtk::Button::builder().label("_Cancel").use_underline(true).build();
        let done = gtk::Button::builder().label("_Done").use_underline(true).css_classes(["suggested-action"]).build();
        let buttons = gtk::Box::builder().spacing(6).halign(gtk::Align::End).build();
        buttons.append(&cancel);
        buttons.append(&done);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        for child in [group.upcast_ref::<gtk::Widget>(), error.upcast_ref(), buttons.upcast_ref()] {
            column.append(child);
        }
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&column));
        dialog.set_child(Some(&toolbar));

        // Closing it any way but through Done logs out, as the client's does.
        let finished = Rc::new(Cell::new(false));
        cancel.connect_clicked({
            let dialog = dialog.clone();
            move |_| {
                dialog.close();
            }
        });
        let submit = {
            let (this, dialog, finished) = (Rc::downgrade(self), dialog.clone(), finished.clone());
            let (current, new, again, error, done) =
                (current.clone(), new.clone(), again.clone(), error.clone(), done.clone());
            move || {
                let Some(this) = this.upgrade() else { return };
                if !done.is_sensitive() {
                    return;
                }
                let (old, new, again) = (current.text().to_string(), new.text().to_string(), again.text().to_string());
                if let Some(problem) = problem(&new, &again) {
                    error.set_label(problem);
                    error.set_visible(true);
                    return;
                }
                error.set_visible(false);
                done.set_sensitive(false);
                let Some(rest) = this.account.state.borrow().rest.clone() else { return };
                let (dialog, finished, error, done) = (dialog.clone(), finished.clone(), error.clone(), done.clone());
                glib::spawn_future_local(async move {
                    match rest.update_password(&old, &new).await {
                        Ok(token) => {
                            finished.set(true);
                            dialog.force_close();
                            // The old token is no good: connect again with the new one.
                            this.logged_in(token);
                        }
                        Err(e) => {
                            error.set_label(&e.to_string());
                            error.set_visible(true);
                            done.set_sensitive(true);
                        }
                    }
                });
            }
        };
        done.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        again.connect_entry_activated(move |_| submit());
        dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade().filter(|_| !finished.get()) {
                    this.logout();
                }
            }
        });
        dialog.present(Some(&self.window));
        current.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::problem;

    #[test]
    fn the_form_checks_the_new_password_before_sending() {
        assert_eq!(problem("", ""), Some("Your new password cannot be empty."));
        assert_eq!(problem("a", "b"), Some("Passwords do not match!"));
        assert_eq!(problem("a", "a"), None);
    }
}
