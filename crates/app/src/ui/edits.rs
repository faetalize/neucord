//! Editing and deleting messages. An edit happens in the message box, which
//! puts its draft and reply aside meanwhile; a delete asks first, unless
//! Shift is held, as in the official client.

use std::rc::Rc;

use adw::prelude::*;
use discord::edits::{self, Edit};
use discord::{ChannelKind, Message, Rest, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::composer::ReplyCtx;
use super::friends::confirm;
use super::menus::{Entry, Item, Kind};
use crate::timelines;

pub(super) const EDIT: Entry<MessageTarget> = Entry::Item(Item {
    name: "edit",
    label: "_Edit Message",
    shows: |app, target| app.offers_edit(&target.message),
    kind: Kind::Action(|app, target| app.start_edit(&target.message, None)),
});

/// Shift deletes without asking.
pub(super) const DELETE: Entry<MessageTarget> = Entry::Item(Item {
    name: "delete",
    label: "_Delete Message",
    shows: |app, target| app.state.borrow().can_delete(&target.message),
    kind: Kind::Action(|app, target| match app.shift_held() {
        true => app.delete_message(&target.message),
        false => app.confirm_delete(&target.message, true),
    }),
});

/// The message the message box is editing.
pub(super) struct EditCtx {
    pub(super) message: Message,
    /// What the box held before, given back when the edit ends.
    draft: String,
    reply: Option<ReplyCtx>,
}

impl App {
    /// "Editing message" above the message box, with a way to cancel.
    pub(super) fn build_edit_bar(self: &Rc<Self>, area: &gtk::Box) {
        let label = gtk::Label::builder().label("Editing message").xalign(0.0).hexpand(true).build();
        let cancel = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Cancel edit")
            .css_classes(["flat"])
            .build();
        cancel.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.end_edit();
                }
            }
        });
        self.edit_bar.append(&label);
        self.edit_bar.append(&cancel);
        area.append(&self.edit_bar);
    }

    /// Whether Edit shows for `message`. Neucord edits in the message box,
    /// so not where that is hidden.
    pub(super) fn offers_edit(&self, message: &Message) -> bool {
        self.composer_box.is_visible() && self.state.borrow().can_edit(message)
    }

    /// The message being edited.
    pub(super) fn editing(&self) -> Option<Snowflake> {
        self.editing.borrow().as_ref().map(|e| e.message.id)
    }

    /// Edit `message` in the message box, starting from `text`, or from
    /// what it says.
    pub(super) fn start_edit(self: &Rc<Self>, message: &Message, text: Option<&str>) {
        if !self.offers_edit(message) {
            return;
        }
        self.end_edit();
        let buffer = self.composer.buffer();
        let draft = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into();
        let reply = self.reply.borrow().clone();
        self.set_reply(None);
        self.editing.replace(Some(EditCtx { message: message.clone(), draft, reply }));
        buffer.set_text(text.unwrap_or(&message.content));
        buffer.place_cursor(&buffer.end_iter());
        self.edit_bar.set_visible(true);
        self.refresh_message(message.id);
        self.composer.grab_focus();
    }

    /// Edit our last message in the open channel that can be, as Up in an
    /// empty message box does. Says whether there was one.
    pub(super) fn edit_last(self: &Rc<Self>) -> bool {
        let messages = self.messages();
        let last = (0..messages.n_items())
            .rev()
            .filter_map(|position| timelines::message_at(&messages, position))
            .find(|message| self.offers_edit(message));
        last.inspect(|message| self.start_edit(message, None)).is_some()
    }

    /// Stop editing, and give the message box back what it held before.
    pub(super) fn end_edit(self: &Rc<Self>) {
        let Some(edit) = self.editing.take() else { return };
        self.edit_bar.set_visible(false);
        self.composer.buffer().set_text(&edit.draft);
        // A reply started elsewhere stays there, as when switching channels.
        if edit.reply.as_ref().is_some_and(|r| Some(r.channel) == self.state.borrow().channel) {
            self.set_reply(edit.reply);
        }
        self.refresh_message(edit.message.id);
    }

    /// Save the edit with what the message box says, untrimmed (see
    /// [`edits::decide`]). Emptied, it offers to delete the message instead,
    /// and unchanged, it sends nothing, as in the official client.
    pub(super) fn save_edit(self: &Rc<Self>) {
        let Some(message) = self.editing.borrow().as_ref().map(|e| e.message.clone()) else { return };
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let bare = message.attachments.is_empty() && message.components.is_empty() && message.sticker_items.is_empty();
        let decision = edits::decide(&self.state.borrow().limits(), &text, &message.content, bare);
        // A refused edit stays open.
        let text = match decision {
            Edit::Refuse(too_long) => return self.show_too_long(too_long),
            Edit::Delete => {
                self.end_edit();
                return self.confirm_delete(&message, false);
            }
            Edit::Unchanged => {
                self.end_edit();
                return;
            }
            Edit::Save(text) => text.to_owned(),
        };
        self.end_edit();
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        // Our permissions may have changed since the edit started.
        if !self.state.borrow().can_edit(&message) {
            return;
        }
        let announced = self.announced(&message);
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The gateway echoes the edit back, which is what changes the row.
            let result = match announced && !this.confirm_announced_edit().await {
                true => Ok(()),
                false => match this.reopen(&rest, message.channel_id).await {
                    Ok(()) => rest.edit_message(&message, &text).await.map(|_| ()),
                    Err(e) => Err(e),
                },
            };
            if let Err(e) = &result {
                this.show_composer_error(&format!("Not edited: {e}"));
            }
            // Turned down or failed: back in the box, unless it's busy.
            let undone = result.is_err() || announced;
            if undone && this.editing().is_none() && this.state.borrow().channel == Some(message.channel_id) {
                this.start_edit(&message, Some(&text));
            }
        });
    }

    /// Ask before an edit that goes on to every server following an
    /// announcement channel.
    async fn confirm_announced_edit(&self) -> bool {
        let dialog = confirm(
            "Are you sure?",
            "This message will also be updated in all servers following this channel. \
                It may take some time before all servers are updated.",
            "_Confirm",
            adw::ResponseAppearance::Suggested,
        );
        dialog.choose_future(Some(&self.window)).await == "confirm"
    }

    /// Ask before deleting `message`; `hint` says Shift skips asking.
    pub(super) fn confirm_delete(self: &Rc<Self>, message: &Message, hint: bool) {
        let mut body = match self.announced(message) {
            true => {
                "This message will be deleted in all servers following this channel. \
                It may take some time before it's removed from all servers."
            }
            false => "Are you sure you want to delete this message?",
        }
        .to_owned();
        if hint {
            body.push_str("\n\nYou can hold down Shift when clicking Delete Message to skip this confirmation.");
        }
        let dialog = confirm("Delete Message", &body, "_Delete", adw::ResponseAppearance::Destructive);
        let (this, message) = (self.clone(), message.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "confirm" {
                this.delete_message(&message);
            }
        });
    }

    /// Delete `message`; the gateway then takes it off the list.
    pub(super) fn delete_message(self: &Rc<Self>, message: &Message) {
        let rest = self.state.borrow().rest.clone();
        // Our permissions may have changed since the menu opened.
        let Some(rest) = rest.filter(|_| self.state.borrow().can_delete(message)) else { return };
        let (channel, id) = (message.channel_id, message.id);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = match this.reopen(&rest, channel).await {
                Ok(()) => rest.delete_message(channel, id).await,
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                this.show_composer_error(&format!("Not deleted: {e}"));
            }
        });
    }

    /// Reopen `channel` first if it's a thread an edit or delete reopens.
    async fn reopen(&self, rest: &Rest, channel: Snowflake) -> Result<(), discord::Error> {
        let needed = self.state.borrow().unarchives_first(channel);
        if needed { rest.unarchive_thread(channel).await } else { Ok(()) }
    }

    /// Whether `message` was published to the servers following its
    /// announcement channel, so changing it changes every copy.
    fn announced(&self, message: &Message) -> bool {
        let state = self.state.borrow();
        let kind = state.find_channel(message.channel_id).map(|(_, c)| c.kind);
        message.is_crossposted() && kind == Some(ChannelKind::Announcement)
    }
}
