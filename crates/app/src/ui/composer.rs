//! The message box: sending, replying, and `@` mention autocomplete.

use std::rc::Rc;
use std::time::Duration;

use discord::cooldowns::{Slowmode, bypasses_slowmode, wait_words};
use discord::rest::Reply;
use discord::verification::{self, Gate};
use discord::{Channel, Message, Snowflake};
use gtk::glib;
use gtk::prelude::*;

use super::App;
use super::state::{now_ms, now_unix};

/// The message the composer is replying to.
#[derive(Clone)]
pub(super) struct ReplyCtx {
    pub(super) channel: Snowflake,
    pub(super) guild: Option<Snowflake>,
    pub(super) to: Snowflake,
    pub(super) author: String,
    /// Set where the ping can't be toggled, as in the official client:
    /// replies to yourself never ping, replies in DMs always do.
    pub(super) fixed_ping: Option<bool>,
}

#[derive(Default)]
pub(super) struct MentionCtx {
    /// Char offset of the `@` being completed.
    pub(super) start: i32,
    pub(super) candidates: Vec<(u64, String)>,
}

impl App {
    /// The message box floating under the list, with its mention popup and keys.
    pub(super) fn build_composer(self: &Rc<Self>, area: &gtk::Box) {
        area.append(&self.composer_error);
        area.append(&self.notice_bar);
        self.reply_bar.append(&self.reply_label);
        self.reply_bar.append(&self.reply_ping);
        self.reply_ping.connect_toggled(|button| {
            button.set_label(if button.is_active() { "@ ON" } else { "@ OFF" });
            button.set_tooltip_text(Some(if button.is_active() {
                "Click to disable pinging the original author."
            } else {
                "Click to enable pinging the original author."
            }));
        });
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
        area.append(&self.reply_bar);
        self.composer_box.append(&self.composer);
        area.append(&self.composer_box);

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

    /// What replaces the message box where we can't post: accept and ignore
    /// in a message request, a notice in the system DM, during a timeout, or
    /// while a server's verification asks for more. While slowmode makes us
    /// wait, the box stays and the wait shows above it.
    pub(super) fn update_notice_bar(self: &Rc<Self>) {
        let bar = &self.notice_bar;
        while let Some(child) = bar.first_child() {
            bar.remove(&child);
        }
        if let Some(tick) = self.notice_tick.take() {
            tick.remove();
        }
        let (system, request, timeout, gate, slowmode) = {
            let state = self.state.borrow();
            let open = state.channel.and_then(|id| state.find_channel(id));
            let system = open.is_some_and(|(_, c)| c.is_system_dm());
            let guild = open.and_then(|(guild, _)| guild);
            let timeout = guild.and_then(|g| g.timeout_left(now_unix()));
            let gate = guild.zip(state.account.as_ref()).and_then(|(g, a)| verification::gate(g, a, now_unix()));
            let slowmode = open.map_or(0, |(_, c)| state.slowmode.remaining_ms(c, now_ms()));
            (system, state.open_request().map(|c| (c.id, c.spam, c.title())), timeout, gate, slowmode)
        };
        let label = |text: String| gtk::Label::builder().label(text).xalign(0.0).wrap(true).hexpand(true).build();
        let text = match (&request, timeout) {
            _ if system => "This chat is reserved for official Discord notifications. \
                Discord will never ask for your password or token."
                .to_owned(),
            (_, Some(left)) => format!(
                "You're in timeout in this server for {}. Until then you can read, but not send messages or react.",
                wait_words(left)
            ),
            (Some((_, true, title)), None) => {
                format!("Discord filed this message request from {title} as likely spam.")
            }
            (Some((_, false, title)), None) => format!("{title} wants to message you. Accept to reply."),
            (None, None) => match &gate {
                Some(gate) => gate_notice(gate, now_unix()),
                None => {
                    self.composer_box.set_visible(true);
                    bar.set_visible(slowmode > 0);
                    if slowmode > 0 {
                        let seconds = (slowmode + 999) / 1000;
                        bar.append(&label(format!(
                            "Slowmode is enabled. You can send another message in this channel in {}.",
                            wait_words(seconds)
                        )));
                        self.tick_notice_bar();
                    }
                    return;
                }
            },
        };
        self.composer_box.set_visible(false);
        bar.set_visible(true);
        bar.append(&label(text));
        if timeout.is_some() || gate.as_ref().and_then(Gate::until).is_some() {
            self.tick_notice_bar();
        }
        if let Some((id, ..)) = request.filter(|_| !system) {
            for button in self.request_actions(id) {
                bar.append(&button);
            }
        }
    }

    /// Show the notice bar again in a second, to count its wait down and to
    /// give the message box back when the wait is over.
    fn tick_notice_bar(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        let tick = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(this) = this.upgrade() {
                // The source is spent; there is nothing left to remove.
                this.notice_tick.take();
                this.update_notice_bar();
            }
        });
        self.notice_tick.replace(Some(tick));
    }

    /// Reply to `message`. Like Discord, the reply pings unless `no_ping`
    /// (Shift held when clicking Reply).
    pub(super) fn start_reply(self: &Rc<Self>, message: &Message, no_ping: bool) {
        // Message requests and the system DM have no message box to reply from.
        if !self.composer_box.is_visible() {
            return;
        }
        let (own, guild) = {
            let state = self.state.borrow();
            let guild = state.find_channel(message.channel_id).and_then(|(guild, _)| guild.map(|g| g.id));
            (state.me() == Some(message.author.id), guild)
        };
        let fixed_ping = match (own, guild) {
            (true, _) => Some(false),
            (false, None) => Some(true),
            (false, Some(_)) => None,
        };
        self.reply_ping.set_active(!no_ping);
        self.set_reply(Some(ReplyCtx {
            channel: message.channel_id,
            guild,
            to: message.id,
            author: message.author.display_name().to_owned(),
            fixed_ping,
        }));
        self.composer.grab_focus();
    }

    /// Start or stop replying; the reply bar and the highlighted row follow.
    pub(super) fn set_reply(&self, reply: Option<ReplyCtx>) {
        if let Some(reply) = &reply {
            let name = glib::markup_escape_text(&reply.author);
            self.reply_label.set_markup(&format!("Replying to <b>{name}</b>"));
            self.reply_ping.set_visible(reply.fixed_ping.is_none());
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

    /// Milliseconds before we may post in `channel` again.
    fn slowmode_left(&self, channel: Snowflake) -> i64 {
        let state = self.state.borrow();
        state.find_channel(channel).map_or(0, |(_, c)| state.slowmode.remaining_ms(c, now_ms()))
    }

    /// Tell the slowmode tracker about a message to `channel`, then show the
    /// wait if that channel is still open. `record` is told whether we
    /// bypass slowmode there.
    fn record_slowmode(self: &Rc<Self>, channel: Snowflake, record: impl FnOnce(&mut Slowmode, &Channel, bool)) {
        let open = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let me = state.me();
            let Some((guild, c)) = state.find_channel(channel) else { return };
            let bypass = guild.zip(me).is_some_and(|(guild, me)| bypasses_slowmode(guild, me));
            let c = c.clone();
            record(&mut state.slowmode, &c, bypass);
            state.channel == Some(channel)
        };
        if open {
            self.update_notice_bar();
        }
    }

    pub(super) fn send_composer(self: &Rc<Self>) {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        let (channel, rest) = {
            let state = self.state.borrow();
            (state.channel, state.rest.clone())
        };
        let (Some(channel), Some(rest)) = (channel, rest) else { return };
        // A hidden message box is a channel we must never post to (#51).
        if text.is_empty() || !self.composer_box.is_visible() {
            return;
        }
        // Slowmode would refuse it; the notice bar says how long is left.
        if self.slowmode_left(channel) > 0 {
            return self.update_notice_bar();
        }
        let replying = self.reply.borrow().clone().filter(|r| r.channel == channel);
        let reply = replying.as_ref().map(|r| Reply {
            to: r.to,
            guild: r.guild,
            ping: r.fixed_ping.unwrap_or(self.reply_ping.is_active()),
        });
        let ping = self.reply_ping.is_active();
        self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
        buffer.set_text("");
        self.set_reply(None);
        self.mention_popover.popdown();
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The gateway echoes our own message back, which is what adds it to the list.
            match rest.send_message(channel, &text, reply).await {
                Ok(_) => {
                    this.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
                    // Sending from an old stretch of history returns to the present.
                    if this.timelines.borrow().get(channel).is_some_and(|t| t.detached) {
                        this.load_newest(channel);
                    }
                }
                Err(e) => {
                    match e.slowmode_wait() {
                        Some(wait) => {
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.refused(c, wait, now_ms()));
                            this.show_composer_error("Not sent: slowmode is enabled in this channel.");
                        }
                        None => {
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                            this.show_composer_error(&format!("Not sent: {e}"));
                        }
                    }
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

/// The official client's words for what a server's verification still asks
/// of us. The gates that only take time count down, as the client's do.
fn gate_notice(gate: &Gate, now_unix: i64) -> String {
    let text = gate.text();
    match gate.until() {
        Some(until) => format!("{text} You can send messages here in {}.", wait_words(until - now_unix)),
        None if *gate == Gate::Screening => format!("{text} Accept the server's rules in the official app."),
        None => text,
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
