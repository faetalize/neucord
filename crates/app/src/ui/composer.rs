//! The message box: sending, replying, and `@` mention autocomplete.

use std::rc::Rc;
use std::time::Duration;

use discord::rest::Reply;
use discord::{Message, Snowflake};
use gtk::glib;
use gtk::prelude::*;

use super::App;

/// The message the composer is replying to.
#[derive(Clone)]
pub(super) struct ReplyCtx {
    pub(super) channel: Snowflake,
    pub(super) to: Snowflake,
    pub(super) author: String,
    /// Our own message: there is nobody to ping.
    pub(super) own: bool,
}

#[derive(Default)]
pub(super) struct MentionCtx {
    /// Char offset of the `@` being completed.
    pub(super) start: i32,
    pub(super) candidates: Vec<(u64, String)>,
}

impl App {
    /// The message box under the list, with its mention popup and keys.
    pub(super) fn build_composer(self: &Rc<Self>, chat: &gtk::Box) {
        chat.append(&self.composer_error);
        chat.append(&self.request_bar);
        self.reply_bar.append(&self.reply_label);
        self.reply_bar.append(&self.reply_ping);
        self.reply_ping.connect_toggled(|button| button.set_label(if button.is_active() { "@ On" } else { "@ Off" }));
        let cancel = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Cancel reply")
            .css_classes(["flat"])
            .build();
        cancel.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.set_reply(None);
                }
            }
        });
        self.reply_bar.append(&cancel);
        chat.append(&self.reply_bar);
        self.composer_box.append(&self.composer);
        chat.append(&self.composer_box);

        // Mention autocomplete popup, anchored to the composer.
        self.mention_popover.set_child(Some(&self.mention_list));
        self.mention_popover.set_parent(&self.composer);
        self.mention_list.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                if let Some(this) = this.upgrade() {
                    this.accept_mention(row.index());
                }
            }
        });
        self.composer.buffer().connect_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.update_mention_popup();
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let this = Rc::downgrade(self);
            move |_, key, _, modifiers| {
                let Some(this) = this.upgrade() else { return glib::Propagation::Proceed };
                use gtk::gdk::Key;
                // While the mention popup is open, it owns the navigation keys.
                if this.mention_popover.is_visible() {
                    match key {
                        Key::Escape => {
                            this.mention_popover.popdown();
                            return glib::Propagation::Stop;
                        }
                        Key::Down | Key::Tab => {
                            this.move_mention_selection(1);
                            return glib::Propagation::Stop;
                        }
                        Key::Up => {
                            this.move_mention_selection(-1);
                            return glib::Propagation::Stop;
                        }
                        Key::Return | Key::KP_Enter => {
                            let index = this.mention_list.selected_row().map(|r| r.index()).unwrap_or(0);
                            this.accept_mention(index);
                            return glib::Propagation::Stop;
                        }
                        _ => {}
                    }
                }
                if key == Key::Escape && this.reply.borrow().is_some() {
                    this.set_reply(None);
                    return glib::Propagation::Stop;
                }
                let enter = matches!(key, Key::Return | Key::KP_Enter);
                if enter && !modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    this.send_composer();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        self.composer.add_controller(keys);
    }

    pub(super) fn start_reply(self: &Rc<Self>, message: &Message) {
        let own = self.state.borrow().me() == Some(message.author.id);
        self.reply_ping.set_active(true);
        self.set_reply(Some(ReplyCtx {
            channel: message.channel_id,
            to: message.id,
            author: message.author.display_name().to_owned(),
            own,
        }));
        self.composer.grab_focus();
    }

    /// Start or stop replying; the reply bar and the highlighted row follow.
    pub(super) fn set_reply(&self, reply: Option<ReplyCtx>) {
        if let Some(reply) = &reply {
            let name = glib::markup_escape_text(&reply.author);
            self.reply_label.set_markup(&format!("Replying to <b>{name}</b>"));
            self.reply_ping.set_visible(!reply.own);
        }
        self.reply_bar.set_visible(reply.is_some());
        let previous = self.reply.replace(reply.clone());
        for id in previous.iter().chain(&reply).map(|r| r.to) {
            self.refresh_message(id);
        }
    }

    pub(super) fn show_composer_error(&self, text: &str) {
        self.composer_error.set_label(text);
        self.composer_error.set_visible(true);
        let label = self.composer_error.downgrade();
        let shown = text.to_owned();
        glib::timeout_add_local_once(Duration::from_secs(8), move || {
            if let Some(label) = label.upgrade()
                && label.label() == shown
            {
                label.set_visible(false);
            }
        });
    }

    /// Show or hide the mention popup based on the `@word` at the cursor.
    pub(super) fn update_mention_popup(self: &Rc<Self>) {
        let Some((start, prefix)) = mention_token(&self.composer.buffer()) else {
            self.mention_popover.popdown();
            return;
        };
        let prefix = prefix.to_lowercase();
        let candidates: Vec<(u64, String)> = {
            let state = self.state.borrow();
            let mut matches: Vec<(u64, String)> = state
                .users
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&prefix))
                .map(|(id, name)| (*id, name.clone()))
                .collect();
            // Prefix matches first, then alphabetical; cap the list.
            matches.sort_by(|a, b| {
                let rank = |n: &str| if n.to_lowercase().starts_with(&prefix) { 0 } else { 1 };
                rank(&a.1).cmp(&rank(&b.1)).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
            matches.truncate(8);
            matches
        };
        if candidates.is_empty() {
            self.mention_popover.popdown();
            return;
        }

        while let Some(child) = self.mention_list.first_child() {
            self.mention_list.remove(&child);
        }
        for (_, name) in &candidates {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&gtk::Label::builder().label(format!("@{name}")).xalign(0.0).build()));
            self.mention_list.append(&row);
        }
        self.mention_list.select_row(self.mention_list.row_at_index(0).as_ref());
        *self.mention.borrow_mut() = MentionCtx { start, candidates };

        // Point at the caret so the popup tracks where you are typing.
        if let Some(rect) = self.caret_rect() {
            self.mention_popover.set_pointing_to(Some(&rect));
        }
        self.mention_popover.popup();
    }

    pub(super) fn move_mention_selection(&self, delta: i32) {
        let count = self.mention.borrow().candidates.len() as i32;
        if count == 0 {
            return;
        }
        let current = self.mention_list.selected_row().map(|r| r.index()).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        self.mention_list.select_row(self.mention_list.row_at_index(next).as_ref());
    }

    pub(super) fn accept_mention(self: &Rc<Self>, index: i32) {
        let (start, id) = {
            let ctx = self.mention.borrow();
            match ctx.candidates.get(index.max(0) as usize) {
                Some((id, _)) => (ctx.start, *id),
                None => return,
            }
        };
        let buffer = self.composer.buffer();
        let cursor = buffer.property::<i32>("cursor-position");
        let mut from = buffer.iter_at_offset(start);
        let mut to = buffer.iter_at_offset(cursor);
        buffer.delete(&mut from, &mut to);
        let mut at = buffer.iter_at_offset(start);
        buffer.insert(&mut at, &format!("<@{id}> "));
        self.mention_popover.popdown();
        self.composer.grab_focus();
    }

    /// The caret's rectangle in composer coordinates, for popover anchoring.
    pub(super) fn caret_rect(&self) -> Option<gtk::gdk::Rectangle> {
        let buffer = self.composer.buffer();
        let iter = buffer.iter_at_offset(buffer.property::<i32>("cursor-position"));
        let strong = self.composer.cursor_locations(Some(&iter)).0;
        let (x, y) = self.composer.buffer_to_window_coords(gtk::TextWindowType::Widget, strong.x(), strong.y());
        Some(gtk::gdk::Rectangle::new(x, y, 1, strong.height()))
    }

    pub(super) fn send_composer(self: &Rc<Self>) {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        let (channel, rest) = {
            let state = self.state.borrow();
            (state.channel, state.rest.clone())
        };
        let (Some(channel), Some(rest)) = (channel, rest) else { return };
        if text.is_empty() {
            return;
        }
        let replying = self.reply.borrow().clone().filter(|r| r.channel == channel);
        let reply = replying.as_ref().map(|r| Reply { to: r.to, ping: !r.own && self.reply_ping.is_active() });
        let ping = self.reply_ping.is_active();
        buffer.set_text("");
        self.set_reply(None);
        self.mention_popover.popdown();
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The gateway echoes our own message back, which is what adds it to the list.
            match rest.send_message(channel, &text, reply).await {
                Ok(_) => {
                    // Sending from an old stretch of history returns to the present.
                    if this.timelines.borrow().get(channel).is_some_and(|t| t.detached) {
                        this.load_newest(channel);
                    }
                }
                Err(e) => {
                    this.show_composer_error(&format!("Not sent: {e}"));
                    let buffer = this.composer.buffer();
                    if buffer.char_count() == 0 && this.reply.borrow().is_none() {
                        buffer.set_text(&text);
                        if this.state.borrow().channel == Some(channel) {
                            this.set_reply(replying);
                            this.reply_ping.set_active(ping);
                        }
                    }
                }
            }
        });
    }
}

/// If the cursor sits just after an `@word` token, return the `@`'s char offset
/// and the word typed so far. The `@` must start a word (start of buffer or
/// after whitespace), and the token must contain no whitespace.
pub(super) fn mention_token(buffer: &gtk::TextBuffer) -> Option<(i32, String)> {
    let cursor = buffer.property::<i32>("cursor-position");
    let text: String = buffer.text(&buffer.start_iter(), &buffer.iter_at_offset(cursor), false).into();
    let chars: Vec<char> = text.chars().collect();
    let at = chars.iter().rposition(|&c| c == '@')?;
    // The word between `@` and the cursor must have no whitespace.
    if chars[at + 1..].iter().any(|c| c.is_whitespace()) {
        return None;
    }
    // `@` must begin a word.
    if at > 0 && !chars[at - 1].is_whitespace() {
        return None;
    }
    Some((at as i32, chars[at + 1..].iter().collect()))
}
