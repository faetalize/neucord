//! Pin Message and Unpin Message in a message's menu (979766): each asks
//! first, unless Shift is held, as the official client does.

use std::rc::Rc;

use adw::prelude::*;
use discord::{Message, Snowflake, pins};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::friends::confirm;
use super::menus::{Entry, Item, Kind};

pub(super) const PIN: Entry<MessageTarget> = Entry::Item(Item {
    name: "pin",
    label: "_Pin Message",
    shows: |app, target| !target.message.pinned && app.account.state.borrow().can_pin(&target.message),
    kind: Kind::Action(|app, target| match app.shift_held() {
        true => app.pin_message(&target.message),
        false => app.confirm_pin(&target.message),
    }),
});

pub(super) const UNPIN: Entry<MessageTarget> = Entry::Item(Item {
    name: "unpin",
    label: "Unpin _Message",
    shows: |app, target| target.message.pinned && app.account.state.borrow().can_pin(&target.message),
    kind: Kind::Action(|app, target| match app.shift_held() {
        true => app.unpin_message(&target.message),
        false => app.confirm_unpin(&target.message),
    }),
});

impl App {
    /// The channel's name for a message, and whether it's a DM.
    fn pin_place(&self, channel: Snowflake) -> (String, bool) {
        let state = self.account.state.borrow();
        let found = state.find_channel(channel);
        let private = found.is_some_and(|(guild, _)| guild.is_none());
        (found.map(|(_, c)| c.title()).unwrap_or_default(), private)
    }

    fn confirm_pin(self: &Rc<Self>, message: &Message) {
        let (name, private) = self.pin_place(message.channel_id);
        let (heading, body, verb) = pins::pin_question(private, &name);
        let dialog = confirm(heading, &body, verb, adw::ResponseAppearance::Suggested);
        let (this, message) = (self.clone(), message.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "confirm" {
                this.pin_message(&message);
            }
        });
    }

    fn confirm_unpin(self: &Rc<Self>, message: &Message) {
        let (heading, body, verb, hint) = pins::UNPIN_QUESTION;
        let dialog = confirm(heading, &format!("{body}\n\n{hint}"), verb, adw::ResponseAppearance::Destructive);
        let (this, message) = (self.clone(), message.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "confirm" {
                this.unpin_message(&message);
            }
        });
    }

    /// Pin `message`; the gateway then marks it. An archived thread is
    /// reopened first, and a refusal is an alert saying why.
    fn pin_message(self: &Rc<Self>, message: &Message) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let (channel, id) = (message.channel_id, message.id);
        let (name, private) = self.pin_place(channel);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = match this.reopen(&rest, channel).await {
                Ok(()) => rest.pin_message(channel, id).await,
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                let said = e.to_string();
                let alert = pins::refused(e.code(), private, &name, Some(&said));
                let dialog = adw::AlertDialog::new(Some(&alert.title), Some(&alert.body));
                dialog.add_response("ok", "_Okay");
                dialog.choose_future(Some(&this.window)).await;
            }
        });
    }

    /// Unpin `message`; a failure offers to try again.
    fn unpin_message(self: &Rc<Self>, message: &Message) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let (channel, id) = (message.channel_id, message.id);
        let this = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let result = match this.reopen(&rest, channel).await {
                    Ok(()) => rest.unpin_message(channel, id).await,
                    Err(e) => Err(e),
                };
                if result.is_ok() {
                    return;
                }
                let (title, body) = pins::UNPIN_FAILED;
                let dialog = adw::AlertDialog::new(Some(title), Some(body));
                dialog.add_responses(&[("cancel", "_Cancel"), ("retry", "_Try Again")]);
                dialog.set_close_response("cancel");
                if dialog.choose_future(Some(&this.window)).await != "retry" {
                    return;
                }
            }
        });
    }
}
