//! The channel list of the open server, or the DM list on Home.

use std::rc::Rc;

use discord::channel_icon::{self, Glyph, Mark};
use discord::notifications::Place;
use discord::{Channel, Snowflake};
use gtk::prelude::*;
use gtk::{glib, pango};

use super::App;
use super::menus::{self, At, Entry, Item, Kind};
use super::state::{Page, now_ms, now_unix};
use super::thread_line::ThreadLine;
use super::widgets::{avatar_widget, badge, round_picture};
use crate::images::Images;

/// What a channel row's menu acts on.
pub(super) struct ChannelTarget(pub(super) Snowflake);

/// Copy Link, on any channel (886393, in the channel's actions).
pub(super) const COPY_CHANNEL_LINK: Entry<ChannelTarget> = Entry::Item(Item {
    name: "copy-channel-link",
    label: "Copy _Link",
    shows: |_, _| true,
    kind: Kind::Action(|app, target| {
        let link = app.state.borrow().find_channel(target.0).map(|(guild, c)| c.link(guild.map(|g| g.id)));
        if let Some(link) = link {
            app.copy(&link);
        }
    }),
});

/// Open Chat, on a voice or stage channel (428631): the client's other way
/// into the chat, which it shows with Connect. Without View Channel too the
/// row isn't listed, so `can_view` stands for both.
pub(super) const OPEN_CHAT: Entry<ChannelTarget> = Entry::Item(Item {
    name: "open-chat",
    label: "Open _Chat",
    shows: |app, target| {
        let state = app.state.borrow();
        state.find_channel(target.0).is_some_and(|(_, c)| c.kind.is_vocal()) && state.can_view(target.0)
    },
    kind: Kind::Action(|app, target| {
        app.select_channel(target.0);
        app.show_content();
    }),
});

/// Copy Channel ID, only in Developer Mode (50268).
pub(super) const COPY_CHANNEL_ID: Entry<ChannelTarget> = Entry::Item(Item {
    name: "copy-channel-id",
    label: "Copy Channel _ID",
    shows: |app, _| app.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, target| app.copy(&target.0.to_string())),
});

/// What a sidebar row opens.
#[derive(Clone, Copy)]
pub(super) enum Target {
    Channel(Snowflake),
    Friends,
    Requests,
}

impl App {
    pub(super) fn rebuild_sidebar(self: &Rc<Self>) {
        self.channels.remove_all();
        self.targets.borrow_mut().clear();
        let state = self.state.borrow();
        self.drawn.borrow_mut().1 = state.sidebar_signature();
        let Some(id) = state.guild else {
            self.guild_name.set("Direct Messages", None);
            self.append_home_entry(
                "Friends",
                state.page == Some(Page::Friends),
                state.relationships.incoming(),
                Target::Friends,
            );
            let requests = state.requests().len();
            let in_requests = state.page == Some(Page::MessageRequests) || state.open_request().is_some();
            if requests > 0 || in_requests {
                self.append_home_entry("Message Requests", in_requests, state.waiting_requests(), Target::Requests);
            }
            let mut dms: Vec<&Channel> = state.dms().collect();
            dms.sort_by_key(|c| std::cmp::Reverse(c.activity()));
            for dm in dms {
                let row = dm_row(dm, &self.images);
                if let Some(badge) = badge(state.read.mentions(dm.id)) {
                    row.append(&badge);
                }
                let row = self.append_row(&row, Some(Target::Channel(dm.id)), state.channel == Some(dm.id));
                let place = Place { guild: None, guild_default: 0, channel: dm.id, parent: None };
                if state.notifications.channel_muted(&place, now_unix()) {
                    row.add_css_class("muted");
                } else if state.is_unread(dm) {
                    row.add_css_class("unread");
                }
            }
            return;
        };
        let Some(guild) = state.guilds.iter().find(|g| g.id == id) else {
            return;
        };
        self.guild_name.set(&guild.name, None);
        for (category, channels) in state.sidebar(guild) {
            if let Some(category) = category {
                let label = gtk::Label::builder()
                    .label(category)
                    .xalign(0.0)
                    .ellipsize(pango::EllipsizeMode::End)
                    .css_classes(["category", "dimmed"])
                    .build();
                self.append_row(&label, None, false);
            }
            for channel in channels {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let icon = channel_icon::of(guild, channel);
                // A listed channel always has an icon; a server never holds a DM.
                let glyph = glyph_widget(icon.map_or(Glyph::Text, |i| i.glyph));
                row.append(&glyph);
                row.append(
                    &gtk::Label::builder()
                        .label(channel.name.as_deref().unwrap_or("unnamed"))
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(pango::EllipsizeMode::End)
                        .build(),
                );
                if let Some(mark) = icon.and_then(|i| i.mark) {
                    row.append(&mark_widget(mark));
                }
                if let Some(badge) = badge(state.read.mentions(channel.id)) {
                    row.append(&badge);
                }
                let row = self.append_row(&row, Some(Target::Channel(channel.id)), state.channel == Some(channel.id));
                self.channel_menu_on(&row, channel.id);
                let place = Place {
                    guild: Some(guild.id),
                    guild_default: guild.default_message_notifications,
                    channel: channel.id,
                    parent: channel.parent_id,
                };
                if state.notifications.channel_muted(&place, now_unix()) {
                    row.add_css_class("muted");
                } else if state.is_unread(channel) {
                    row.add_css_class("unread");
                }
                let threads = state.sidebar_threads(guild, channel);
                for (i, thread) in threads.iter().enumerate() {
                    self.append_thread(&state, thread, i + 1 == threads.len());
                }
            }
        }
        self.watch_thread_expiry(guild);
    }

    /// Open `channel`'s menu where its row is right-clicked.
    fn channel_menu_on(self: &Rc<Self>, row: &gtk::ListBoxRow, channel: Snowflake) {
        let (app, row_weak) = (Rc::downgrade(self), row.downgrade());
        menus::on_secondary(row, move |x, y| {
            let (Some(app), Some(row)) = (app.upgrade(), row_weak.upgrade()) else { return };
            menus::popup(&app, menus::CHANNEL, ChannelTarget(channel), &row, At::Point(x, y));
        });
    }

    /// A thread's row, under its parent's, joined to it by a line; `last`
    /// if no thread of the same parent follows.
    fn append_thread(self: &Rc<Self>, state: &super::state::State, thread: &Channel, last: bool) {
        let row = gtk::Box::builder().spacing(6).build();
        row.append(&ThreadLine::new(last));
        row.append(
            &gtk::Label::builder()
                .label(thread.name.as_deref().unwrap_or("unnamed"))
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::End)
                .build(),
        );
        if let Some(badge) = badge(state.read.mentions(thread.id)) {
            row.append(&badge);
        }
        self.thread_menu_on(&row, thread.id);
        let row = self.append_row(&row, Some(Target::Channel(thread.id)), state.channel == Some(thread.id));
        if super::state::State::thread_muted(thread) {
            row.add_css_class("muted");
        } else if state.is_unread(thread) {
            row.add_css_class("unread");
        }
    }

    /// Look at the sidebar again when a thread it lists would drop out on
    /// its own, quiet for its whole auto-archive window.
    fn watch_thread_expiry(self: &Rc<Self>, guild: &discord::Guild) {
        if let Some(timer) = self.thread_expiry.take() {
            timer.remove();
        }
        let Some(at) = discord::threads::next_expiry(guild, now_ms()) else { return };
        let delay = std::time::Duration::from_millis((at - now_ms()).max(0) as u64 + 1);
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(delay, move || {
            if let Some(this) = this.upgrade() {
                this.thread_expiry.take();
                this.refresh_badges();
            }
        });
        self.thread_expiry.replace(Some(timer));
    }

    /// Add a row to the channel list. Rows without a target are headings.
    fn append_row(&self, child: &impl IsA<gtk::Widget>, target: Option<Target>, active: bool) -> gtk::ListBoxRow {
        let row =
            gtk::ListBoxRow::builder().child(child).selectable(target.is_some()).activatable(target.is_some()).build();
        self.channels.append(&row);
        self.targets.borrow_mut().push(target);
        if active {
            self.channels.select_row(Some(&row));
        }
        row
    }

    /// One of the entries above the DM list that open a Home page.
    fn append_home_entry(&self, label: &str, active: bool, count: u32, target: Target) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build());
        if let Some(badge) = badge(count) {
            row.append(&badge);
        }
        self.append_row(&row, Some(target), active);
    }

    /// Open what a row stands for, putting the sidebar away on a narrow window.
    pub(super) fn activate_sidebar_row(self: &Rc<Self>, index: i32) {
        let target = usize::try_from(index).ok().and_then(|i| self.targets.borrow().get(i).copied().flatten());
        match target {
            Some(Target::Channel(id)) => self.select_channel(id),
            Some(Target::Friends) => self.show_friends(),
            Some(Target::Requests) => self.show_requests(),
            None => return,
        }
        self.show_content();
    }
}

/// The width of a symbolic icon at the default size.
const GLYPH_WIDTH: i32 = 16;

/// The kind of channel, in front of its name. Text channels show a `#`, which
/// the theme has no icon for. Every glyph is as wide as an icon, so the names
/// line up; a size group would measure every row on each rebuild.
pub(super) fn glyph_widget(glyph: Glyph) -> gtk::Widget {
    let (icon, tooltip) = match glyph {
        Glyph::Text => {
            return gtk::Label::builder()
                .label("#")
                .halign(gtk::Align::Center)
                .width_request(GLYPH_WIDTH)
                .build()
                .upcast();
        }
        Glyph::Announcement => ("application-rss+xml-symbolic", "Announcement channel"),
        Glyph::Voice => ("audio-volume-high-symbolic", "Voice channel"),
        Glyph::Stage => ("microphone-sensitivity-high-symbolic", "Stage channel"),
        Glyph::Forum => ("view-list-symbolic", "Forum channel"),
        Glyph::Media => ("image-x-generic-symbolic", "Media channel"),
        Glyph::Rules => ("text-x-generic-symbolic", "Rules channel"),
    };
    gtk::Image::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .halign(gtk::Align::Center)
        .width_request(GLYPH_WIDTH)
        .build()
        .upcast()
}

/// After a channel's name, for what sets it apart.
fn mark_widget(mark: Mark) -> gtk::Image {
    let (icon, tooltip) = match mark {
        Mark::Nsfw => ("dialog-warning-symbolic", "Age-restricted"),
        Mark::Locked => ("channel-secure-symbolic", "Limited"),
    };
    gtk::Image::builder().icon_name(icon).tooltip_text(tooltip).build()
}

/// A DM list entry: the other person's avatar (or the group's icon) and the title.
pub(super) fn dm_row(channel: &Channel, images: &Images) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.append(&dm_icon(channel, images));
    row.append(
        &gtk::Label::builder()
            .label(channel.title())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build(),
    );
    row
}

/// The other person's avatar, or the group's icon.
pub(super) fn dm_icon(channel: &Channel, images: &Images) -> gtk::Widget {
    const SIZE: i32 = 32;
    let icon: gtk::Widget = match (channel.kind, channel.recipients.first(), channel.icon.as_deref()) {
        (discord::ChannelKind::GroupDm, _, Some(hash)) => {
            let picture = round_picture(SIZE);
            images.set(&picture, &discord::cdn::group_dm_icon_url(channel.id.0, hash, 64), false);
            picture.upcast()
        }
        (discord::ChannelKind::Dm, Some(user), _) => avatar_widget(user, SIZE, Some(images)),
        _ => adw::Avatar::new(SIZE, Some(&channel.title()), true).upcast(),
    };
    icon.set_valign(gtk::Align::Center);
    icon
}
