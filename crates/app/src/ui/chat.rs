//! The message list: opening a channel, scrollback, live updates, and
//! building each message row.

use std::rc::Rc;

use discord::{Message, Snowflake, User};
use gtk::prelude::*;
use gtk::{gio, glib};

use super::App;
use super::state::now_unix;
use super::widgets::{avatar_widget, palette_index};
use crate::images::Images;
use crate::render::Names;
use crate::{render, timelines};

/// Messages fetched when a channel opens: enough to fill the view, small so
/// the channel appears fast. Also what a channel keeps once we leave it.
const FIRST_PAGE: u8 = 40;
/// Messages per scrollback request (Discord's maximum): these load in the
/// background, so bigger pages just mean fewer round trips.
const OLDER_PAGE: u8 = 100;
/// Start fetching older messages this many screens before reaching the top,
/// so they are usually in place before the user gets there.
const PREFETCH_SCREENS: f64 = 3.0;
/// Consecutive messages by one author within this window share a header.
const GROUP_WINDOW_SECS: i64 = 7 * 60;

impl App {
    /// The chat column: header, message list (with scrollback) and composer.
    pub(super) fn build_chat(self: &Rc<Self>) -> gtk::Widget {
        let chat =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).css_classes(["chat"]).build();
        chat.append(&self.header);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_activatable(false);
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 16)));
        });
        factory.connect_bind({
            let this = Rc::downgrade(self);
            move |_, item| {
                let Some(this) = this.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                let row = item.child().and_downcast::<gtk::Box>().expect("row box");
                let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
                let message = message.borrow::<Message>().clone();
                let messages = this.messages();
                let previous = item.position().checked_sub(1).and_then(|p| timelines::message_at(&messages, p));
                let names = this.message_names(&message);
                fill_message_row(&row, &message, previous.as_ref(), &names, Some(&this.images));
            }
        });
        self.list.set_factory(Some(&factory));
        self.list.add_css_class("messages");
        self.scroller.set_child(Some(&self.list));
        chat.append(&self.scroller);

        // Scrollback: fetch older messages well before the top is reached.
        self.scroller.vadjustment().connect_value_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.maybe_load_older();
                }
            }
        });

        self.build_composer(&chat);
        chat.upcast()
    }

    pub(super) fn on_message(self: &Rc<Self>, message: Message) {
        self.learn_users(std::iter::once(&message.author).chain(&message.mentions));
        let channel = message.channel_id;
        let viewing = self.state.borrow().channel == Some(channel) && self.window.is_active();
        let notify = {
            let mut state = self.state.borrow_mut();
            state.bump_last_message(channel, message.id);
            let own = state.me() == Some(message.author.id);
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
                state.read.on_message(channel, mentioned);
                place.filter(|p| state.notifications.should_notify(p, mentioned, now_unix()))
            }
        };
        if viewing && self.state.borrow().me() != Some(message.author.id) {
            self.queue_ack(channel, message.id);
        }
        if let Some(place) = notify {
            self.send_notification(&message, &place);
        }
        self.refresh_badges();

        // Keep every cached timeline current, not only the open one.
        let Some(store) = self.timelines.borrow().get(channel).map(|t| t.store.clone()) else { return };
        // A history fetch may have raced this event and already added it.
        if timelines::find(&store, message.id).is_some() {
            return;
        }
        let open = self.state.borrow().channel == Some(channel);
        let at_bottom = open && self.at_bottom();
        if open {
            self.resolve_mentions(&message.content);
        }
        store.append(&timelines::boxed(message));
        if at_bottom {
            self.scroll_to_end();
        }
    }

    pub(super) fn at_bottom(&self) -> bool {
        let adjustment = self.scroller.vadjustment();
        adjustment.value() + adjustment.page_size() >= adjustment.upper() - 80.0
    }

    pub(super) fn on_message_update(
        &self,
        id: Snowflake,
        channel_id: Snowflake,
        content: Option<String>,
        edited: bool,
    ) {
        // Embed-only edits carry no content; nothing to re-render yet.
        let Some(content) = content else { return };
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, mut message)) = timelines::find(&store, id) {
            message.content = content;
            if edited && message.edited_timestamp.is_none() {
                message.edited_timestamp = Some(String::new());
            }
            store.splice(position, 1, &[timelines::boxed(message)]);
        }
    }

    pub(super) fn on_message_delete(&self, id: Snowflake, channel_id: Snowflake) {
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, _)) = timelines::find(&store, id) {
            store.remove(position);
            // The message after it may now group with the one before.
            if position < store.n_items() {
                store.items_changed(position, 1, 1);
            }
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
        let (title, rest, left) = {
            let mut state = self.state.borrow_mut();
            let left = state.channel.filter(|&c| c != id);
            state.channel = Some(id);
            let title = match state.private_channels.iter().find(|c| c.id == id) {
                Some(dm) if dm.kind == discord::ChannelKind::Dm => format!("@ {}", dm.title()),
                Some(group) => group.title(),
                None => {
                    let name = state
                        .guilds
                        .iter()
                        .flat_map(|g| &g.channels)
                        .find(|c| c.id == id)
                        .and_then(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("# {name}")
                }
            };
            (title, state.rest.clone(), left)
        };
        // The channel we left keeps only its newest messages: live events keep
        // those current, and coming back refetches the newest page anyway.
        if let Some(left) = left
            && let Some(timeline) = self.timelines.borrow_mut().get_mut(left)
        {
            timelines::keep_newest(timeline, FIRST_PAGE as u32);
        }
        self.rebuild_sidebar();
        self.header.set_label(&title);
        self.composer_error.set_visible(false);

        // Show what we have at once (instant for a recently viewed channel),
        // then fetch the newest page and merge it in.
        let store = self.timelines.borrow_mut().open(id).store.clone();
        self.selection.set_model(Some(&store));
        self.scroll_to_end();

        let Some(rest) = rest else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(id, FIRST_PAGE, None).await;
            match result {
                Ok(page) => {
                    this.learn_messages(&page);
                    let full = page.len() == FIRST_PAGE as usize;
                    let open = this.state.borrow().channel == Some(id);
                    let at_bottom = !open || this.at_bottom();
                    let replaced = timelines::merge_newest(&store, page);
                    if let Some(timeline) = this.timelines.borrow_mut().get_mut(id) {
                        // A short page is the whole channel; a replaced store
                        // lost its scrollback and can page again.
                        if !full {
                            timeline.complete = true;
                        } else if replaced {
                            timeline.complete = false;
                        }
                    }
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
                        this.mark_read(id);
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

    /// Learn names from a batch of messages and look up unknown mentions.
    pub(super) fn learn_messages(self: &Rc<Self>, messages: &[Message]) {
        self.learn_users(messages.iter().flat_map(|m| std::iter::once(&m.author).chain(&m.mentions)));
        for message in messages {
            self.resolve_mentions(&message.content);
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
            let result = rest.messages(channel, OLDER_PAGE, Some(before)).await;
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
                    if page.len() < OLDER_PAGE as usize
                        && let Some(timeline) = this.timelines.borrow_mut().get_mut(channel)
                    {
                        timeline.complete = true;
                    }
                    this.learn_messages(&page);
                    timelines::prepend(&store, page);
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
        let state = self.state.borrow();
        let mut users = state.users.clone();
        for user in message.mentions.iter().chain(std::iter::once(&message.author)) {
            users.entry(user.id.0).or_insert_with(|| user.display_name().to_string());
        }
        let channels = state
            .guild
            .and_then(|id| state.guilds.iter().find(|g| g.id == id))
            .map(|guild| {
                guild.channels.iter().filter_map(|c| c.name.as_ref().map(|name| (c.id.0, name.clone()))).collect()
            })
            .unwrap_or_default();
        Names { users, channels }
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
        let n = messages.n_items();
        if n > 0 {
            messages.items_changed(0, n, n);
        }
    }

    pub(super) fn scroll_to_end(&self) {
        let list = self.list.downgrade();
        let messages = self.messages();
        // After layout, so the new rows have sizes.
        glib::idle_add_local_once(move || {
            if let (Some(list), Some(last)) = (list.upgrade(), messages.n_items().checked_sub(1)) {
                list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
            }
        });
    }
}

/// User ids referenced by `<@id>` mentions within a parsed span (recursive, so
/// mentions inside styled/quoted spans count).
pub(super) fn collect_user_mentions(span: &discord::markup::Span) -> Vec<u64> {
    use discord::markup::{Entity, Span};
    match span {
        Span::Entity(Entity::User(id)) => vec![*id],
        Span::Styled(_, children) | Span::Link { children, .. } => {
            children.iter().flat_map(collect_user_mentions).collect()
        }
        _ => Vec::new(),
    }
}

pub(super) fn parse_time(timestamp: &str) -> Option<glib::DateTime> {
    glib::DateTime::from_iso8601(timestamp, None).ok()?.to_local().ok()
}

pub(super) fn format_time(time: &glib::DateTime) -> String {
    let today = glib::DateTime::now_local().ok().map(|now| now.ymd());
    let pattern = if today == Some(time.ymd()) { "Today at %H:%M" } else { "%Y-%m-%d %H:%M" };
    time.format(pattern).map(String::from).unwrap_or_default()
}

pub(super) fn is_grouped(message: &Message, previous: Option<&Message>) -> bool {
    let Some(previous) = previous else { return false };
    if previous.author.id != message.author.id {
        return false;
    }
    match (parse_time(&previous.timestamp), parse_time(&message.timestamp)) {
        (Some(a), Some(b)) => b.difference(&a).as_seconds() < GROUP_WINDOW_SECS,
        _ => false,
    }
}

/// Rows are rebuilt on every bind: simple, and cheap at chat-history sizes.
pub(super) fn fill_message_row(
    row: &gtk::Box,
    message: &Message,
    previous: Option<&Message>,
    names: &Names,
    images: Option<&Images>,
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    let grouped = is_grouped(message, previous);
    row.set_css_classes(if grouped { &["message"] } else { &["message", "first"] });
    let colour = palette_index(&message.author);

    let gutter = gtk::Box::builder().width_request(40).valign(gtk::Align::Start).build();
    if !grouped {
        gutter.append(&avatar_widget(&message.author, colour, 40, images));
    }
    row.append(&gutter);

    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
    if !grouped {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.append(
            &gtk::Label::builder()
                .label(message.author.display_name())
                .css_classes(["author", &format!("author-{colour}")])
                .build(),
        );
        let time = parse_time(&message.timestamp).map(|t| format_time(&t)).unwrap_or_default();
        header.append(&gtk::Label::builder().label(time).css_classes(["time"]).build());
        body.append(&header);
    }
    let edited = message.edited_timestamp.is_some();
    if !message.content.is_empty() {
        body.append(&render::render(&discord::markup::parse(&message.content), names, edited));
    } else if edited {
        body.append(&render::render(&[], names, edited));
    }
    for attachment in &message.attachments {
        match (attachment.is_image(), images) {
            (true, Some(images)) => body.append(&image_attachment(attachment, images)),
            _ => {
                let markup = format!(
                    "<a href=\"{}\">📎 {}</a>",
                    glib::markup_escape_text(&attachment.url),
                    glib::markup_escape_text(&attachment.filename)
                );
                body.append(
                    &gtk::Label::builder()
                        .use_markup(true)
                        .label(markup)
                        .xalign(0.0)
                        .css_classes(["attachment"])
                        .build(),
                );
            }
        }
    }
    row.append(&body);
}

/// An inline image attachment, scaled to fit a reasonable box.
pub(super) fn image_attachment(attachment: &discord::Attachment, images: &Images) -> gtk::Widget {
    const MAX_W: f64 = 400.0;
    const MAX_H: f64 = 300.0;
    let (mut w, mut h) = match (attachment.width, attachment.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (MAX_W, MAX_H),
    };
    let scale = (MAX_W / w).min(MAX_H / h).min(1.0);
    w *= scale;
    h *= scale;
    let animated = attachment.content_type.as_deref() == Some("image/gif")
        || attachment.url.split('?').next().is_some_and(|u| u.ends_with(".gif"));
    let picture = gtk::Picture::builder()
        .width_request(w as i32)
        .height_request(h as i32)
        .halign(gtk::Align::Start)
        .content_fit(gtk::ContentFit::Contain)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["attachment-image"])
        .build();
    images.set(&picture, &attachment.url, animated);
    picture.upcast()
}
