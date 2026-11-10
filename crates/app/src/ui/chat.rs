//! The message list: opening a channel, scrollback, live updates, and
//! building each message row.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use discord::held::Opening;
use discord::history::{AROUND_PAGE, Around, KINETIC_INPUT, Load, Loadable, Position, SCROLL_INPUT, Viewport};
use discord::notifications;
use discord::read_state::starts_unread;
use discord::rest::Page;
use discord::system::{self, Kind};
use discord::{ChannelKind, Message, MessageEdit, Referenced, Snowflake, User};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use super::App;
use super::embeds::{behind_spoiler, embed_widget};
use super::menus::{self, At, Entry, Item};
use super::message_row::{MessageRow, Parts};
use super::state::{Unread, now_ms, now_unix};
use super::widgets::avatar_widget;
use super::{attachments, components, polls};
use crate::render::{Context, Names, Spoilers};
use crate::rich_text::RichText;
use crate::{render, timelines};

/// How long a message we jumped to stays highlighted.
const FLASH: std::time::Duration = std::time::Duration::from_secs(2);
/// Space between the last message and the composer, as around the composer.
const COMPOSER_GAP: i32 = 12;
/// Consecutive messages by one author within this window share a header.
const GROUP_WINDOW_SECS: i64 = 7 * 60;

/// What a message's menu acts on.
pub(super) struct MessageTarget {
    /// The row's message.
    pub(super) message: Message,
    /// The text selected in it, as Ctrl+C would copy it.
    pub(super) selection: Option<String>,
    /// Where the link the menu was opened on goes.
    pub(super) link: Option<String>,
}

impl MessageTarget {
    /// The message the row shows: for a thread's first message, the one
    /// the thread was started from.
    fn shown(&self) -> &Message {
        self.message.thread_starter().unwrap_or(&self.message)
    }
}

/// The selection, as on the desktop official client (373963).
pub(super) const COPY_SELECTION: Entry<MessageTarget> = Entry::Item(Item {
    name: "copy",
    label: "_Copy",
    shows: |_, target| target.selection.is_some(),
    kind: menus::Kind::Action(|app, target| app.copy(target.selection.as_deref().unwrap_or_default())),
});

pub(super) const COPY_TEXT: Entry<MessageTarget> = Entry::Item(Item {
    name: "copy-text",
    label: "Copy _Text",
    shows: |_, target| target.shown().copy_text().is_some(),
    kind: menus::Kind::Action(|app, target| app.copy(&target.shown().copy_text().unwrap_or_default())),
});

pub(super) const COPY_MESSAGE_LINK: Entry<MessageTarget> = Entry::Item(Item {
    name: "copy-message-link",
    label: "Copy Message _Link",
    shows: |_, _| true,
    kind: menus::Kind::Action(|app, target| {
        let message = target.shown();
        let guild =
            app.state.borrow().find_channel(message.channel_id).map_or(message.guild_id, |(g, _)| g.map(|g| g.id));
        app.copy(&message.link(guild));
    }),
});

/// The link the menu was opened on (777933).
pub(super) const COPY_LINK: Entry<MessageTarget> = Entry::Item(Item {
    name: "copy-link",
    label: "Copy Lin_k",
    shows: |_, target| target.link.is_some(),
    kind: menus::Kind::Action(|app, target| app.copy(target.link.as_deref().unwrap_or_default())),
});

pub(super) const OPEN_LINK: Entry<MessageTarget> = Entry::Item(Item {
    name: "open-link",
    label: "_Open Link",
    shows: |_, target| target.link.is_some(),
    kind: menus::Kind::Action(|app, target| {
        if let Some(link) = &target.link {
            gtk::UriLauncher::new(link).launch(Some(&app.window), gio::Cancellable::NONE, |_| {});
        }
    }),
});

/// Only in Developer Mode (50268). Shift adds the channel's id in front.
pub(super) const COPY_MESSAGE_ID: Entry<MessageTarget> = Entry::Item(Item {
    name: "copy-message-id",
    label: "Copy Message _ID",
    shows: |app, _| app.state.borrow().developer_mode,
    kind: menus::Kind::Action(|app, target| {
        let message = target.shown();
        app.copy(&match app.shift_held() {
            true => format!("{}-{}", message.channel_id, message.id),
            false => message.id.to_string(),
        });
    }),
});

impl App {
    /// The chat column: the message list (with scrollback) from edge to edge,
    /// and the header bar and composer floating over it.
    pub(super) fn build_chat(self: &Rc<Self>) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let this = Rc::downgrade(self);
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                item.set_activatable(false);
                let cell = MessageRow::new(item);
                item.set_child(Some(&cell));
                attach_message_menu(&this, item, &cell);
                attach_message_actions(&this, &cell);
                if let Some(this) = this.upgrade() {
                    let mut rows = this.rows.borrow_mut();
                    rows.retain(|row| row.upgrade().is_some());
                    rows.push(cell.downgrade());
                }
            }
        });
        factory.connect_bind({
            let this = Rc::downgrade(self);
            move |_, item| {
                let Some(this) = this.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                this.bind_row(item);
            }
        });
        self.list.set_factory(Some(&factory));
        self.list.add_css_class("messages");
        // The keys for a menu open the focused message's, wherever in it the
        // focus is: ahead of a label's own menu.
        menus::on_keys(&self.list, |list| {
            // The focused list item holds the cell the menu's action is on.
            let cell = list.focus_child().and_then(|item| item.first_child());
            cell.is_some_and(|cell| cell.activate_action("message.menu", None).is_ok())
        });
        self.scroller.set_child(Some(&self.list));
        self.scroller.add_css_class("messages");

        // Near an end, fetch more, but only when the user took us there: the
        // list also moves by itself, and to its very top when its model is
        // swapped or a page replaces what it held.
        self.scroller.vadjustment().connect_value_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    let position = this.scroll_position();
                    if this.scroll_loader.borrow_mut().moved(position, Instant::now()) {
                        this.maybe_load(true);
                    }
                    this.check_unread_seen();
                }
            }
        });
        self.watch_scroll_input();

        // After a jump: say so, with the way back.
        self.present_bar.connect_button_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if let Some(channel) = this.state.borrow().channel {
                    this.load_newest(channel);
                }
            }
        });

        let composer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["composer-area", "floating-card", "floating"])
            .build();
        self.build_composer(&composer);
        let page = adw::ToolbarView::builder()
            .content(&self.scroller)
            .extend_content_to_top_edge(true)
            .extend_content_to_bottom_edge(true)
            .top_bar_style(adw::ToolbarStyle::Flat)
            .bottom_bar_style(adw::ToolbarStyle::Flat)
            .css_classes(["floating-bars"])
            .build();
        page.add_controller(self.drop_target());
        let header = self.header.header_bar(&[self.sidebar_toggle().upcast_ref()]);
        page.add_top_bar(&header);
        page.add_top_bar(&self.present_bar);
        page.add_top_bar(&self.unread_bar);
        page.add_top_bar(&self.paused_bar);
        self.unread_bar.connect_button_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.jump_to_unread();
                }
            }
        });
        page.add_bottom_bar(&composer);

        // The list scrolls under the bars, and pads its ends to their height
        // so its first and last messages can come out from under them. The
        // scrollbar keeps out from under them altogether.
        let padding = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&page),
            &padding,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let pad = move |page: &adw::ToolbarView| {
            let (top, bottom) = (page.top_bar_height(), page.bottom_bar_height());
            // The last message stops short of the composer by the composer's
            // own margin.
            let end = bottom + COMPOSER_GAP;
            padding.load_from_string(&format!(
                "listview.messages {{ padding-top: {top}px; padding-bottom: {end}px; }}\n\
                 scrolledwindow.messages > scrollbar.vertical {{ margin-top: {top}px; margin-bottom: {bottom}px; }}"
            ));
        };
        page.connect_top_bar_height_notify(pad.clone());
        page.connect_bottom_bar_height_notify(pad);
        page.upcast()
    }

    pub(super) fn on_message(self: &Rc<Self>, message: Message) {
        let original = match &message.referenced_message {
            Referenced::Message(original) => Some(&original.author),
            _ => None,
        };
        self.learn_users(std::iter::once(&message.author).chain(&message.mentions).chain(original));
        if let (Some(guild), Some(member)) = (message.guild_id, &message.member) {
            self.state.borrow_mut().members.add(guild, vec![(message.author.id, member.roles.clone())], &[]);
        }
        let channel = message.channel_id;
        self.state.borrow_mut().typing.message(channel, message.author.id, false);
        self.refresh_typing();
        let viewing = self.state.borrow().channel == Some(channel) && self.chat_visible();
        let notify = {
            let mut state = self.state.borrow_mut();
            state.bump_last_message(channel, message.id);
            let own = state.me() == Some(message.author.id);
            // No notifications for message requests, or for what `is_dropped` drops (as in Discord).
            let group_dm = state.find_channel(channel).is_some_and(|(_, c)| c.kind == ChannelKind::GroupDm);
            let blocked = |id| state.relationships.is_blocked(id);
            let quiet = state.find_channel(channel).is_some_and(|(_, c)| c.is_request())
                || notifications::is_dropped(&message, group_dm, blocked);
            if own || viewing {
                // Discord marks what we send as read; what we watch arrive, we ack.
                state.read.ack(channel, message.id);
                None
            } else {
                let place = state.place(channel);
                let roles = message
                    .guild_id
                    .and_then(|g| state.guilds.iter().find(|guild| guild.id == g))
                    .map(|g| g.my_roles.clone())
                    .unwrap_or_default();
                let pinged = state.me().is_some_and(|me| state.notifications.mentions_me(&message, me, &roles));
                let notify = !quiet
                    && match state.find_channel(channel) {
                        // A thread goes by our membership, and its parent.
                        Some((Some(_), thread)) if thread.kind.is_thread() => {
                            let parent = thread.parent_id.and_then(|id| state.place(id));
                            parent.zip(state.me()).is_some_and(|(parent, me)| {
                                let n = &state.notifications;
                                n.should_notify_in_thread(thread, &parent, &message, me, &roles, now_unix())
                            })
                        }
                        _ => place.is_some_and(|p| state.notifications.should_notify(&p, pinged, now_unix())),
                    };
                let counted =
                    state.notifications.counts_mention(place.as_ref(), group_dm, &message, pinged, blocked, now_unix());
                state.read.on_message(channel, counted);
                place.filter(|_| notify)
            }
        };
        if viewing && self.state.borrow().me() != Some(message.author.id) {
            self.queue_ack(channel, message.id);
        }
        if let Some(place) = notify {
            self.send_notification(&message, &place);
        }
        self.refresh_badges();

        // Keep every cached timeline current, not only the open one. A
        // detached one would get a gap; scrolling down fetches in order.
        let Some(store) = self.timelines.borrow().get(channel).filter(|t| !t.detached).map(|t| t.store.clone()) else {
            return;
        };
        // A history fetch may have raced this event and already added it.
        if timelines::find(&store, message.id).is_some() {
            return;
        }
        let open = self.state.borrow().channel == Some(channel);
        if !self.timelines.borrow().held.live_messages(channel, open) {
            return;
        }
        let at_bottom = open && self.at_bottom();
        if open {
            self.resolve_mentions(&message.content);
        }
        let blocked = self.state.borrow().relationships.is_blocked(message.author.id);
        store.append(&timelines::boxed(message));
        if blocked {
            self.rebind_blocked_run(&store, store.n_items() - 1);
        }
        if at_bottom {
            self.scroll_to_end();
        }
        // The client cuts a channel as messages arrive too: one we aren't
        // looking at doesn't grow without end.
        if !open && let Some(timeline) = self.timelines.borrow_mut().get_mut(channel) {
            timelines::keep_newest(timeline);
        }
    }

    pub(super) fn at_bottom(&self) -> bool {
        let adjustment = self.scroller.vadjustment();
        adjustment.value() + adjustment.page_size() >= adjustment.upper() - 80.0
    }

    pub(super) fn on_message_update(self: &Rc<Self>, edit: MessageEdit) {
        let Some(store) = self.timelines.borrow().get(edit.channel_id).map(|t| t.store.clone()) else { return };
        let (id, content) = (edit.id, edit.content.clone());
        // In place: a new object would get a new row, rather than this one
        // changed.
        if let Some(position) = timelines::edit(&store, id, |message| edit.apply(message)) {
            self.rebind(&store, position, 1);
        }
        // Replies show the start of what they answer; keep it current.
        if let Some(content) = content {
            let replies = timelines::update_replies(&store, id, |original| {
                if let Referenced::Message(original) = original {
                    original.content = content.clone();
                }
            });
            for position in replies {
                self.rebind(&store, position, 1);
            }
        }
    }

    pub(super) fn on_message_delete(self: &Rc<Self>, id: Snowflake, channel_id: Snowflake) {
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, _)) = timelines::find(&store, id) {
            self.hand_off_focus(&store, position);
            store.remove(position);
            // The message after it may now group with the one before.
            self.rebind(&store, position, 1);
            // A collapsed run it belonged to now counts one less.
            if let Some(before) = position.checked_sub(1) {
                self.rebind_blocked_run(&store, before);
            }
        }
        for position in timelines::update_replies(&store, id, |original| *original = Referenced::Deleted) {
            self.rebind(&store, position, 1);
        }
        // Nothing left to reply to, or edit, as in the official client.
        if self.reply.borrow().as_ref().is_some_and(|r| r.to == id) {
            self.set_reply(None);
        }
        if self.editing() == Some(id) {
            self.end_edit();
        }
    }

    /// A purge: every message it names goes, as one by one.
    pub(super) fn on_message_delete_bulk(self: &Rc<Self>, ids: Vec<Snowflake>, channel_id: Snowflake) {
        for id in ids {
            self.on_message_delete(id, channel_id);
        }
    }

    /// Votes arrived for a poll, or were taken back. Held channels count too:
    /// the client keeps their polls current, and so must we.
    pub(super) fn on_poll_vote(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        votes: Vec<(u32, Snowflake)>,
        added: bool,
        many: bool,
    ) {
        let Some(store) = self.timelines.borrow().get(channel).map(|t| t.store.clone()) else { return };
        let (me, dm) = {
            let state = self.state.borrow();
            (state.me(), state.find_channel(channel).is_some_and(|(_, c)| c.kind == ChannelKind::Dm))
        };
        let mut changed = None;
        for (answer, user) in votes {
            let mine = me == Some(user);
            let at = timelines::edit(&store, message, |m| {
                if let Some(poll) = &mut m.poll {
                    if added { poll.vote(answer, mine, dm && !many) } else { poll.unvote(answer, mine) }
                }
            });
            changed = at.or(changed);
        }
        if let Some(position) = changed {
            self.rebind(&store, position, 1);
        }
    }

    /// Move the focus to a neighbour of the row at `position` in `store`, if
    /// that row has it and is about to go. The list scrolls to follow its
    /// focus, and GTK would move it to the first row, taking the view to
    /// the top of the channel.
    fn hand_off_focus(self: &Rc<Self>, store: &gio::ListStore, position: u32) {
        if *store != self.messages() {
            return;
        }
        let focused = self.rows.borrow().iter().filter_map(|row| row.upgrade()).any(|row| {
            let item = row.parts().item.upgrade();
            item.is_some_and(|item| item.item().is_some() && item.position() == position)
                && row.parent().is_some_and(|widget| widget.has_focus() || widget.focus_child().is_some())
        });
        if !focused {
            return;
        }
        let neighbour = (position + 1 < store.n_items()).then_some(position + 1).or(position.checked_sub(1));
        match neighbour {
            Some(neighbour) => self.list.scroll_to(neighbour, gtk::ListScrollFlags::FOCUS, None),
            None => _ = self.composer.grab_focus(),
        }
    }

    /// Fill `item`'s row in for the message it holds now.
    fn bind_row(self: &Rc<Self>, item: &gtk::ListItem) {
        let cell = item.child().and_downcast::<MessageRow>().expect("message row");
        let parts = cell.parts();
        let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
        let message = message.borrow::<Message>().clone();
        let messages = self.messages();
        let position = item.position();
        let previous = position.checked_sub(1).and_then(|p| timelines::message_at(&messages, p));
        let divided = self.fill_divider(parts, &message, previous.as_ref());
        let (run, revealed) = {
            let state = self.state.borrow();
            let run = timelines::run_at(&messages, position, |m| state.relationships.is_blocked(m.author.id));
            (run, run.is_some_and(|r| state.revealed_blocked.contains(&r.first)))
        };
        match run {
            Some(run) if run.start == position => self.fill_blocked_bar(&cell, run, revealed),
            _ => parts.bar.set_visible(false),
        }
        parts.row.set_visible(run.is_none() || revealed);
        if !parts.row.is_visible() {
            return;
        }
        self.fill_message_row(parts, &message, previous.as_ref().filter(|_| !divided), &messages);
    }

    /// Fill in again the rows showing `len` messages of `store` from `start`,
    /// if it is the open channel's, from the messages and state as they are
    /// now. Splicing in a fresh object would do it too, but the list then
    /// makes a new row for it rather than reuse the one it had.
    pub(super) fn rebind(self: &Rc<Self>, store: &gio::ListStore, start: u32, len: u32) {
        if *store != self.messages() {
            return;
        }
        let rows: Vec<MessageRow> = self.rows.borrow().iter().filter_map(|row| row.upgrade()).collect();
        for row in rows {
            let Some(item) = row.parts().item.upgrade() else { continue };
            if item.item().is_some() && (start..start.saturating_add(len)).contains(&item.position()) {
                self.bind_row(&item);
            }
        }
    }

    /// Above a message: the date where a new day starts, and "New" above the
    /// first unread message. Returns whether there is a divider, which
    /// starts a new group of messages.
    fn fill_divider(&self, parts: &Parts, message: &Message, previous: Option<&Message>) -> bool {
        let divider = &parts.divider;
        let time = parse_time(&message.timestamp);
        let day = |m: &Message| parse_time(&m.timestamp).map(|t| t.ymd());
        let new_day = previous.is_none_or(|p| day(p) != time.as_ref().map(|t| t.ymd()));
        let unread = self.open_unread().is_some_and(|u| {
            u.channel == message.channel_id && starts_unread(previous.map(|p| p.id), message.id, u.acked)
        });
        // The list keeps many more rows than it shows, so where the first
        // unread one is decides whether it has been seen, not its binding.
        if unread {
            self.unread_divider.set(Some(divider));
        } else if self.unread_divider.upgrade().as_ref() == Some(divider) {
            self.unread_divider.set(None);
        }
        // A new day whose time doesn't parse gets the line without a date.
        let date =
            time.filter(|_| new_day).map(|t| t.format("%e %B %Y").map(|d| d.trim().to_owned()).unwrap_or_default());
        parts.set_divider(new_day, date.as_deref(), unread);
        new_day || unread
    }

    /// Once the first unread message has come into view, stop offering to
    /// jump to it.
    fn check_unread_seen(&self) {
        let Some(divider) = self.unread_divider.upgrade().filter(|d| d.is_mapped()) else { return };
        let Some(bounds) = divider.compute_bounds(&self.scroller) else { return };
        let height = self.scroller.height() as f32;
        if bounds.y() + bounds.height() > 0.0 && bounds.y() < height {
            self.unread_seen();
        }
    }

    fn unread_seen(&self) {
        if let Some(unread) = self.state.borrow_mut().unread.as_mut() {
            unread.seen = true;
        }
        self.unread_bar.set_revealed(false);
    }

    /// What was unread in the open channel when it opened.
    fn open_unread(&self) -> Option<Unread> {
        let state = self.state.borrow();
        state.unread.filter(|u| Some(u.channel) == state.channel)
    }

    /// Offer to jump to the first unread message, until it has been in view:
    /// how many there are, and since when.
    pub(super) fn update_unread_bar(&self) {
        let Some(unread) = self.open_unread().filter(|u| !u.seen) else {
            return self.unread_bar.set_revealed(false);
        };
        let store = self.messages();
        let (count, mut more) = discord::read_state::unread_count(&timelines::ids(&store), unread.acked);
        more &= !self.timelines.borrow().get(unread.channel).is_some_and(|t| t.complete);
        if count == 0 {
            return self.unread_bar.set_revealed(false);
        }
        // Since the first unread message, or if that isn't loaded, about
        // when the last read one was sent.
        let first = timelines::ids(&store).iter().position(|&id| id > unread.acked);
        let since = match first.filter(|_| !more).and_then(|p| timelines::message_at(&store, p as u32)) {
            Some(first) => parse_time(&first.timestamp),
            None => glib::DateTime::from_unix_local(unread.acked.unix_ms() / 1000).ok(),
        };
        let today = glib::DateTime::now_local().ok().map(|now| now.ymd());
        let pattern = if since.as_ref().map(|t| t.ymd()) == today { "%H:%M" } else { "%H:%M on %e %B %Y" };
        let since = since.and_then(|t| t.format(pattern).ok()).unwrap_or_default();
        let plural = if count == 1 && !more { "" } else { "s" };
        let plus = if more { "+" } else { "" };
        self.unread_bar.set_title(&format!("{count}{plus} new message{plural} since {since}"));
        self.unread_bar.set_revealed(true);
    }

    /// Show the "viewing older messages" bar while the open channel is detached.
    /// Requests are held back for `wait` ([`discord::Error::Paused`]): say
    /// so until it is over, rather than leave the window to stall silently.
    pub(super) fn show_paused(self: &Rc<Self>, wait: Duration) {
        if let Some(timer) = self.paused_end.take() {
            timer.remove();
        }
        let minutes = wait.as_secs().div_ceil(60).max(1);
        let plural = if minutes == 1 { "" } else { "s" };
        self.paused_bar
            .set_title(&format!("Discord refused too many requests, so none are sent for {minutes} minute{plural}"));
        self.paused_bar.set_revealed(true);
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(wait, move || {
            if let Some(this) = this.upgrade() {
                // The source is spent; there is nothing left to remove.
                this.paused_end.take();
                this.paused_bar.set_revealed(false);
            }
        });
        self.paused_end.replace(Some(timer));
    }

    pub(super) fn update_present_bar(&self) {
        let channel = self.state.borrow().channel;
        let detached = channel.and_then(|c| self.timelines.borrow().get(c).map(|t| t.detached)).unwrap_or(false);
        self.present_bar.set_revealed(detached);
    }

    /// The bar that stands in for a run of blocked users' messages: how many
    /// there are, and a button to show or hide them.
    fn fill_blocked_bar(self: &Rc<Self>, cell: &MessageRow, run: timelines::Run, revealed: bool) {
        let parts = cell.parts();
        let (label, button) = parts.bar_parts.get_or_init(|| {
            let label = gtk::Label::builder().xalign(0.0).build();
            let button = gtk::Button::builder().css_classes(["link"]).build();
            let (this, cell) = (Rc::downgrade(self), cell.downgrade());
            button.connect_clicked(move |_| {
                let (Some(this), Some(cell)) = (this.upgrade(), cell.upgrade()) else { return };
                let Some(run) = cell.parts().run.get() else { return };
                {
                    let mut state = this.state.borrow_mut();
                    if !state.revealed_blocked.remove(&run.first) {
                        state.revealed_blocked.insert(run.first);
                    }
                }
                this.rebind(&this.messages(), run.start, run.len);
            });
            parts.bar.append(&label);
            parts.bar.append(&button);
            (label, button)
        });
        parts.run.set(Some(run));
        label.set_label(&match run.len {
            1 => "1 blocked message".to_owned(),
            n => format!("{n} blocked messages"),
        });
        button.set_label(if revealed { "Hide" } else { "Show" });
        parts.bar.set_visible(true);
    }

    /// Re-bind the first row of the blocked run at `position`, if any, so its
    /// bar shows the current count.
    fn rebind_blocked_run(self: &Rc<Self>, store: &gio::ListStore, position: u32) {
        let run = {
            let state = self.state.borrow();
            timelines::run_at(store, position, |m| state.relationships.is_blocked(m.author.id))
        };
        if let Some(run) = run {
            self.rebind(store, run.start, 1);
        }
    }

    /// The open channel's store (or the blank one).
    pub(super) fn messages(&self) -> gio::ListStore {
        self.selection.model().and_downcast().unwrap_or_else(|| self.blank.clone())
    }

    pub(super) fn show_blank(&self) {
        self.selection.set_model(Some(&self.blank));
    }

    pub(super) fn select_channel(self: &Rc<Self>, id: Snowflake) {
        let (title, forum, rest, left) = {
            let mut state = self.state.borrow_mut();
            let left = state.channel.filter(|&c| c != id);
            state.channel = Some(id);
            state.page = None;
            let found = state.find_channel(id).map(|(_, c)| c);
            let name = found.and_then(|c| c.name.clone()).unwrap_or_default();
            let title = match found {
                Some(dm) if dm.kind == discord::ChannelKind::Dm => format!("@ {}", dm.title()),
                Some(group) if group.kind == discord::ChannelKind::GroupDm => group.title(),
                Some(thread) if thread.kind.is_thread() => name,
                _ => format!("# {name}"),
            };
            let forum = found.is_some_and(|c| c.kind.is_forum_like());
            (title, forum, state.rest.clone(), left)
        };
        let subtitle = self.channel_subtitle(id);
        self.refresh_typing();
        // The channel we left keeps only its newest messages, which live
        // events keep current. The client keeps the rest to page back in from
        // memory; we ask for it.
        if let Some(left) = left
            && let Some(timeline) = self.timelines.borrow_mut().get_mut(left)
        {
            timelines::keep_newest(timeline);
        }
        if self.reply.borrow().as_ref().is_some_and(|r| r.channel != id) {
            self.set_reply(None);
        }
        if self.editing.borrow().as_ref().is_some_and(|e| e.message.channel_id != id) {
            self.end_edit();
        }
        // A forum or media channel is made of posts, and never takes a
        // message: nothing to load, and no message box.
        if forum {
            self.state.borrow_mut().unread = None;
            self.content.set_visible_child_name("forum");
            self.forum_title.set(&title, subtitle.as_deref());
            self.set_content_title(&title);
            self.rebuild_sidebar();
            self.update_notice_bar();
            return;
        }
        // What is unread now, before opening the channel reads it.
        let unread = {
            let state = self.state.borrow();
            let acked = state.find_channel(id).filter(|(_, c)| state.is_unread(c)).and_then(|_| state.read.acked(id));
            acked.map(|acked| Unread { channel: id, acked, seen: false })
        };
        self.state.borrow_mut().unread = unread;
        self.content.set_visible_child_name("chat");
        self.rebuild_sidebar();
        self.header.set(&title, subtitle.as_deref());
        self.set_content_title(&title);
        self.composer_error.set_visible(false);
        self.update_notice_bar();

        // Show what we have at once (instant for a recently viewed channel),
        // and ask for the newest page, to merge in, unless it's a DM we hold,
        // which the gateway keeps current as the client trusts it to.
        let (system_dm_unread, private) = {
            let state = self.state.borrow();
            let found = state.find_channel(id);
            (
                found.is_some_and(|(_, c)| c.is_system_dm() && state.is_unread(c)),
                found.is_some_and(|(g, _)| g.is_none()),
            )
        };
        let (store, loading, detached, fetch) = {
            let mut timelines = self.timelines.borrow_mut();
            let timeline = timelines.reopen(id, u64::try_from(now_ms()).unwrap_or_default());
            let (store, loading, detached) = (timeline.store.clone(), timeline.loading, timeline.detached);
            let fetch = timelines.held.fetch_on_open(id, Opening { loading, system_dm_unread, private });
            (store, loading, detached, fetch)
        };
        self.scroll_loader.borrow_mut().reset();
        self.selection.set_model(Some(&store));
        self.update_present_bar();
        self.update_unread_bar();
        self.settle_at_end();

        if rest.is_some() && fetch {
            self.load_newest(id);
        } else if !loading && !detached && self.chat_visible() {
            // What we hold is shown as it is, and reading it is all a page
            // arriving would have done.
            self.mark_read(id);
        }
    }

    /// Fetch the newest page and merge it into the channel's store. This is
    /// also how a jump returns to the present.
    pub(super) fn load_newest(self: &Rc<Self>, id: Snowflake) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let (store, was_detached) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(id) else { return };
            timeline.loading = true;
            (timeline.store.clone(), timeline.detached)
        };
        let limit = self.viewport().first_page();
        let started = self.timelines.borrow().held.started();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(id, limit, Page::Newest).await;
            if let Some(timeline) = this.timelines.borrow_mut().get_mut(id) {
                timeline.loading = false;
            }
            match result {
                Ok(page) => {
                    let stale = this.timelines.borrow_mut().landed(id, started);
                    this.learn_messages(&page);
                    let full = page.len() == usize::from(limit);
                    let open = this.state.borrow().channel == Some(id);
                    let at_bottom = !open || was_detached || this.at_bottom();
                    let replaced = timelines::merge_newest(&store, page);
                    if let Some(timeline) = this.timelines.borrow_mut().get_mut(id) {
                        timeline.detached = false;
                        // A short page is the whole channel; a replaced store
                        // lost its scrollback and can page again.
                        if !full {
                            timeline.complete = true;
                        } else if replaced {
                            timeline.complete = false;
                        }
                    }
                    this.update_present_bar();
                    this.update_unread_bar();
                    this.reached_present(id, &store);
                    // The user may have switched channels while this was loading.
                    if open && this.state.borrow().channel == Some(id) {
                        if at_bottom {
                            this.settle_at_end();
                        }
                        if this.chat_visible() {
                            this.mark_read(id);
                        }
                    }
                    this.refetch_if_stale(id, stale);
                }
                Err(e) => {
                    if this.state.borrow().channel == Some(id) {
                        this.show_composer_error(&format!("Could not load messages: {e}"));
                    }
                }
            }
        });
    }

    /// A page just arrived for `channel`. If the connection changed while it
    /// was on its way, and the channel is open, the client asks for it again
    /// at once (`$` in 123363).
    fn refetch_if_stale(self: &Rc<Self>, channel: Snowflake, stale: bool) {
        let open = self.state.borrow().channel == Some(channel);
        if self.timelines.borrow_mut().held.refetch(channel, stale, open, Instant::now()) {
            self.load_newest(channel);
        }
    }

    /// Learn names from a batch of messages (and the ones they reply to) and
    /// look up unknown mentions.
    pub(super) fn learn_messages(self: &Rc<Self>, messages: &[Message]) {
        let originals = messages.iter().filter_map(|m| match &m.referenced_message {
            Referenced::Message(original) => Some(original.as_ref()),
            _ => None,
        });
        let all: Vec<&Message> = messages.iter().chain(originals).collect();
        self.learn_users(all.iter().flat_map(|m| std::iter::once(&m.author).chain(&m.mentions)));
        for message in all {
            self.resolve_mentions(&message.content);
        }
        // The page's authors and whom they mention, for their colours.
        let guild = {
            let state = self.state.borrow();
            messages.first().and_then(|m| state.find_channel(m.channel_id)).and_then(|(g, _)| g.map(|g| g.id))
        };
        if let Some(guild) = guild {
            self.state.borrow_mut().members.page_loaded(guild, messages);
            self.send_member_requests();
        }
    }

    /// The store holds the channel's newest messages again.
    fn reached_present(&self, channel: Snowflake, store: &gio::ListStore) {
        if let Some(newest) = store.n_items().checked_sub(1).and_then(|p| timelines::message_at(store, p)) {
            // READY's newest-message ids can be stale; history is not.
            self.state.borrow_mut().bump_last_message(channel, newest.id);
        }
    }

    /// What page sizes go by.
    fn viewport(&self) -> Viewport {
        // In 1024ths of a dot per inch, -1 for the default; 96dpi is scale 1.
        let dpi = gtk::Settings::default().map_or(-1, |settings| settings.gtk_xft_dpi());
        Viewport {
            height: self.window.height(),
            text_scale: if dpi > 0 { f64::from(dpi) / (96.0 * 1024.0) } else { 1.0 },
            compact: self.state.borrow().message_display_compact,
        }
    }

    /// How tall the messages are. The adjustment's `upper` is no good: it
    /// stops at the view's height, so a list that doesn't fill the view yet
    /// would seem not to grow, and pause.
    fn content_height(&self) -> f64 {
        let (_, natural, _, _) = self.list.measure(gtk::Orientation::Vertical, self.list.width());
        f64::from(natural)
    }

    fn scroll_position(&self) -> Position {
        let adjustment = self.scroller.vadjustment();
        Position { above: adjustment.value(), below: adjustment.upper() - adjustment.value() - adjustment.page_size() }
    }

    /// Listen for what the user does to the list: only that loads history.
    /// Each of these also lifts a pause, as in the client (`onWheel`,
    /// `onTouchMove`, `onKeyDown`, `onMouseDown` and `onMouseUp` in 567008).
    fn watch_scroll_input(self: &Rc<Self>) {
        let input = gtk::EventControllerLegacy::new();
        input.set_propagation_phase(gtk::PropagationPhase::Capture);
        input.connect_event({
            let this = Rc::downgrade(self);
            move |_, event| {
                if let (Some(this), Some(lasts)) = (this.upgrade(), scroll_input(event)) {
                    this.user_scrolled(lasts);
                }
                glib::Propagation::Proceed
            }
        });
        self.scroller.add_controller(input);

        let flings = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::KINETIC,
        );
        flings.set_propagation_phase(gtk::PropagationPhase::Capture);
        flings.connect_decelerate({
            let this = Rc::downgrade(self);
            move |_, _, _| {
                if let Some(this) = this.upgrade() {
                    this.user_scrolled(KINETIC_INPUT);
                }
            }
        });
        self.scroller.add_controller(flings);

        // A drag runs from press to release whatever the pointer does, so
        // these are events rather than gestures, which others could claim.
        let drag = gtk::EventControllerLegacy::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        drag.connect_event({
            let this = Rc::downgrade(self);
            move |_, event| {
                let Some(this) = this.upgrade() else { return glib::Propagation::Proceed };
                match event.event_type() {
                    gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => {
                        this.scroll_loader.borrow_mut().press();
                    }
                    gdk::EventType::ButtonRelease | gdk::EventType::TouchEnd => {
                        this.scroll_loader.borrow_mut().release(Instant::now());
                        this.maybe_load(true);
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        });
        self.scroller.vscrollbar().add_controller(drag);
    }

    fn user_scrolled(self: &Rc<Self>, lasts: Duration) {
        self.scroll_loader.borrow_mut().input(Instant::now(), lasts);
        self.maybe_load(true);
    }

    /// Fetch the next page if the open channel needs one. `asked` is whether
    /// the user just scrolled, rather than a page having landed.
    fn maybe_load(self: &Rc<Self>, asked: bool) {
        let Some(channel) = self.state.borrow().channel else { return };
        let unread = self.open_unread().is_some_and(|u| !u.seen);
        let Some(can) = self.timelines.borrow().get(channel).map(|timeline| Loadable {
            older: !timeline.complete,
            newer: timeline.detached,
            loading: timeline.loading,
            unread,
        }) else {
            return;
        };
        let page = self.scroll_loader.borrow().next(self.scroll_position(), can, asked);
        match page {
            Some(Load::Older) => self.load_older(channel),
            Some(Load::Newer) => self.load_newer(channel),
            None => {}
        }
    }

    /// Fetch the page before the oldest loaded message.
    fn load_older(self: &Rc<Self>, channel: Snowflake) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let (store, before) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(channel) else { return };
            // Nothing loaded yet: the newest page is still on its way.
            let Some(before) = timelines::oldest_id(&timeline.store) else { return };
            timeline.loading = true;
            (timeline.store.clone(), before)
        };
        self.scroll_loader.borrow_mut().load_started(self.content_height());
        let limit = self.viewport().scroll_page();
        let started = self.timelines.borrow().held.started();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, limit, Page::Before(before)).await;
            let stale = result.is_ok() && this.timelines.borrow_mut().landed(channel, started);
            let still_cached = this.timelines.borrow().get(channel).is_some_and(|t| t.store == store);
            if let Some(timeline) = this.timelines.borrow_mut().get_mut(channel) {
                timeline.loading = false;
            }
            if !still_cached {
                return;
            }
            let landed = result.is_ok();
            match result {
                // Only prepend if the store still starts where we asked from;
                // a merge that replaced it would make this page a gap.
                Ok(page) if timelines::oldest_id(&store) == Some(before) => {
                    if page.len() < usize::from(limit)
                        && let Some(timeline) = this.timelines.borrow_mut().get_mut(channel)
                    {
                        timeline.complete = true;
                    }
                    this.learn_messages(&page);
                    let count = page.len() as u32;
                    timelines::prepend(&store, page);
                    // What was first has one before it now, to group with.
                    this.rebind(&store, count, 1);
                    this.update_unread_bar();
                }
                Ok(_) => {}
                Err(e) => {
                    if this.state.borrow().channel == Some(channel) {
                        this.show_composer_error(&format!("Could not load older messages: {e}"));
                    }
                }
            }
            this.page_landed(channel);
            if landed {
                this.refetch_if_stale(channel, stale);
            }
        });
    }

    /// After a jump, fetch the page after the newest loaded message, until
    /// the store reaches the present again.
    fn load_newer(self: &Rc<Self>, channel: Snowflake) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let (store, after) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(channel) else { return };
            let Some(after) = timelines::newest_id(&timeline.store) else { return };
            timeline.loading = true;
            (timeline.store.clone(), after)
        };
        self.scroll_loader.borrow_mut().load_started(self.content_height());
        let limit = self.viewport().scroll_page();
        let started = self.timelines.borrow().held.started();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, limit, Page::After(after)).await;
            let stale = result.is_ok() && this.timelines.borrow_mut().landed(channel, started);
            let landed = result.is_ok();
            let still_detached = {
                let mut timelines = this.timelines.borrow_mut();
                let timeline = timelines.get_mut(channel).filter(|t| t.store == store);
                timeline.is_some_and(|t| {
                    t.loading = false;
                    t.detached
                })
            };
            match result {
                Ok(page) if still_detached && timelines::newest_id(&store) == Some(after) => {
                    let reached_newest = page.len() < usize::from(limit);
                    this.learn_messages(&page);
                    timelines::append(&store, page);
                    // A short page came back to the present by itself, with
                    // everything sent meanwhile: no need to ask for it again.
                    if reached_newest {
                        if let Some(timeline) = this.timelines.borrow_mut().get_mut(channel) {
                            timeline.detached = false;
                        }
                        this.reached_present(channel, &store);
                        this.update_present_bar();
                        this.update_unread_bar();
                        if this.state.borrow().channel == Some(channel) && this.chat_visible() {
                            this.mark_read(channel);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if this.state.borrow().channel == Some(channel) {
                        this.show_composer_error(&format!("Could not load newer messages: {e}"));
                    }
                }
            }
            this.page_landed(channel);
            if landed {
                this.refetch_if_stale(channel, stale);
            }
        });
    }

    /// A page came back, or failed to. Once the list has laid it out, see
    /// how far it got us and whether another one is due.
    fn page_landed(self: &Rc<Self>, channel: Snowflake) {
        let this = Rc::downgrade(self);
        self.after_layout(move || {
            if let Some(this) = this.upgrade()
                && this.state.borrow().channel == Some(channel)
            {
                this.scroll_loader.borrow_mut().load_finished(this.content_height());
                this.maybe_load(false);
            }
        });
    }

    /// Run `then` once the list has laid out what just changed. A frame runs
    /// its tick callbacks before its layout, so the second tick comes after
    /// one; until then the scroll position is the old model's.
    fn after_layout(&self, then: impl FnOnce() + 'static) {
        let first = Cell::new(true);
        let then = Cell::new(Some(then));
        self.scroller.add_tick_callback(move |_, _| {
            if first.replace(false) {
                return glib::ControlFlow::Continue;
            }
            if let Some(then) = then.take() {
                then();
            }
            glib::ControlFlow::Break
        });
    }

    /// Scroll to a message in the open channel and flash it, first loading
    /// the messages around it if they aren't loaded.
    pub(super) fn jump_to(self: &Rc<Self>, channel: Snowflake, id: Snowflake) {
        if self.state.borrow().channel != Some(channel) {
            return;
        }
        if let Some((position, _)) = timelines::find(&self.messages(), id) {
            self.flash(id, position);
            return;
        }
        self.load_around(channel, id, true, move |this, store| {
            if let Some((position, _)) = timelines::find(store, id) {
                this.flash(id, position);
            }
        });
    }

    /// Show the first unread message of the open channel, loading the
    /// messages around the last one read if need be.
    fn jump_to_unread(self: &Rc<Self>) {
        let Some(Unread { channel, acked, .. }) = self.open_unread() else { return };
        let show = move |this: &Rc<Self>, store: &gio::ListStore| {
            let first = timelines::ids(store).iter().position(|&id| id > acked).map(|p| p as u32);
            if let Some(position) = first {
                this.list.scroll_to(position, gtk::ListScrollFlags::NONE, None);
                this.unread_seen();
            }
        };
        let store = self.messages();
        let loaded = timelines::oldest_id(&store).is_some_and(|oldest| oldest <= acked);
        if loaded {
            show(self, &store);
        } else {
            self.load_around(channel, acked, false, show);
        }
    }

    /// Replace the open channel's messages with those around `id`, then
    /// call `then`. With `must_exist`, a page without `id` is an error.
    fn load_around(
        self: &Rc<Self>,
        channel: Snowflake,
        id: Snowflake,
        must_exist: bool,
        then: impl FnOnce(&Rc<Self>, &gio::ListStore) + 'static,
    ) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        if let Some(timeline) = self.timelines.borrow_mut().get_mut(channel) {
            timeline.loading = true;
        }
        let started = self.timelines.borrow().held.started();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, AROUND_PAGE, Page::Around(id)).await;
            if let Some(timeline) = this.timelines.borrow_mut().get_mut(channel) {
                timeline.loading = false;
            }
            let page = match result {
                Ok(page) => page,
                Err(e) => return this.show_composer_error(&format!("Could not load the message: {e}")),
            };
            if this.state.borrow().channel != Some(channel) {
                return;
            }
            if must_exist && !page.iter().any(|m| m.id == id) {
                return this.show_composer_error("That message no longer exists.");
            }
            this.learn_messages(&page);
            // Like the official client, judge by the page's shape rather than
            // the channel's newest id, which can be stale: a page that comes
            // back short on one side has reached that end of the channel.
            let newest = this.state.borrow().find_channel(channel).and_then(|(_, c)| c.last_message_id);
            let ids: Vec<Snowflake> = page.iter().map(|m| m.id).collect();
            let around = Around::of(&ids, id, newest);
            let Some(store) = this.timelines.borrow_mut().get_mut(channel).map(|timeline| {
                timelines::replace(timeline, page, around);
                timeline.store.clone()
            }) else {
                return;
            };
            let stale = this.timelines.borrow_mut().landed(channel, started);
            // The client asks for the page around the message again when it
            // lands stale; we don't, but its ten seconds still run.
            let open = this.state.borrow().channel == Some(channel);
            this.timelines.borrow_mut().held.refetch(channel, stale, open, Instant::now());
            this.update_present_bar();
            this.update_unread_bar();
            then(&this, &store);
        });
    }

    /// Scroll a message into view and highlight it for a moment.
    fn flash(self: &Rc<Self>, id: Snowflake, position: u32) {
        self.highlight.set(Some(id));
        self.refresh_message(id);
        let list = self.list.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(list) = list.upgrade() {
                list.scroll_to(position, gtk::ListScrollFlags::NONE, None);
            }
        });
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(FLASH, move || {
            if let Some(this) = this.upgrade()
                && this.highlight.get() == Some(id)
            {
                this.highlight.set(None);
                this.refresh_message(id);
            }
        });
    }

    /// Re-bind one message's row, if it is in the open channel.
    pub(super) fn refresh_message(self: &Rc<Self>, id: Snowflake) {
        let messages = self.messages();
        if let Some((position, _)) = timelines::find(&messages, id) {
            self.rebind(&messages, position, 1);
        }
    }

    /// Resolve the mentions a message can contain: the user cache (plus this
    /// message's own mentions) for users, and every channel of the open guild
    /// for channel references.
    pub(super) fn message_names(&self, message: &Message) -> Names {
        let mut names = self.names();
        let original = match &message.referenced_message {
            Referenced::Message(original) => Some(&original.author),
            _ => None,
        };
        for user in message.mentions.iter().chain(std::iter::once(&message.author)).chain(original) {
            names.users.entry(user.id.0).or_insert_with(|| user.display_name().to_string());
        }
        names
    }

    /// The users we know and the open guild's channels.
    fn names(&self) -> Names {
        let state = self.state.borrow();
        let channels = state
            .guild
            .and_then(|id| state.guilds.iter().find(|g| g.id == id))
            .map(|guild| {
                (guild.channels.iter().chain(&guild.threads))
                    .filter_map(|c| c.name.as_ref().map(|name| (c.id.0, name.clone())))
                    .collect()
            })
            .unwrap_or_default();
        let roles = state
            .guild
            .and_then(|id| state.guilds.iter().find(|g| g.id == id))
            .map(|guild| guild.roles.iter().map(|r| (r.id.0, (r.name.clone(), guild.role_colour(r)))).collect())
            .unwrap_or_default();
        Names { users: state.users.clone(), channels, roles }
    }

    /// The line under a channel's name: a server channel's topic, a friend's
    /// status in a DM, or a group's size.
    fn channel_subtitle(&self, id: Snowflake) -> Option<String> {
        let topic = {
            let state = self.state.borrow();
            if let Some(dm) = state.private_channels.iter().find(|c| c.id == id) {
                if dm.kind != discord::ChannelKind::Dm {
                    return Some(format!("{} members", dm.recipients.len() + 1));
                }
                let user = dm.recipients.first()?;
                state.relationships.get(user.id).filter(|r| r.kind == discord::relationships::Kind::Friend)?;
                let presence = state.relationships.presence(user.id);
                return Some(presence.text.unwrap_or_else(|| presence.status.label().to_owned()));
            }
            let (guild, channel) = state.find_channel(id)?;
            // A thread: the channel it's in.
            if channel.kind.is_thread() {
                let parent = guild?.channel(channel.parent_id?)?;
                return Some(format!("# {}", parent.name.as_deref().unwrap_or("unnamed")));
            }
            channel.topic.clone()?
        };
        let text = render::plain_text(&discord::markup::parse(&topic), &self.names());
        text.lines().find(|line| !line.trim().is_empty()).map(str::to_owned)
    }

    /// Record display names for later mention resolution.
    pub(super) fn learn_users<'a>(&self, users: impl Iterator<Item = &'a User>) {
        let mut state = self.state.borrow_mut();
        for user in users {
            state.users.insert(user.id.0, user.display_name().to_string());
        }
    }

    /// For each `<@id>` in `content` we cannot yet name, fetch the user once and
    /// refresh the view when it arrives. Fixes mentions of people who left.
    pub(super) fn resolve_mentions(self: &Rc<Self>, content: &str) {
        let unknown: Vec<Snowflake> = {
            let state = self.state.borrow();
            discord::markup::parse(content)
                .iter()
                .flat_map(collect_user_mentions)
                .filter(|id| !state.users.contains_key(id) && !state.fetched_users.contains(id))
                .map(Snowflake)
                .collect()
        };
        if unknown.is_empty() {
            return;
        }
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest else { return };
        for id in unknown {
            self.state.borrow_mut().fetched_users.insert(id.0);
            let this = self.clone();
            let rest = rest.clone();
            glib::spawn_future_local(async move {
                if let Ok(user) = rest.user(id).await {
                    this.learn_users(std::iter::once(&user));
                    this.refresh_rows();
                }
            });
        }
    }

    /// Fill every row in again so newly resolved names appear, making their
    /// content again too, which [`Self::rebind`] alone leaves as it was.
    pub(super) fn refresh_rows(self: &Rc<Self>) {
        self.row_epoch.set(self.row_epoch.get() + 1);
        let messages = self.messages();
        self.rebind(&messages, 0, messages.n_items());
    }

    pub(super) fn scroll_to_end(&self) {
        self.scroll_to_end_then(|| {});
    }

    /// Scroll to the newest messages and load more if a tall window still
    /// shows the top of what's loaded, as the client does on opening.
    fn settle_at_end(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        self.scroll_to_end_then(move || {
            let Some(app) = this.upgrade() else { return };
            let this = this.clone();
            app.after_layout(move || {
                if let Some(this) = this.upgrade() {
                    this.scroll_loader.borrow_mut().arrived();
                    this.maybe_load(false);
                }
            });
        });
    }

    fn scroll_to_end_then(&self, then: impl FnOnce() + 'static) {
        let list = self.list.downgrade();
        let messages = self.messages();
        let adjustment = self.scroller.vadjustment();
        // After layout, so the new rows have sizes.
        glib::idle_add_local_once(move || {
            if let (Some(list), Some(last)) = (list.upgrade(), messages.n_items().checked_sub(1)) {
                list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
                // That stops once the row shows, which may be under the
                // composer; the list's padding below it goes past it.
                glib::idle_add_local_once(move || {
                    adjustment.set_value(adjustment.upper() - adjustment.page_size());
                    then();
                });
            }
        });
    }
}

/// How long the moves after `event` count as the user's, if it's one that
/// scrolls: the keys are `il` in 567008.
fn scroll_input(event: &gdk::Event) -> Option<Duration> {
    match event.event_type() {
        gdk::EventType::Scroll | gdk::EventType::TouchUpdate => Some(SCROLL_INPUT),
        gdk::EventType::TouchEnd => Some(KINETIC_INPUT),
        gdk::EventType::KeyPress => {
            let key = event.downcast_ref::<gdk::KeyEvent>()?.keyval();
            const KEYS: [gdk::Key; 6] =
                [gdk::Key::Up, gdk::Key::Down, gdk::Key::Page_Up, gdk::Key::Page_Down, gdk::Key::Home, gdk::Key::End];
            KEYS.contains(&key).then_some(SCROLL_INPUT)
        }
        _ => None,
    }
}

/// Open the menu of the message `cell` shows, from a right-click or
/// long-press on it, or from the keys for a menu through `cell`'s
/// `message.menu`, at its top left corner.
fn attach_message_menu(this: &std::rc::Weak<App>, item: &gtk::ListItem, cell: &MessageRow) {
    let row = &cell.parts().row;
    let open = Rc::new({
        let (this, item, row) = (this.clone(), item.downgrade(), row.downgrade());
        move |at: At, link: Option<String>| {
            let (Some(this), Some(item), Some(row)) = (this.upgrade(), item.upgrade(), row.upgrade()) else { return };
            // Hidden rows: collapsed blocked messages, and those that show nothing.
            let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>().filter(|_| row.is_visible()) else {
                return;
            };
            let message = message.borrow::<Message>().clone();
            this.message_menu(&row, message, at, link);
        }
    });
    menus::on_secondary(row, {
        let (open, row) = (open.clone(), row.downgrade());
        move |x, y| {
            let link = row.upgrade().and_then(|row| link_at(row.upcast_ref(), x, y));
            open(At::Point(x, y), link);
        }
    });
    let action = gio::SimpleAction::new("menu", None);
    action.connect_activate(move |_, _| open(At::Point(0.0, 0.0), None));
    let actions = gio::SimpleActionGroup::new();
    actions.add_action(&action);
    cell.insert_action_group("message", Some(&actions));
}

/// Make the Edit, Reply and More buttons act on whichever message `cell` shows.
fn attach_message_actions(this: &std::rc::Weak<App>, cell: &MessageRow) {
    let parts = cell.parts();
    let shown = |cell: &glib::WeakRef<MessageRow>| {
        let cell = cell.upgrade()?;
        let message = cell.parts().shown.borrow().as_ref().map(|(message, _)| message.clone());
        message.map(|message| (cell, message))
    };
    parts.edit_button.connect_clicked({
        let (this, cell) = (this.clone(), cell.downgrade());
        move |_| {
            let (Some(this), Some((_, message))) = (this.upgrade(), shown(&cell)) else { return };
            this.start_edit(&message, None);
        }
    });
    parts.reply_button.connect_clicked({
        let (this, cell) = (this.clone(), cell.downgrade());
        move |_| {
            let (Some(this), Some((_, message))) = (this.upgrade(), shown(&cell)) else { return };
            this.start_reply(&message, this.shift_held());
        }
    });
    parts.more.connect_clicked({
        let (this, cell) = (this.clone(), cell.downgrade());
        move |button| {
            let (Some(this), Some((cell, message))) = (this.upgrade(), shown(&cell)) else { return };
            let row = &cell.parts().row;
            let Some(b) = button.compute_bounds(row) else { return };
            let rect = gdk::Rectangle::new(b.x() as i32, b.y() as i32, b.width() as i32, b.height() as i32);
            this.message_menu(row, message, At::Below(rect), None);
        }
    });
}

/// The text selected in `row`, in the label with the focus, as Ctrl+C would
/// copy it.
fn selection_in(row: &gtk::Widget) -> Option<String> {
    let label = row.root()?.focus().and_downcast::<gtk::Label>().filter(|l| l.is_ancestor(row))?;
    let text = match label.parent().and_downcast::<RichText>() {
        Some(text) => text.selected_text()?,
        None => {
            let (start, end) = label.selection_bounds()?;
            label.text().chars().skip(start as usize).take((end - start) as usize).collect()
        }
    };
    (!text.is_empty()).then_some(text)
}

/// Where the link at `(x, y)` in `row` goes.
fn link_at(row: &gtk::Widget, x: f64, y: f64) -> Option<String> {
    let mut widget = row.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(w) = widget.filter(|w| w != row) {
        if let Some(text) = w.downcast_ref::<RichText>() {
            let point = row.compute_point(text, &graphene::Point::new(x as f32, y as f32))?;
            return text.link_at(point.x() as f64, point.y() as f64);
        }
        // Other labels are all one link, if any: file names, embed titles.
        if let Some(label) = w.downcast_ref::<gtk::Label>()
            && !label.parent().is_some_and(|p| p.is::<RichText>())
        {
            return label.current_uri().map(String::from);
        }
        widget = w.parent();
    }
    None
}

/// User ids referenced by `<@id>` mentions within a parsed span, at any
/// depth.
pub(super) fn collect_user_mentions(span: &discord::markup::Span) -> Vec<u64> {
    use discord::markup::{Entity, Span};
    std::iter::once(span)
        .chain(span.descendants())
        .filter_map(|span| match span {
            Span::Entity(Entity::User(id)) => Some(*id),
            _ => None,
        })
        .collect()
}

pub(super) fn parse_time(timestamp: &str) -> Option<glib::DateTime> {
    glib::DateTime::from_iso8601(timestamp, None).ok()?.to_local().ok()
}

pub(super) fn format_time(time: &glib::DateTime) -> String {
    let today = glib::DateTime::now_local().ok().map(|now| now.ymd());
    let pattern = if today == Some(time.ymd()) { "Today at %H:%M" } else { "%Y-%m-%d %H:%M" };
    time.format(pattern).map(String::from).unwrap_or_default()
}

/// When a forwarded message was first sent: the time today, the day this
/// week, else the date.
fn forward_time(time: &glib::DateTime) -> String {
    let Ok(now) = glib::DateTime::now_local() else { return String::new() };
    let days = now.difference(time).as_seconds() / 86_400;
    let pattern = if now.ymd() == time.ymd() {
        "%H:%M"
    } else if now.add_days(-1).is_ok_and(|yesterday| yesterday.ymd() == time.ymd()) {
        "Yesterday at %H:%M"
    } else if days < 7 {
        "%A"
    } else {
        "%Y-%m-%d"
    };
    time.format(pattern).map(String::from).unwrap_or_default()
}

pub(super) fn is_grouped(message: &Message, previous: Option<&Message>) -> bool {
    let Some(previous) = previous.filter(|p| p.is_user_message()) else { return false };
    // A reply always shows what it answers, under a full header.
    if previous.author.id != message.author.id || message.reply_to().is_some() {
        return false;
    }
    match (parse_time(&previous.timestamp), parse_time(&message.timestamp)) {
        (Some(a), Some(b)) => b.difference(&a).as_seconds() < GROUP_WINDOW_SECS && a.ymd() == b.ymd(),
        _ => false,
    }
}

impl App {
    /// `text` naming `user` in `channel`, in the colour of their highest role.
    fn name_label(&self, channel: Snowflake, user: &discord::User, text: &str) -> gtk::Label {
        let label = gtk::Label::builder().css_classes(["heading"]).build();
        self.set_name(&label, channel, user, text);
        label
    }

    /// Put `text` on `label` as [`Self::name_label`] makes it.
    fn set_name(&self, label: &gtk::Label, channel: Snowflake, user: &discord::User, text: &str) {
        match self.state.borrow().name_colour(channel, user.id) {
            Some(colour) => label.set_markup(&render::coloured(text, &colour)),
            // Unlike `set_label`, this drops the markup a colour left on.
            None => label.set_text(text),
        }
    }

    /// Fill a row in for `message`. Its content and reply line are made again
    /// only when the message or `row_epoch` has changed since they were
    /// made: a re-bind for a highlight or new neighbours changes the rest.
    /// `messages` is the store the row belongs to.
    fn fill_message_row(
        self: &Rc<Self>,
        parts: &Parts,
        message: &Message,
        previous: Option<&Message>,
        messages: &gio::ListStore,
    ) {
        let row = &parts.row;
        let kind = self.message_kind(message);
        if kind == Kind::Hidden {
            row.set_visible(false);
            return;
        }
        let grouped = kind == Kind::Normal && is_grouped(message, previous);
        let mut classes = vec!["message"];
        if !grouped {
            classes.push("first");
        }
        if self.highlight.get() == Some(message.id) {
            classes.push("highlight");
        }
        if self.reply.borrow().as_ref().is_some_and(|r| r.to == message.id) {
            classes.push("replying");
        }
        if self.editing() == Some(message.id) {
            classes.push("editing");
        }
        let editable = self.offers_edit(message);
        if editable {
            classes.push("editable");
        }
        row.set_css_classes(&classes);

        // A thread's first message shows the one the thread was started from.
        let shown = message.thread_starter().unwrap_or(message);
        let system = match &kind {
            Kind::System(system) => Some(system),
            _ => None,
        };
        match system {
            Some(system) => parts.show_icon(system_icon(system.icon)),
            None if grouped => parts.hide_gutter(),
            None => parts.show_avatar(&shown.author, || avatar_widget(&shown.author, 40, Some(&self.images))),
        }
        parts.header.set_visible(system.is_none() && !grouped);
        if parts.header.is_visible() {
            self.set_name(&parts.name, shown.channel_id, &shown.author, shown.author.display_name());
            parts.time.set_label(&parse_time(&shown.timestamp).map(|t| format_time(&t)).unwrap_or_default());
        }
        parts.edit_button.set_visible(editable);
        parts.reply_button.set_visible(self.offers_reply(message));

        let epoch = self.row_epoch.get();
        let changed = parts.shown.borrow().as_ref().is_none_or(|(was, at)| *at != epoch || was != message);
        let reply_to = message.reply_to();
        // Discord didn't resolve what this answers, but it may have loaded since.
        let unresolved = reply_to.is_some() && matches!(message.referenced_message, Referenced::Unresolved);
        if changed || unresolved {
            if let Some(old) = parts.reply.take() {
                row.remove(&old);
            }
            if let Some(to) = reply_to {
                let preview = self.reply_preview(message, to, messages);
                row.prepend(&preview);
                parts.reply.replace(Some(preview));
            }
        }
        if !changed {
            return;
        }
        let content = &parts.content;
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }
        if let Some(system) = system {
            content.append(&self.system_line(message, system));
            if let Some(outcome) = discord::poll::outcome(message) {
                content.append(&polls::outcome(&outcome, &self.images));
            }
        } else {
            self.message_content(content, shown, &self.message_names(shown));
        }
        // Empty, it would still take the body's spacing.
        content.set_visible(content.first_child().is_some());
        parts.shown.replace(Some((message.clone(), epoch)));
    }

    /// Open the menu for `message`, whose row is `row`, at `at` there.
    fn message_menu(self: &Rc<Self>, row: &gtk::Box, message: Message, at: At, link: Option<String>) {
        let target = MessageTarget { selection: selection_in(row.upcast_ref()), link, message };
        let regulated = self.state.borrow().media_takedown_regulated;
        menus::popup(self, menus::message(regulated), target, row, at);
    }

    /// The spoilers revealed in a message's text, remembered while the app
    /// runs as its row is rebuilt.
    fn spoilers(self: &Rc<Self>, message: Snowflake) -> Spoilers {
        let revealed = self.state.borrow().revealed_spoilers.get(&message).cloned().unwrap_or_default();
        let this = Rc::downgrade(self);
        let on_reveal = move |n| {
            if let Some(this) = this.upgrade() {
                this.state.borrow_mut().revealed_spoilers.entry(message).or_default().insert(n);
            }
        };
        Spoilers { revealed, on_reveal: Rc::new(on_reveal) }
    }

    /// What a message says, and shows: its text, attachments and embeds.
    fn message_content(self: &Rc<Self>, body: &gtk::Box, message: &Message, names: &Names) {
        let media = self.state.borrow().media;
        let edited = message.edited_timestamp.is_some();
        let embeds = discord::embed::visible(message, &media);
        let spans = message.markup();
        let cx = Context {
            spoilers: Some(self.spoilers(message.id)),
            jumbo: discord::markup::is_jumbo(&spans),
            ..Context::new(names, Some(&self.images))
        };
        if discord::embed::hides_content(&message.content, &embeds, media.inline_embed_media) {
            // A bare GIF or image link: the media below stands for it.
        } else if !message.content.is_empty() {
            body.append(&render::render(&spans, &cx, edited));
        } else if edited {
            body.append(&render::render(&[], &cx, edited));
        }
        if let Some(poll) = &message.poll {
            body.append(&polls::poll(poll, &self.images));
        }
        let voice = message.is_voice_message();
        // A poll's attachments are its answers' images, which the official
        // client shows with them, not under it.
        // Nor do messages built from components show theirs: the components do.
        let shown = message.poll.is_none() && !message.is_components_v2();
        let attached = message.attachments.iter().filter(|a| !a.is_hidden() && shown);
        for attachment in attached {
            body.append(&attachments::attachment(attachment, voice, &self.images, &media));
        }
        for sticker in &message.sticker_items {
            body.append(&attachments::sticker(sticker, &self.images, media.gif_auto_play));
        }
        // A link in a spoiler covers all the message's embeds, as in Discord.
        let spoilered = discord::embed::has_spoilered_link(&message.content);
        for embed in embeds {
            if spoilered {
                let (names, images) = (names.clone(), self.images.clone());
                body.append(&behind_spoiler(move || embed_widget(&embed, &names, Some(&images), &media)));
            } else {
                body.append(&embed_widget(&embed, names, Some(&self.images), &media));
            }
        }
        if !message.components.is_empty() {
            body.append(&components::components(&message.components, &cx));
        }
        if let Some(original) = message.forwarded() {
            body.append(&self.forward(message, &original, names));
        }
    }

    /// A forwarded message, under a bar as quotes are: what it said, and
    /// where and when it was first sent. Clicking that goes there.
    fn forward(self: &Rc<Self>, message: &Message, original: &Message, names: &Names) -> gtk::Widget {
        let bar = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).css_classes(["quote"]).build();
        let header = gtk::Box::builder().spacing(4).css_classes(["caption", "dimmed"]).build();
        header.append(&gtk::Image::from_icon_name("mail-forward-symbolic"));
        header.append(&gtk::Label::new(Some("Forwarded")));
        bar.append(&header);
        self.message_content(&bar, original, names);

        // Where it came from, as the official client names it: the channel
        // if it is in this server, else the server.
        let origin = {
            let state = self.state.borrow();
            let here = state.find_channel(message.channel_id).and_then(|(guild, _)| guild.map(|g| g.id));
            match state.find_channel(original.channel_id) {
                Some((Some(guild), channel)) if Some(guild.id) == here => {
                    channel.name.as_ref().map(|name| format!("#{name}"))
                }
                Some((Some(guild), _)) => Some(guild.name.clone()),
                Some((None, dm)) => Some(dm.title()),
                None => {
                    original.guild_id.and_then(|id| state.guilds.iter().find(|g| g.id == id)).map(|g| g.name.clone())
                }
            }
        };
        let time = parse_time(&original.timestamp).map(|t| forward_time(&t));
        let footer: Vec<String> = origin.into_iter().chain(time).collect();
        if !footer.is_empty() {
            let label = gtk::Label::builder()
                .label(footer.join(" • "))
                .xalign(0.0)
                .css_classes(["caption", "dimmed"])
                .tooltip_text("Go to the original message")
                .build();
            label.set_cursor_from_name(Some("pointer"));
            let click = gtk::GestureClick::new();
            click.connect_released({
                let this = Rc::downgrade(self);
                let (channel, id) = (original.channel_id, original.id);
                move |_, _, _, _| {
                    let Some(this) = this.upgrade() else { return };
                    if this.state.borrow().channel == Some(channel) {
                        this.jump_to(channel, id);
                    } else if this.state.borrow().can_view(channel) {
                        this.open_channel(channel);
                    }
                }
            });
            label.add_controller(click);
            bar.append(&label);
        }
        bar.upcast()
    }

    /// How a message shows: as written, as a system message, or not at all.
    fn message_kind(&self, message: &Message) -> Kind {
        let state = self.state.borrow();
        let channel = state.find_channel(message.channel_id);
        let place = system::Place {
            me: state.me(),
            guild_name: channel.and_then(|(guild, _)| guild).map(|g| g.name.as_str()),
            group: channel.is_some_and(|(_, c)| c.kind == discord::ChannelKind::GroupDm),
        };
        system::kind(message, &place)
    }

    /// A system message's line, and when it happened. Its link to a message
    /// jumps there.
    fn system_line(self: &Rc<Self>, message: &Message, system: &system::System) -> gtk::Widget {
        let time = parse_time(&message.timestamp).map(|t| format_time(&t)).unwrap_or_default();
        let text = RichText::new(&render::system_markup(&system.parts, &time), Vec::new(), Vec::new(), Vec::new());
        text.label().connect_activate_link({
            let this = Rc::downgrade(self);
            let channel = message.channel_id;
            move |_, uri| {
                let Some(to) = render::jump_target(uri) else { return glib::Propagation::Proceed };
                if let Some(this) = this.upgrade() {
                    this.jump_to(channel, to);
                }
                glib::Propagation::Stop
            }
        });
        text.upcast()
    }

    /// The line above a reply: who it answers and the start of what they said.
    /// Clicking it jumps to the original.
    fn reply_preview(self: &Rc<Self>, message: &Message, to: Snowflake, messages: &gio::ListStore) -> gtk::Widget {
        let preview = gtk::Box::builder().spacing(6).css_classes(["reply-preview"]).build();
        preview.append(&gtk::Box::builder().valign(gtk::Align::End).css_classes(["reply-spine"]).build());
        let found;
        let original = match &message.referenced_message {
            Referenced::Message(original) => Some(original.as_ref()),
            Referenced::Deleted => None,
            // Discord didn't resolve it, but we may have it loaded.
            Referenced::Unresolved => {
                found = timelines::find(messages, to).map(|(_, m)| m);
                found.as_ref()
            }
        };
        let snippet = |text: &str, classes: &[&str]| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(classes)
                .build()
        };
        match original {
            Some(original) => {
                // As in Discord, a blocked author keeps their name but not
                // their avatar or words.
                let blocked = self.state.borrow().relationships.is_blocked(original.author.id);
                if blocked {
                    preview.append(&gtk::Image::from_icon_name("mail-reply-sender-symbolic"));
                } else {
                    preview.append(&avatar_widget(&original.author, 16, Some(&self.images)));
                }
                // Discord marks replies that pinged the author with an @.
                let pinged = message.mentions.iter().any(|u| u.id == original.author.id);
                let name = format!("{}{}", if pinged { "@" } else { "" }, original.author.display_name());
                preview.append(&self.name_label(original.channel_id, &original.author, &name));
                let names = self.message_names(original);
                // A forward reads as what it forwards.
                let content = original.forwarded().map_or_else(|| original.content.clone(), |f| f.content);
                let text = render::plain_text(&discord::markup::parse(&content), &names);
                let text = match text.lines().find(|line| !line.trim().is_empty()) {
                    _ if blocked => "Blocked message".to_owned(),
                    _ if let Kind::System(system) = self.message_kind(original) => system.text(),
                    Some(line) => line.to_owned(),
                    // The official client's wording for messages with no text.
                    None if !original.sticker_items.is_empty() => "Click to see sticker".to_owned(),
                    None => "Click to see attachment".to_owned(),
                };
                preview.append(&snippet(&text, &["reply-snippet"]));
            }
            None => {
                let text = match message.referenced_message {
                    Referenced::Deleted => "Original message was deleted",
                    _ => "Message could not be loaded",
                };
                preview.append(&snippet(text, &["reply-snippet", "missing"]));
            }
        }
        if !matches!(message.referenced_message, Referenced::Deleted) {
            preview.set_cursor_from_name(Some("pointer"));
            let click = gtk::GestureClick::new();
            click.connect_released({
                let this = Rc::downgrade(self);
                let channel = message.channel_id;
                move |_, _, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.jump_to(channel, to);
                    }
                }
            });
            preview.add_controller(click);
        }
        preview.upcast()
    }
}

fn system_icon(icon: system::Icon) -> &'static str {
    use system::Icon;
    match icon {
        Icon::Join => "go-next-symbolic",
        Icon::Leave => "go-previous-symbolic",
        Icon::Call => "call-start-symbolic",
        Icon::MissedCall => "call-missed-symbolic",
        Icon::Pin => "view-pin-symbolic",
        Icon::Boost => "starred-symbolic",
        Icon::Edit => "document-edit-symbolic",
        Icon::Thread => "chat-message-new-symbolic",
        Icon::Stage => "audio-speakers-symbolic",
        Icon::Alert => "dialog-warning-symbolic",
        Icon::Poll => "view-list-bullet-symbolic",
        Icon::Other => "dialog-information-symbolic",
    }
}
