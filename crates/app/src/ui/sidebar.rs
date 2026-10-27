//! The channel list of the open server, or the DM list on Home.

use std::rc::Rc;

use discord::notifications::Place;
use discord::{Channel, Snowflake};
use gtk::pango;
use gtk::prelude::*;

use super::App;
use super::state::{Page, now_unix};
use super::widgets::{avatar_widget, badge, round_picture};
use crate::images::Images;

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
        for (category, channels) in guild.sidebar() {
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
                row.append(
                    &gtk::Label::builder()
                        .label(format!("# {}", channel.name.as_deref().unwrap_or("unnamed")))
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(pango::EllipsizeMode::End)
                        .build(),
                );
                if let Some(badge) = badge(state.read.mentions(channel.id)) {
                    row.append(&badge);
                }
                let row = self.append_row(&row, Some(Target::Channel(channel.id)), state.channel == Some(channel.id));
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
            }
        }
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
