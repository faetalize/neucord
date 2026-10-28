//! The message list: opening a channel, scrollback, live updates, and
//! building each message row.

use std::rc::Rc;

use discord::read_state::starts_unread;
use discord::rest::Page;
use discord::system::{self, Kind};
use discord::{Message, MessageEdit, Referenced, Snowflake, User};
use gtk::prelude::*;
use gtk::{gio, glib};

use super::App;
use super::embeds::{behind_spoiler, embed_widget};
use super::state::{Unread, now_unix};
use super::widgets::avatar_widget;
use super::{attachments, components, polls};
use crate::render::{Context, Names, Spoilers};
use crate::rich_text::RichText;
use crate::{render, timelines};

/// Messages fetched when a channel opens: enough to fill the view, small so
/// the channel appears fast. Also what a channel keeps once we leave it.
const FIRST_PAGE: u8 = 40;
/// Messages per scrollback request, up or down (Discord's maximum): these
/// load in the background, so bigger pages just mean fewer round trips.
const SCROLL_PAGE: u8 = 100;
/// Messages fetched around one we jump to (the official client's number).
const JUMP_PAGE: u8 = 30;
/// How long a message we jumped to stays highlighted.
const FLASH: std::time::Duration = std::time::Duration::from_secs(2);
/// Start fetching more messages this many screens before reaching the end,
/// so they are usually in place before the user gets there.
const PREFETCH_SCREENS: f64 = 3.0;
/// Space between the last message and the composer, as around the composer.
const COMPOSER_GAP: i32 = 12;
/// Consecutive messages by one author within this window share a header.
const GROUP_WINDOW_SECS: i64 = 7 * 60;

impl App {
    /// The chat column: the message list (with scrollback) from edge to edge,
    /// and the header bar and composer floating over it.
    pub(super) fn build_chat(self: &Rc<Self>) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_activatable(false);
            // A divider for a new day or the first unread message, a bar for
            // collapsed blocked messages, then the message itself.
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
            cell.append(
                &gtk::Box::builder().visible(false).spacing(8).margin_top(12).margin_start(18).margin_end(18).build(),
            );
            cell.append(&gtk::Box::builder().visible(false).css_classes(["blocked-bar"]).build());
            cell.append(&gtk::Box::new(gtk::Orientation::Vertical, 2));
            item.set_child(Some(&cell));
        });
        factory.connect_bind({
            let this = Rc::downgrade(self);
            move |_, item| {
                let Some(this) = this.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                let cell = item.child().expect("cell");
                let divider = cell.first_child().and_downcast::<gtk::Box>().expect("divider");
                let bar = divider.next_sibling().and_downcast::<gtk::Box>().expect("blocked bar");
                let row = bar.next_sibling().and_downcast::<gtk::Box>().expect("row box");
                let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
                let message = message.borrow::<Message>().clone();
                let messages = this.messages();
                let position = item.position();
                let previous = position.checked_sub(1).and_then(|p| timelines::message_at(&messages, p));
                let divided = this.fill_divider(&divider, &message, previous.as_ref());
                let (run, revealed) = {
                    let state = this.state.borrow();
                    let run = timelines::run_at(&messages, position, |m| state.relationships.is_blocked(m.author.id));
                    (run, run.is_some_and(|r| state.revealed_blocked.contains(&r.first)))
                };
                match run {
                    Some(run) if run.start == position => this.fill_blocked_bar(&bar, &messages, run, revealed),
                    _ => bar.set_visible(false),
                }
                row.set_visible(run.is_none() || revealed);
                if !row.is_visible() {
                    return;
                }
                this.fill_message_row(&row, &message, previous.as_ref().filter(|_| !divided), &messages);
            }
        });
        self.list.set_factory(Some(&factory));
        self.list.add_css_class("messages");
        self.scroller.set_child(Some(&self.list));
        self.scroller.add_css_class("messages");

        // Fetch older messages well before the top is reached, and after a
        // jump, newer ones well before the bottom.
        self.scroller.vadjustment().connect_value_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.maybe_load_older();
                    this.maybe_load_newer();
                    this.check_unread_seen();
                }
            }
        });

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
        let header = self.header.header_bar(&[self.sidebar_toggle().upcast_ref()]);
        page.add_top_bar(&header);
        page.add_top_bar(&self.present_bar);
        page.add_top_bar(&self.unread_bar);
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
        let viewing = self.state.borrow().channel == Some(channel) && self.chat_visible();
        let notify = {
            let mut state = self.state.borrow_mut();
            state.bump_last_message(channel, message.id);
            let own = state.me() == Some(message.author.id);
            // No notifications for message requests (as in Discord) or from blocked users.
            let quiet = state.find_channel(channel).is_some_and(|(_, c)| c.is_request())
                || state.relationships.is_blocked(message.author.id);
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
                let is_dm = place.is_some_and(|p| p.guild.is_none());
                let mentioned =
                    is_dm || state.me().is_some_and(|me| state.notifications.mentions_me(&message, me, &roles));
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
                        _ => place.is_some_and(|p| state.notifications.should_notify(&p, mentioned, now_unix())),
                    };
                state.read.on_message(channel, mentioned);
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
    }

    pub(super) fn at_bottom(&self) -> bool {
        let adjustment = self.scroller.vadjustment();
        adjustment.value() + adjustment.page_size() >= adjustment.upper() - 80.0
    }

    pub(super) fn on_message_update(&self, edit: MessageEdit) {
        let Some(store) = self.timelines.borrow().get(edit.channel_id).map(|t| t.store.clone()) else { return };
        let (id, content) = (edit.id, edit.content.clone());
        if let Some((position, mut message)) = timelines::find(&store, id) {
            edit.apply(&mut message);
            store.splice(position, 1, &[timelines::boxed(message)]);
        }
        // Replies show the start of what they answer; keep it current.
        if let Some(content) = content {
            timelines::update_replies(&store, id, |original| {
                if let Referenced::Message(original) = original {
                    original.content = content.clone();
                }
            });
        }
    }

    pub(super) fn on_message_delete(&self, id: Snowflake, channel_id: Snowflake) {
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, _)) = timelines::find(&store, id) {
            store.remove(position);
            // The message after it may now group with the one before.
            if position < store.n_items() {
                timelines::rebind(&store, position, 1);
            }
            // A collapsed run it belonged to now counts one less.
            if let Some(before) = position.checked_sub(1) {
                self.rebind_blocked_run(&store, before);
            }
        }
        timelines::update_replies(&store, id, |original| *original = Referenced::Deleted);
        // Nothing left to reply to, as in the official client.
        if self.reply.borrow().as_ref().is_some_and(|r| r.to == id) {
            self.set_reply(None);
        }
    }

    /// Above a message: the date where a new day starts, and "New" above the
    /// first unread message. Returns whether there is a divider, which
    /// starts a new group of messages.
    fn fill_divider(&self, divider: &gtk::Box, message: &Message, previous: Option<&Message>) -> bool {
        while let Some(child) = divider.first_child() {
            divider.remove(&child);
        }
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
        divider.set_visible(new_day || unread);
        if !divider.is_visible() {
            return false;
        }
        let line = || gtk::Separator::builder().hexpand(true).valign(gtk::Align::Center).build();
        divider.append(&line());
        if let Some(time) = time.filter(|_| new_day) {
            let date = time.format("%e %B %Y").map(|d| d.trim().to_owned()).unwrap_or_default();
            divider.append(&gtk::Label::builder().label(date).css_classes(["caption", "dimmed"]).build());
            divider.append(&line());
        }
        if unread {
            divider.append(&gtk::Label::builder().label("New").css_classes(["caption-heading", "accent"]).build());
        }
        true
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
    pub(super) fn update_present_bar(&self) {
        let channel = self.state.borrow().channel;
        let detached = channel.and_then(|c| self.timelines.borrow().get(c).map(|t| t.detached)).unwrap_or(false);
        self.present_bar.set_revealed(detached);
    }

    /// The bar that stands in for a run of blocked users' messages: how many
    /// there are, and a button to show or hide them.
    fn fill_blocked_bar(self: &Rc<Self>, bar: &gtk::Box, store: &gio::ListStore, run: timelines::Run, revealed: bool) {
        while let Some(child) = bar.first_child() {
            bar.remove(&child);
        }
        let count = if run.len == 1 { "1 blocked message".to_owned() } else { format!("{} blocked messages", run.len) };
        bar.append(&gtk::Label::builder().label(count).xalign(0.0).build());
        let button = gtk::Button::builder().label(if revealed { "Hide" } else { "Show" }).css_classes(["link"]).build();
        let this = Rc::downgrade(self);
        let store = store.downgrade();
        button.connect_clicked(move |_| {
            let (Some(this), Some(store)) = (this.upgrade(), store.upgrade()) else { return };
            {
                let mut state = this.state.borrow_mut();
                if !state.revealed_blocked.remove(&run.first) {
                    state.revealed_blocked.insert(run.first);
                }
            }
            timelines::rebind(&store, run.start, run.len);
        });
        bar.append(&button);
        bar.set_visible(true);
    }

    /// Re-bind the first row of the blocked run at `position`, if any, so its
    /// bar shows the current count.
    fn rebind_blocked_run(&self, store: &gio::ListStore, position: u32) {
        let run = {
            let state = self.state.borrow();
            timelines::run_at(store, position, |m| state.relationships.is_blocked(m.author.id))
        };
        if let Some(run) = run {
            timelines::rebind(store, run.start, 1);
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
        // The channel we left keeps only its newest messages: live events keep
        // those current, and coming back refetches the newest page anyway.
        if let Some(left) = left
            && let Some(timeline) = self.timelines.borrow_mut().get_mut(left)
        {
            timelines::keep_newest(timeline, FIRST_PAGE as u32);
        }
        if self.reply.borrow().as_ref().is_some_and(|r| r.channel != id) {
            self.set_reply(None);
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
        // then fetch the newest page and merge it in.
        let store = self.timelines.borrow_mut().open(id).store.clone();
        self.selection.set_model(Some(&store));
        self.update_present_bar();
        self.update_unread_bar();
        self.scroll_to_end();

        if rest.is_some() {
            self.load_newest(id);
        }
    }

    /// Fetch the newest page and merge it into the channel's store. This is
    /// also how a jump returns to the present.
    pub(super) fn load_newest(self: &Rc<Self>, id: Snowflake) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let Some((store, was_detached)) = self.timelines.borrow().get(id).map(|t| (t.store.clone(), t.detached)) else {
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(id, FIRST_PAGE, Page::Newest).await;
            match result {
                Ok(page) => {
                    this.learn_messages(&page);
                    let full = page.len() == FIRST_PAGE as usize;
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
                    if let Some(newest) = store.n_items().checked_sub(1).and_then(|p| timelines::message_at(&store, p))
                    {
                        // READY's newest-message ids can be stale; history is not.
                        this.state.borrow_mut().bump_last_message(id, newest.id);
                    }
                    // The user may have switched channels while this was loading.
                    if open && this.state.borrow().channel == Some(id) {
                        if at_bottom {
                            this.scroll_to_end();
                        }
                        if this.chat_visible() {
                            this.mark_read(id);
                        }
                        this.load_older_if_short();
                    }
                }
                Err(e) => {
                    if this.state.borrow().channel == Some(id) {
                        this.show_composer_error(&format!("Could not load messages: {e}"));
                    }
                }
            }
        });
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

    /// Fetch the page before the oldest loaded message when the user is within
    /// a few screens of the top. One request per channel at a time.
    pub(super) fn maybe_load_older(self: &Rc<Self>) {
        let adjustment = self.scroller.vadjustment();
        if adjustment.value() > adjustment.page_size() * PREFETCH_SCREENS {
            return;
        }
        let Some(channel) = self.state.borrow().channel else { return };
        let (store, before) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(channel) else { return };
            if timeline.complete || timeline.loading_older {
                return;
            }
            // Nothing loaded yet: the newest page is still on its way.
            let Some(before) = timelines::oldest_id(&timeline.store) else { return };
            timeline.loading_older = true;
            (timeline.store.clone(), before)
        };
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, SCROLL_PAGE, Page::Before(before)).await;
            let still_cached = this.timelines.borrow().get(channel).is_some_and(|t| t.store == store);
            if let Some(timeline) = this.timelines.borrow_mut().get_mut(channel) {
                timeline.loading_older = false;
            }
            if !still_cached {
                return;
            }
            match result {
                // Only prepend if the store still starts where we asked from;
                // a merge that replaced it would make this page a gap.
                Ok(page) if timelines::oldest_id(&store) == Some(before) => {
                    if page.len() < SCROLL_PAGE as usize
                        && let Some(timeline) = this.timelines.borrow_mut().get_mut(channel)
                    {
                        timeline.complete = true;
                    }
                    this.learn_messages(&page);
                    timelines::prepend(&store, page);
                    this.update_unread_bar();
                    this.load_older_if_short();
                }
                Ok(_) => {}
                Err(e) => {
                    if this.state.borrow().channel == Some(channel) {
                        this.show_composer_error(&format!("Could not load older messages: {e}"));
                    }
                }
            }
        });
    }

    /// After a jump, fetch the page after the newest loaded message when the
    /// user nears the bottom, until the store reaches the present again.
    pub(super) fn maybe_load_newer(self: &Rc<Self>) {
        let adjustment = self.scroller.vadjustment();
        let below = adjustment.upper() - adjustment.value() - adjustment.page_size();
        if below > adjustment.page_size() * PREFETCH_SCREENS {
            return;
        }
        let Some(channel) = self.state.borrow().channel else { return };
        let (store, after) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(channel) else { return };
            if !timeline.detached || timeline.loading_newer {
                return;
            }
            let Some(after) = timelines::newest_id(&timeline.store) else { return };
            timeline.loading_newer = true;
            (timeline.store.clone(), after)
        };
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, SCROLL_PAGE, Page::After(after)).await;
            let still_detached = {
                let mut timelines = this.timelines.borrow_mut();
                let timeline = timelines.get_mut(channel).filter(|t| t.store == store);
                timeline.is_some_and(|t| {
                    t.loading_newer = false;
                    t.detached
                })
            };
            match result {
                Ok(page) if still_detached && timelines::newest_id(&store) == Some(after) => {
                    let reached_newest = page.len() < SCROLL_PAGE as usize;
                    this.learn_messages(&page);
                    timelines::append(&store, page);
                    if reached_newest {
                        // Merging the newest page also picks up whatever
                        // arrived live while we were detached.
                        this.load_newest(channel);
                    } else {
                        this.load_newer_if_short();
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if this.state.borrow().channel == Some(channel) {
                        this.show_composer_error(&format!("Could not load newer messages: {e}"));
                    }
                }
            }
        });
    }

    fn load_newer_if_short(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(this) = this.upgrade() {
                this.maybe_load_newer();
            }
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
        let this = self.clone();
        glib::spawn_future_local(async move {
            let page = match rest.messages(channel, JUMP_PAGE, Page::Around(id)).await {
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
            // back short after the target has reached the present.
            let newest = this.state.borrow().find_channel(channel).and_then(|(_, c)| c.last_message_id);
            let after = page.iter().filter(|m| m.id > id).count();
            let at_newest = page.last().is_some_and(|last| Some(last.id) == newest);
            let detached = after >= JUMP_PAGE as usize / 2 && !at_newest;
            let Some(store) = this.timelines.borrow_mut().get_mut(channel).map(|timeline| {
                timelines::replace(timeline, page, detached);
                timeline.store.clone()
            }) else {
                return;
            };
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
    pub(super) fn refresh_message(&self, id: Snowflake) {
        let messages = self.messages();
        if let Some((position, _)) = timelines::find(&messages, id) {
            timelines::rebind(&messages, position, 1);
        }
    }

    /// After layout, page again if the loaded messages still don't reach far
    /// enough up (a short page, or a tall window).
    pub(super) fn load_older_if_short(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(this) = this.upgrade() {
                this.maybe_load_older();
            }
        });
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

    /// Re-bind visible rows so newly resolved names appear.
    pub(super) fn refresh_rows(&self) {
        let messages = self.messages();
        timelines::rebind(&messages, 0, messages.n_items());
    }

    pub(super) fn scroll_to_end(&self) {
        let list = self.list.downgrade();
        let messages = self.messages();
        let adjustment = self.scroller.vadjustment();
        // After layout, so the new rows have sizes.
        glib::idle_add_local_once(move || {
            if let (Some(list), Some(last)) = (list.upgrade(), messages.n_items().checked_sub(1)) {
                list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
                // That stops once the row shows, which may be under the
                // composer; the list's padding below it goes past it.
                glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper() - adjustment.page_size()));
            }
        });
    }
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
        match self.state.borrow().name_colour(channel, user.id) {
            Some(colour) => label.set_markup(&render::coloured(text, &colour)),
            None => label.set_label(text),
        }
        label
    }

    /// Rows are rebuilt on every bind: simple, and cheap at chat-history sizes.
    /// `messages` is the store the row belongs to.
    fn fill_message_row(
        self: &Rc<Self>,
        row: &gtk::Box,
        message: &Message,
        previous: Option<&Message>,
        messages: &gio::ListStore,
    ) {
        while let Some(child) = row.first_child() {
            row.remove(&child);
        }
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
        row.set_css_classes(&classes);
        if let Some(to) = message.reply_to() {
            row.append(&self.reply_preview(message, to, messages));
        }

        let main = gtk::Box::builder().spacing(16).css_classes(["message-main"]).build();
        let gutter = gtk::Box::builder().width_request(40).valign(gtk::Align::Start).build();
        if let Kind::System(system) = &kind {
            gutter.append(&gtk::Image::builder().icon_name(system_icon(system.icon)).halign(gtk::Align::End).build());
        } else if !grouped {
            gutter.append(&avatar_widget(&message.author, 40, Some(&self.images)));
        }
        main.append(&gutter);

        let names = self.message_names(message);
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
        if let Kind::System(system) = &kind {
            body.append(&self.system_line(message, system));
            if let Some(outcome) = discord::poll::outcome(message) {
                body.append(&polls::outcome(&outcome, &self.images));
            }
        } else if !grouped {
            let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = self.name_label(message.channel_id, &message.author, message.author.display_name());
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            header.append(&name);
            let time = parse_time(&message.timestamp).map(|t| format_time(&t)).unwrap_or_default();
            header.append(&gtk::Label::builder().label(time).css_classes(["caption", "dimmed"]).build());
            body.append(&header);
        }
        if kind == Kind::Normal {
            self.message_content(&body, message, &names);
        }
        main.append(&body);

        if message.is_replyable() && self.state.borrow().can_reply(message.channel_id) {
            let reply = gtk::Button::builder()
                .icon_name("mail-reply-sender-symbolic")
                .tooltip_text("Reply (Shift: without pinging)")
                .halign(gtk::Align::End)
                .valign(gtk::Align::Start)
                .css_classes(["flat", "message-action"])
                .build();
            reply.connect_clicked({
                let this = Rc::downgrade(self);
                let message = message.clone();
                move |button| {
                    let shift =
                        button.display().default_seat().and_then(|seat| seat.keyboard()).is_some_and(|keyboard| {
                            keyboard.modifier_state().contains(gtk::gdk::ModifierType::SHIFT_MASK)
                        });
                    if let Some(this) = this.upgrade() {
                        this.start_reply(&message, shift);
                    }
                }
            });
            // Over the row, not in it: as part of the layout it would make
            // every one-line message as tall as the button.
            let overlay = gtk::Overlay::builder().child(&main).build();
            overlay.add_overlay(&reply);
            row.append(&overlay);
        } else {
            row.append(&main);
        }
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
        let text = RichText::new(&render::system_markup(&system.parts, &time), Vec::new(), Vec::new());
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
