//! The channel list of the open server, or the DM list on Home.

use std::rc::Rc;

use discord::Channel;
use discord::notifications::Place;
use gtk::pango;
use gtk::prelude::*;

use super::App;
use super::state::{Page, now_unix};
use super::widgets::{PALETTE_SIZE, avatar_widget, badge, initials, palette_index};
use crate::images::Images;

impl App {
    pub(super) fn rebuild_sidebar(self: &Rc<Self>) {
        while let Some(child) = self.channels.first_child() {
            self.channels.remove(&child);
        }
        let state = self.state.borrow();
        self.drawn.borrow_mut().1 = state.sidebar_signature();
        let Some(id) = state.guild else {
            self.guild_name.set_label("Direct Messages");
            self.append_home_entry(
                "Friends",
                state.page == Some(Page::Friends),
                state.relationships.incoming(),
                App::show_friends,
            );
            let requests = state.requests().len();
            let in_requests = state.page == Some(Page::MessageRequests) || state.open_request().is_some();
            if requests > 0 || in_requests {
                self.append_home_entry("Message Requests", in_requests, state.waiting_requests(), App::show_requests);
            }
            let mut dms: Vec<&Channel> = state.dms().collect();
            dms.sort_by_key(|c| std::cmp::Reverse(c.activity()));
            for dm in dms {
                let row = dm_row(dm, &self.images);
                if let Some(badge) = badge(state.read.mentions(dm.id)) {
                    row.append(&badge);
                }
                let button = gtk::Button::builder().child(&row).css_classes(["channel", "dm"]).build();
                if state.channel == Some(dm.id) {
                    button.add_css_class("active");
                }
                if state.is_unread(dm) {
                    button.add_css_class("unread");
                }
                let place = Place { guild: None, guild_default: 0, channel: dm.id, parent: None };
                if state.notifications.channel_muted(&place, now_unix()) {
                    button.add_css_class("muted");
                }
                let id = dm.id;
                let this = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(this) = this.upgrade() {
                        this.select_channel(id);
                    }
                });
                self.channels.append(&button);
            }
            return;
        };
        let Some(guild) = state.guilds.iter().find(|g| g.id == id) else {
            return;
        };
        self.guild_name.set_label(&guild.name);
        for (category, channels) in guild.sidebar() {
            if let Some(category) = category {
                self.channels.append(
                    &gtk::Label::builder()
                        .label(category.to_uppercase())
                        .xalign(0.0)
                        .ellipsize(pango::EllipsizeMode::End)
                        .css_classes(["category"])
                        .build(),
                );
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
                let button = gtk::Button::builder().child(&row).css_classes(["channel"]).build();
                if state.channel == Some(channel.id) {
                    button.add_css_class("active");
                }
                let place = Place {
                    guild: Some(guild.id),
                    guild_default: guild.default_message_notifications,
                    channel: channel.id,
                    parent: channel.parent_id,
                };
                if state.notifications.channel_muted(&place, now_unix()) {
                    button.add_css_class("muted");
                } else if state.is_unread(channel) {
                    button.add_css_class("unread");
                }
                let id = channel.id;
                let this = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(this) = this.upgrade() {
                        this.select_channel(id);
                    }
                });
                self.channels.append(&button);
            }
        }
    }
}

impl App {
    /// One of the entries above the DM list that open a Home page.
    fn append_home_entry(self: &Rc<Self>, label: &str, active: bool, count: u32, open: fn(&Rc<App>)) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build());
        if let Some(badge) = badge(count) {
            row.append(&badge);
        }
        let button = gtk::Button::builder().child(&row).css_classes(["channel", "home-entry"]).build();
        if active {
            button.add_css_class("active");
        }
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                open(&this);
            }
        });
        self.channels.append(&button);
    }
}

/// A DM list entry: the other person's avatar (or the group's icon) and the title.
pub(super) fn dm_row(channel: &Channel, images: &Images) -> gtk::Box {
    const SIZE: i32 = 32;
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon: gtk::Widget = match (channel.kind, channel.recipients.first(), channel.icon.as_deref()) {
        (discord::ChannelKind::GroupDm, _, Some(hash)) => {
            let picture = gtk::Picture::builder()
                .width_request(SIZE)
                .height_request(SIZE)
                .content_fit(gtk::ContentFit::Cover)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["avatar", "small"])
                .build();
            images.set(&picture, &discord::cdn::group_dm_icon_url(channel.id.0, hash, 64), false);
            picture.upcast()
        }
        (discord::ChannelKind::Dm, Some(user), _) => avatar_widget(user, palette_index(user), SIZE, Some(images)),
        _ => gtk::Label::builder()
            .label(initials(&channel.title()))
            .width_request(SIZE)
            .height_request(SIZE)
            .css_classes(["avatar", "small", &format!("avatar-{}", channel.id.0 % PALETTE_SIZE)])
            .build()
            .upcast(),
    };
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);
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
