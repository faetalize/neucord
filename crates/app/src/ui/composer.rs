//! The message box: sending, replying, and autocomplete for `@` mentions and
//! `:emoji:`. Editing happens here too, in `edits`.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::cooldowns::{Slowmode, bypasses_slowmode, wait_words};
use discord::emoji::{Intention, Item as Emoji, locked_notice};
use discord::limits::{LengthCount, TooLong, message_length};
use discord::notice::Notice;
use discord::rest::{Reply, ReplyPing};
use discord::scheduled;
use discord::verification::Gate;
use discord::{Channel, Message, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::menus::{self, Entry, Item, Kind};
use super::state::{now_ms, now_unix};
use super::uploads::{Outgoing, Stopped};

/// Reply to a message from its menu; Shift turns the ping off, as with the
/// button.
pub(super) const REPLY: Entry<MessageTarget> = Entry::Item(Item {
    name: "reply",
    label: "_Reply",
    shows: |app, target| app.offers_reply(&target.message),
    kind: Kind::Action(|app, target| app.start_reply(&target.message, app.shift_held())),
});

/// The message the composer is replying to.
#[derive(Clone)]
pub(super) struct ReplyCtx {
    pub(super) channel: Snowflake,
    pub(super) guild: Option<Snowflake>,
    pub(super) to: Snowflake,
    pub(super) author: String,
    /// Set where the ping can't be toggled, as in the official client:
    /// replies to yourself never ping, and in DMs the click alone decides.
    pub(super) fixed_ping: Option<bool>,
}

/// How long the message box's text is against the limit, in its corner.
pub(super) struct LengthIndicator {
    root: gtk::Box,
    icon: gtk::Image,
    number: gtk::Label,
}

impl LengthIndicator {
    pub(super) fn new() -> Self {
        let icon = gtk::Image::from_icon_name("starred-symbolic");
        let number = gtk::Label::builder().css_classes(["numeric"]).build();
        let root = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).visible(false).build();
        root.append(&icon);
        root.append(&number);
        Self { root, icon, number }
    }

    pub(super) fn show(&self, count: Option<LengthCount>) {
        self.root.set_visible(count.is_some());
        let Some(count) = count else { return };
        self.icon.set_visible(count.powered);
        self.icon.set_tooltip_text(Some(&count.powered_tooltip()));
        self.number.set_visible(count.number);
        self.number.set_label(&count.remaining.to_string());
        self.number.set_tooltip_text(Some(&count.tooltip()));
        if count.remaining < 0 {
            self.number.add_css_class("error");
        } else {
            self.number.remove_css_class("error");
        }
    }
}

/// What accepting a row of the autocomplete popup writes.
pub(super) enum Completion {
    Mention(u64, String),
    Emoji(Emoji),
    /// A command to fill in the form of, not text to insert.
    Command(super::slash::Suggestion),
}

impl Completion {
    /// The text that replaces the token, with the space that follows it.
    fn text(&self) -> String {
        match self {
            Completion::Mention(id, _) => format!("<@{id}> "),
            Completion::Emoji(item) => format!("{} ", item.text()),
            Completion::Command(_) => String::new(),
        }
    }

    fn row(&self, images: &crate::images::Images) -> gtk::Widget {
        match self {
            Completion::Mention(_, name) => {
                gtk::Label::builder().label(format!("@{name}")).xalign(0.0).build().upcast()
            }
            Completion::Emoji(item) => {
                let line = gtk::Box::builder().spacing(8).build();
                line.append(&super::emoji::glyph(item, images));
                line.append(&gtk::Label::builder().label(format!(":{}:", item.name())).xalign(0.0).build());
                line.upcast()
            }
            Completion::Command(suggestion) => {
                let line = gtk::Box::builder().spacing(12).build();
                line.append(&gtk::Label::builder().label(&suggestion.name).xalign(0.0).build());
                line.append(
                    &gtk::Label::builder()
                        .label(&suggestion.description)
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .width_chars(30)
                        .max_width_chars(48)
                        .css_classes(["dim-label"])
                        .build(),
                );
                line.append(&gtk::Label::builder().label(&suggestion.app).css_classes(["dim-label"]).build());
                line.upcast()
            }
        }
    }
}

#[derive(Default)]
pub(super) struct MentionCtx {
    /// Char offset of the `@` or `:` being completed.
    pub(super) start: i32,
    pub(super) candidates: Vec<Completion>,
}

impl App {
    /// The message box floating under the list, with its mention popup and keys.
    pub(super) fn build_composer(self: &Rc<Self>, area: &gtk::Box) {
        menus::extend(self, &self.composer, menus::COMPOSER, || ());
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
        self.build_schedule_bar(area);
        self.build_edit_bar(area);
        self.build_slash(area);
        self.build_attach(area);
        self.build_stickers(area);
        self.build_gifs();
        self.composer_box.append(&self.composer);
        self.composer_box.append(&self.gif_button);
        self.composer_box.append(&self.sticker_button);
        self.composer_box.append(&self.emoji_button);
        self.composer_box.append(&self.length.root);
        self.emoji_button.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                let Some(this) = this.upgrade() else { return };
                let Some(channel) = this.account.state.borrow().channel else { return };
                this.open_emoji_picker(channel, button, Intention::Chat, |app, item| app.insert_emoji(item));
            }
        });
        self.build_voice();
        area.append(&self.composer_box);
        area.append(&self.typing_line);

        // Autocomplete popup, anchored to the composer.
        let scroller = gtk::ScrolledWindow::builder()
            .child(&self.mention_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .build();
        self.mention_popover.set_child(Some(&scroller));
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
                    this.update_length_indicator();
                    this.composer_typed();
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
                if key == Key::Escape && this.editing().is_some() {
                    this.end_edit();
                    return glib::Propagation::Stop;
                }
                // Up alone in an empty box edits our last message, as in the official client.
                let alone = !modifiers.intersects(gtk::accelerator_get_default_mod_mask());
                if key == Key::Up && alone && this.composer.buffer().char_count() == 0 && this.edit_last() {
                    return glib::Propagation::Stop;
                }
                if key == Key::Escape && this.reply.borrow().is_some() {
                    this.set_reply(None);
                    return glib::Propagation::Stop;
                }
                // The scheduled time goes after the reply, and a waiting sticker after it (92960).
                let channel = this.account.state.borrow().channel;
                if key == Key::Escape
                    && let Some(channel) = channel.filter(|c| this.scheduled_time(*c).is_some())
                {
                    this.clear_schedule(channel);
                    return glib::Propagation::Stop;
                }
                if key == Key::Escape
                    && let Some(channel) = channel.filter(|c| this.waiting.of(*c).is_some())
                {
                    this.set_waiting_sticker(channel, None);
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
    /// in a message request, a notice in the system DM, during a timeout,
    /// while a server's verification asks for more, or without Send Messages
    /// (in a thread, by the thread rule: see `permissions::in_channel`). The
    /// last keeps the attach button for Create Thread (see [`Notice`]). While
    /// slowmode makes us wait, the box stays and the wait shows above it.
    pub(super) fn update_notice_bar(self: &Rc<Self>) {
        self.watch_timeout_end();
        self.show_notice_bar();
        self.refresh_attach();
    }

    /// Look at permissions again when our timeout in the open server ends:
    /// the countdown only redraws the notice bar, and administrators don't
    /// get one, so Reply would stay off the rows on screen. Not armed from
    /// the countdown, whose last tick would cancel this just as it's due.
    fn watch_timeout_end(self: &Rc<Self>) {
        if let Some(timer) = self.timeout_end.take() {
            timer.remove();
        }
        let timeout = {
            let state = self.account.state.borrow();
            let guild = state.channel.and_then(|id| state.find_channel(id)).and_then(|(guild, _)| guild);
            guild.and_then(|g| Some((g.id, g.timed_out_until?)))
        };
        let Some((guild, until)) = timeout.filter(|&(_, until)| until > now_unix()) else { return };
        let delay = Duration::from_millis((until * 1000 - now_ms()).max(0) as u64 + 1);
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(delay, move || {
            if let Some(this) = this.upgrade() {
                // The source is spent; there is nothing left to remove.
                this.timeout_end.take();
                this.permissions_changed(Some(guild));
            }
        });
        self.timeout_end.replace(Some(timer));
    }

    fn show_notice_bar(self: &Rc<Self>) {
        let bar = &self.notice_bar;
        while let Some(child) = bar.first_child() {
            bar.remove(&child);
        }
        if let Some(tick) = self.notice_tick.take() {
            tick.remove();
        }
        // A forum or media channel has its own page and never a box; the
        // send guard below trusts the box's visibility, not the page's.
        let forum = {
            let state = self.account.state.borrow();
            state.channel.and_then(|id| state.find_channel(id)).is_some_and(|(_, c)| c.kind.is_forum_like())
        };
        if forum {
            self.show_composer(false);
            bar.set_visible(false);
            return;
        }
        let (notice, request, denied_here, slowmode) = {
            let state = self.account.state.borrow();
            let open = state.channel.and_then(|id| state.find_channel(id));
            let denied_here = match open.map(|(_, c)| c) {
                Some(c) if c.kind == discord::ChannelKind::MediaThread => "This thread can only be read.",
                Some(c) if c.is_locked() => "This thread is locked. Only moderators can send messages in it.",
                _ => "You do not have permission to send messages in this channel.",
            };
            let slowmode = open.map_or(0, |(_, c)| state.slowmode.remaining_ms(c, now_ms()));
            let request = state.open_request().map(|c| (c.id, c.title()));
            (state.channel.and_then(|id| state.notice(id)), request, denied_here, slowmode)
        };
        let label = |text: String| gtk::Label::builder().label(text).xalign(0.0).wrap(true).hexpand(true).build();
        let request = request.as_ref().map(|(id, title)| (*id, title.as_str()));
        let text = match &notice {
            Some(Notice::System) => "This chat is reserved for official Discord notifications. \
                Discord will never ask for your password or token."
                .to_owned(),
            Some(Notice::Timeout(left)) => format!(
                "You're in timeout in this server for {}. Until then you can read, but not send messages or react.",
                wait_words(*left)
            ),
            Some(Notice::Request { spam: true }) => {
                format!("Discord filed this message request from {} as likely spam.", request.map_or("", |r| r.1))
            }
            Some(Notice::Request { spam: false }) => {
                format!("{} wants to message you. Accept to reply.", request.map_or("", |r| r.1))
            }
            Some(Notice::Gate(gate)) => gate_notice(gate, now_unix()),
            Some(Notice::Denied) => denied_here.to_owned(),
            None => {
                self.show_composer(true);
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
        };
        self.show_composer(false);
        bar.set_visible(true);
        let thread = {
            let state = self.account.state.borrow();
            state.channel.is_some_and(|id| state.offers_thread_in_attach_menu(id))
        };
        if thread && notice.as_ref().is_some_and(Notice::keeps_attach_button) {
            bar.append(&self.drafts.notice_attach);
        }
        bar.append(&label(text));
        let counting = match &notice {
            Some(Notice::Timeout(_)) => true,
            Some(Notice::Gate(gate)) => gate.until().is_some(),
            _ => false,
        };
        if counting {
            self.tick_notice_bar();
        }
        if let (Some(Notice::Request { .. }), Some((id, _))) = (&notice, request) {
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
                this.show_notice_bar();
            }
        });
        self.notice_tick.replace(Some(tick));
    }

    /// Show or hide the message box. Hiding it ends an edit, and the rows
    /// on screen follow, since Edit is only offered with the box.
    fn show_composer(self: &Rc<Self>, shown: bool) {
        if self.composer_box.is_visible() == shown {
            return;
        }
        self.composer_box.set_visible(shown);
        if !shown {
            self.end_edit();
        }
        self.refresh_attach();
        let messages = self.messages();
        self.rebind(&messages, 0, messages.n_items());
    }

    /// Whether Reply shows for `message`: what the official client replies
    /// to, where it lets us.
    pub(super) fn offers_reply(&self, message: &Message) -> bool {
        message.is_replyable() && self.account.state.borrow().can_reply(message.channel_id)
    }

    /// Reply to `message`. Like Discord, the reply pings unless `no_ping`
    /// (Shift held when clicking Reply).
    pub(super) fn start_reply(self: &Rc<Self>, message: &Message, no_ping: bool) {
        // Message requests and the system DM have no message box to reply from.
        if !self.composer_box.is_visible() || !self.account.state.borrow().can_reply(message.channel_id) {
            return;
        }
        self.end_edit();
        let (own, guild) = {
            let state = self.account.state.borrow();
            let guild = state.find_channel(message.channel_id).and_then(|(guild, _)| guild.map(|g| g.id));
            (state.me() == Some(message.author.id), guild)
        };
        let ping = ReplyPing::for_click(own, guild.is_none(), no_ping);
        let fixed_ping = ping.fixed();
        self.reply_ping.set_active(ping.ping);
        self.set_reply(Some(ReplyCtx {
            channel: message.channel_id,
            guild,
            to: message.id,
            author: self.account.state.borrow().name_in(message.channel_id, &message.author),
            fixed_ping,
        }));
        self.composer.grab_focus();
    }

    /// Start or stop replying; the reply bar and the highlighted row follow.
    pub(super) fn set_reply(self: &Rc<Self>, reply: Option<ReplyCtx>) {
        if let Some(reply) = &reply {
            let name = glib::markup_escape_text(&reply.author);
            self.reply_label.set_markup(&format!("Replying to <b>{name}</b>"));
            self.reply_ping.set_visible(reply.fixed_ping.is_none());
        }
        self.reply_bar.set_visible(reply.is_some());
        let previous = self.reply.replace(reply.clone());
        // Use Apps in the attach menu shows only where nothing is replied to.
        self.refresh_attach();
        for id in previous.iter().chain(&reply).map(|r| r.to) {
            self.refresh_message(id);
        }
        self.reply_changed_slash();
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

    /// Draw the length indicator for what the box holds now.
    pub(super) fn update_length_indicator(&self) {
        let buffer = self.composer.buffer();
        let length = message_length(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));
        let count = self.account.state.borrow().limits().length_count(length, self.editing().is_some());
        self.length.show(count);
    }

    /// Tell the user a message is too long, and keep it in the box.
    pub(super) fn show_too_long(self: &Rc<Self>, too_long: TooLong) {
        let dialog = adw::AlertDialog::new(Some(too_long.heading()), Some(&too_long.body()));
        dialog.add_response("okay", "_Okay");
        let this = self.clone();
        glib::spawn_future_local(async move {
            dialog.choose_future(Some(&this.window)).await;
            this.composer.grab_focus();
        });
    }

    /// Show or hide the autocomplete popup for the `@word` or `:word` at the
    /// cursor.
    pub(super) fn update_mention_popup(self: &Rc<Self>) {
        // A `/` at the start asks for a command instead.
        if self.update_slash_popup() {
            return;
        }
        let buffer = self.composer.buffer();
        let found = match mention_token(&buffer) {
            Some((start, prefix)) => Some((start, self.mention_candidates(&prefix))),
            None => emoji_token(&buffer).map(|(start, query)| (start, self.emoji_candidates(&query))),
        };
        match found {
            Some((start, candidates)) => self.show_completions(start, candidates),
            None => self.mention_popover.popdown(),
        }
    }

    /// Show `candidates` in the popup, for the token starting at `start`.
    pub(super) fn show_completions(self: &Rc<Self>, start: i32, candidates: Vec<Completion>) {
        if candidates.is_empty() {
            self.mention_popover.popdown();
            return;
        }

        while let Some(child) = self.mention_list.first_child() {
            self.mention_list.remove(&child);
        }
        for candidate in &candidates {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&candidate.row(&self.images)));
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

    fn mention_candidates(&self, prefix: &str) -> Vec<Completion> {
        let prefix = prefix.to_lowercase();
        let state = self.account.state.borrow();
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
        matches.into_iter().map(|(id, name)| Completion::Mention(id, name)).collect()
    }

    /// The emoji we may send in the open channel that `query` finds, best
    /// first by how much the account uses them, in its skin tone. Locked
    /// ones aren't offered here, as in the client. Looking for emoji loads
    /// the counts of use (`queryEmojiResults`, 997625), which rank them once
    /// they are there.
    fn emoji_candidates(self: &Rc<Self>, query: &str) -> Vec<Completion> {
        let Some(channel) = self.account.state.borrow().channel else { return Vec::new() };
        self.load_frecency(|| {});
        let scores = self.emoji_scores(Intention::Chat);
        let tone = self.account.state.borrow().settings.skin_tone;
        let found = self.emoji_library(channel).search(query, super::emoji::AUTOCOMPLETE, &scores);
        found.unlocked.into_iter().map(|item| Completion::Emoji(super::emoji::toned(item, tone))).collect()
    }

    /// Put `item` in the message box at the cursor, as the picker chooses it.
    pub(super) fn insert_emoji(&self, item: &Emoji) {
        self.composer.buffer().insert_at_cursor(&item.text());
        // The popover hands focus back to the button as it closes.
        let composer = self.composer.clone();
        glib::idle_add_local_once(move || {
            composer.grab_focus();
        });
    }

    pub(super) fn move_mention_selection(&self, delta: i32) {
        let count = self.mention.borrow().candidates.len() as i32;
        if count == 0 {
            return;
        }
        let current = self.mention_list.selected_row().map(|r| r.index()).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        let row = self.mention_list.row_at_index(next);
        self.mention_list.select_row(row.as_ref());
        // The list scrolls, so the selection must stay in view.
        if let (Some(row), Some(scroller)) =
            (row, self.mention_list.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>())
            && let Some(bounds) = row.compute_bounds(&self.mention_list)
        {
            let adjustment = scroller.vadjustment();
            let (top, bottom) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
            if top < adjustment.value() {
                adjustment.set_value(top);
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
        }
    }

    pub(super) fn accept_mention(self: &Rc<Self>, index: i32) {
        let command = match self.mention.borrow().candidates.get(index.max(0) as usize) {
            Some(Completion::Command(suggestion)) => Some(suggestion.id.clone()),
            _ => None,
        };
        if let Some(id) = command {
            return self.start_command(&id);
        }
        let (start, text) = {
            let ctx = self.mention.borrow();
            match ctx.candidates.get(index.max(0) as usize) {
                Some(candidate) => (ctx.start, candidate.text()),
                None => return,
            }
        };
        let buffer = self.composer.buffer();
        let cursor = buffer.property::<i32>("cursor-position");
        let mut from = buffer.iter_at_offset(start);
        let mut to = buffer.iter_at_offset(cursor);
        buffer.delete(&mut from, &mut to);
        let mut at = buffer.iter_at_offset(start);
        buffer.insert(&mut at, &text);
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
    pub(super) fn slowmode_left(&self, channel: Snowflake) -> i64 {
        let state = self.account.state.borrow();
        state.find_channel(channel).map_or(0, |(_, c)| state.slowmode.remaining_ms(c, now_ms()))
    }

    /// Tell the slowmode tracker about a message to `channel`, then show the
    /// wait if that channel is still open. `record` is told whether we
    /// bypass slowmode there.
    pub(super) fn record_slowmode(
        self: &Rc<Self>,
        channel: Snowflake,
        record: impl FnOnce(&mut Slowmode, &Channel, bool),
    ) {
        let open = {
            let mut state = self.account.state.borrow_mut();
            let state = &mut *state;
            let Some((guild, c)) = state.find_channel(channel) else { return };
            let bypass =
                guild.zip(state.account.as_ref()).is_some_and(|(g, me)| bypasses_slowmode(g, c, me, now_unix()));
            let c = c.clone();
            record(&mut state.slowmode, &c, bypass);
            state.channel == Some(channel)
        };
        if open {
            self.update_notice_bar();
        }
    }

    pub(super) fn send_composer(self: &Rc<Self>) {
        if self.editing().is_some() {
            return self.save_edit();
        }
        let buffer = self.composer.buffer();
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        let (channel, rest) = {
            let state = self.account.state.borrow();
            (state.channel, state.rest.clone())
        };
        let (Some(channel), Some(rest)) = (channel, rest) else { return };
        let files = self.files_for_message(channel);
        let waiting = self.waiting.of(channel);
        // A hidden message box is a channel we must never post to (#51).
        if (typed.is_empty() && files.is_empty() && waiting.is_none()) || !self.composer_box.is_visible() {
            return;
        }
        // The box follows our permissions, but they may have changed since.
        if !self.account.state.borrow().can_send(channel) {
            return self.update_notice_bar();
        }
        // What was true when the sticker was picked may not be now.
        if let Some(listed) = &waiting
            && self.sticker_sendable(channel, listed).is_err()
        {
            self.set_waiting_sticker(channel, None);
            return self.show_composer_error("That sticker can't be sent here.");
        }
        // `:name:`s go out as emoji. The length is checked on what goes out,
        // which is longer than what was typed.
        let library = self.emoji_library(channel);
        let translated = library.translate(&typed);
        let (text, used) = (translated.text, translated.used);
        // The client checks length before slowmode. The text stays in the box to be shortened.
        let too_long = self.account.state.borrow().limits().check_length(&text);
        if let Err(too_long) = too_long {
            return self.show_too_long(too_long);
        }
        if !files.is_empty() {
            if !self.account.state.borrow().can_attach(channel) {
                return self.update_notice_bar();
            }
            let allowed = {
                let state = self.account.state.borrow();
                state.limits().uploads(state.find_channel(channel).and_then(|(guild, _)| guild))
            };
            if let Err(refusal) = discord::uploads::check_send(&allowed, &Self::sizes(&files)) {
                return self.show_refusal(&refusal);
            }
        }
        // Slowmode would refuse it; the notice bar says how long is left.
        if self.slowmode_left(channel) > 0 {
            return self.update_notice_bar();
        }
        // The time may have come too close since it was chosen, which Discord would refuse.
        let scheduled = self.scheduled_time(channel);
        if let Some(Err(refusal)) = scheduled.map(|at| scheduled::check(at, now_unix(), None)) {
            return self.show_composer_error(refusal.text());
        }
        // `+:name:` reacts to the newest message in place of sending, unless
        // files go with it, which send as they are.
        if files.is_empty() && self.react_by_shortcut(channel, &typed, &library) {
            self.typing_message_sent(channel);
            self.set_composer_text("");
            self.mention_popover.popdown();
            return;
        }
        let can_reply = self.account.state.borrow().can_reply(channel);
        let replying = self.reply.borrow().clone().filter(|r| r.channel == channel && can_reply);
        let reply = replying.as_ref().map(|r| Reply {
            to: r.to,
            guild: r.guild,
            ping: r.fixed_ping.unwrap_or(self.reply_ping.is_active()),
        });
        let ping = self.reply_ping.is_active();
        // The client shows a message as sent before the request, ending what
        // we were typing, unless files go with it: those it shows on success.
        // A scheduled one isn't shown, nor does it start a cooldown.
        if scheduled.is_none() {
            self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
            if files.is_empty() {
                self.typing_message_sent(channel);
            }
        }
        self.set_composer_text("");
        self.set_reply(None);
        self.set_waiting_sticker(channel, None);
        self.mention_popover.popdown();
        // The client sends the message all the same, and says why an emoji won't show.
        if !translated.locked.is_empty() {
            self.show_composer_error(locked_notice(&translated.locked, library.place.external));
        }
        let this = self.clone();
        let at = scheduled.map(scheduled::iso);
        glib::spawn_future_local(async move {
            // The gateway echoes our own message back, which is what adds it to the list.
            let sticker = waiting.as_ref().map(|w| w.sticker.id);
            let message = Outgoing { text: &text, reply, sticker, at: at.as_deref() };
            match this.send_with_uploads(&rest, channel, &message, files).await {
                Ok(()) => match scheduled {
                    Some(at) => this.message_scheduled(channel, at),
                    None => {
                        // Once Discord took it; a scheduled message is not counted.
                        this.track_message_use(&used, sticker);
                        this.message_sent(channel)
                    }
                },
                Err(stopped) => {
                    match scheduled {
                        Some(_) => this.scheduling_failed(&stopped),
                        None => this.message_stopped(channel, &stopped),
                    }
                    let buffer = this.composer.buffer();
                    if buffer.char_count() == 0 && this.reply.borrow().is_none() {
                        if this.waiting.of(channel).is_none() {
                            this.set_waiting_sticker(channel, waiting);
                        }
                        this.set_composer_text(&typed);
                        if this.account.state.borrow().channel == Some(channel) {
                            this.set_reply(replying);
                            this.reply_ping.set_active(ping);
                        }
                    }
                }
            }
        });
    }
}

impl App {
    /// What follows a message that went out.
    pub(super) fn message_sent(self: &Rc<Self>, channel: Snowflake) {
        self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
        // The client shows every message again on success, which ends what we typed meanwhile.
        self.typing_message_sent(channel);
        // Sending from an old stretch of history returns to the present.
        if self.account.timelines.borrow().get(channel).is_some_and(|t| t.detached) {
            self.load_newest(channel);
        }
    }

    /// What follows a message that didn't: slowmode as Discord says it is,
    /// and the reason.
    pub(super) fn message_stopped(self: &Rc<Self>, channel: Snowflake, stopped: &Stopped) {
        if let Some(why) = self.record_stopped(channel, stopped) {
            self.show_composer_error(&why);
        }
    }

    /// Slowmode as a message that didn't go out leaves it, and what to tell
    /// of it, unless the user took it back.
    pub(super) fn record_stopped(self: &Rc<Self>, channel: Snowflake, stopped: &Stopped) -> Option<String> {
        match stopped {
            Stopped::Send(e) if e.slowmode_wait().is_some() => {
                let wait = e.slowmode_wait().unwrap_or_default();
                self.record_slowmode(channel, |slowmode, c, _| slowmode.refused(c, wait, now_ms()));
                Some("Not sent: slowmode is enabled in this channel.".to_owned())
            }
            Stopped::Send(e) => {
                self.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                Some(format!("Not sent: {e}"))
            }
            Stopped::Upload(why) => {
                self.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                Some(format!("Not sent: {why}"))
            }
            Stopped::Canceled => {
                self.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                None
            }
        }
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

/// If the cursor sits just after a `:word` of at least two characters, the
/// `:`'s char offset and the word without it. The word runs back to the
/// last whitespace, as the client takes it, and one that has closed its
/// colons already is no query.
pub(super) fn emoji_token(buffer: &gtk::TextBuffer) -> Option<(i32, String)> {
    let cursor = buffer.property::<i32>("cursor-position");
    let text: String = buffer.text(&buffer.start_iter(), &buffer.iter_at_offset(cursor), false).into();
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().rposition(|c| c.is_whitespace()).map_or(0, |at| at + 1);
    let query = &chars[start..];
    let (colon, query) = query.split_first()?;
    (*colon == ':' && query.len() >= 2 && !query.contains(&':')).then(|| (start as i32, query.iter().collect()))
}
