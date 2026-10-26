//! The server rail: Home, servers and folders, with unread pills and
//! mention counts.

use std::fmt::Write as _;
use std::rc::Rc;

use discord::settings::RailEntry;
use discord::{Guild, Snowflake};
use gtk::prelude::*;

use super::App;
use super::state::State;
use super::widgets::{badge, initials};
use crate::images::Images;

impl App {
    pub(super) fn rebuild_rail(self: &Rc<Self>) {
        while let Some(child) = self.rail.first_child() {
            self.rail.remove(&child);
        }
        let state = self.state.borrow();
        self.drawn.borrow_mut().0 = state.rail_signature();

        let home =
            gtk::Button::builder().label("DM").tooltip_text("Direct Messages").css_classes(["guild", "home"]).build();
        if state.guild.is_none() {
            home.add_css_class("active");
        }
        home.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.select_home();
                }
            }
        });
        self.rail.append(&rail_item(&home, false, state.home_badge()));
        self.rail.append(&gtk::Separator::builder().css_classes(["rail-separator"]).build());

        let mut folder_css = String::new();
        let guild = |id: Snowflake| state.guilds.iter().find(|g| g.id == id);
        for entry in state.rail_layout() {
            match entry {
                RailEntry::Guild(id) => {
                    if let Some(guild) = guild(id) {
                        self.rail.append(&self.guild_item(&state, guild));
                    }
                }
                RailEntry::Folder { id, name, color, guilds } => {
                    let members: Vec<&Guild> = guilds.iter().filter_map(|g| guild(*g)).collect();
                    let class = format!("folder-{id}");
                    if let Some(color) = color {
                        let (r, g, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
                        let _ = writeln!(
                            folder_css,
                            ".folder.{class}:not(.expanded) {{ background: rgba({r}, {g}, {b}, 0.35); }}\n\
                             .folder.{class} image {{ color: rgb({r}, {g}, {b}); }}"
                        );
                    }
                    let expanded = state.open_folders.contains(&id);
                    let button = gtk::Button::builder()
                        .tooltip_text(name.as_deref().unwrap_or("Folder"))
                        .css_classes(["guild", "folder", &class])
                        .overflow(gtk::Overflow::Hidden)
                        .build();
                    if expanded {
                        button.add_css_class("expanded");
                        button.set_child(Some(&gtk::Image::from_icon_name("folder-symbolic")));
                    } else {
                        button.set_child(Some(&folder_preview(&members, &self.images)));
                        if members.iter().any(|g| state.guild == Some(g.id)) {
                            button.add_css_class("active");
                        }
                    }
                    button.connect_clicked({
                        let this = Rc::downgrade(self);
                        move |_| {
                            if let Some(this) = this.upgrade() {
                                this.toggle_folder(id);
                            }
                        }
                    });
                    if expanded {
                        let group = gtk::Box::builder()
                            .orientation(gtk::Orientation::Vertical)
                            .css_classes(["folder-open"])
                            .build();
                        group.append(&rail_item(&button, false, 0));
                        for member in &members {
                            group.append(&self.guild_item(&state, member));
                        }
                        self.rail.append(&group);
                    } else {
                        // A collapsed folder carries its servers' markers.
                        let (unread, mentions) = members.iter().fold((false, 0), |(unread, mentions), g| {
                            let (u, m) = state.guild_badge(g);
                            (unread || u, mentions + m)
                        });
                        self.rail.append(&rail_item(&button, unread, mentions));
                    }
                }
            }
        }
        self.folder_css.load_from_string(&folder_css);
    }

    /// One server's rail button with its unread pill and mention count.
    pub(super) fn guild_item(self: &Rc<Self>, state: &State, guild: &Guild) -> gtk::Widget {
        let button = gtk::Button::builder()
            .tooltip_text(&guild.name)
            .css_classes(["guild"])
            // Clip the icon to the button's rounded corners.
            .overflow(gtk::Overflow::Hidden)
            .build();
        match guild.icon.as_deref() {
            Some(hash) => {
                let picture = gtk::Picture::builder()
                    .width_request(48)
                    .height_request(48)
                    .content_fit(gtk::ContentFit::Cover)
                    .overflow(gtk::Overflow::Hidden)
                    .build();
                self.images.set(
                    &picture,
                    &discord::cdn::guild_icon_url(guild.id.0, hash, 64),
                    discord::cdn::is_animated(hash),
                );
                button.set_child(Some(&picture));
            }
            None => button.set_label(&initials(&guild.name)),
        }
        if state.guild == Some(guild.id) {
            button.add_css_class("active");
        }
        let id = guild.id;
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.select_guild(id);
            }
        });
        let (unread, mentions) = state.guild_badge(guild);
        rail_item(&button, unread, mentions)
    }

    pub(super) fn toggle_folder(self: &Rc<Self>, id: u64) {
        {
            let mut state = self.state.borrow_mut();
            if !state.open_folders.remove(&id) {
                state.open_folders.insert(id);
            }
        }
        self.rebuild_rail();
    }

    pub(super) fn select_guild(self: &Rc<Self>, id: Snowflake) {
        let first_channel = {
            let mut state = self.state.borrow_mut();
            state.guild = Some(id);
            state
                .guilds
                .iter()
                .find(|g| g.id == id)
                .and_then(|g| g.sidebar().first().and_then(|(_, chans)| chans.first().map(|c| c.id)))
        };
        self.rebuild_rail();
        match first_channel {
            Some(channel) => self.select_channel(channel),
            None => {
                {
                    let mut state = self.state.borrow_mut();
                    state.channel = None;
                    state.page = None;
                }
                self.content.set_visible_child_name("chat");
                self.update_request_bar();
                self.show_blank();
                self.header.set_label("No text channels you can see here");
                self.rebuild_sidebar();
            }
        }
    }

    /// Open Home: the most recent DM, or the friends page if there are none.
    pub(super) fn select_home(self: &Rc<Self>) {
        let latest = {
            let mut state = self.state.borrow_mut();
            state.guild = None;
            state.dms().max_by_key(|c| c.activity()).map(|c| c.id)
        };
        match latest {
            Some(channel) => {
                self.rebuild_rail();
                self.select_channel(channel);
            }
            None => self.show_friends(),
        }
    }
}

/// A rail button with Discord's markers: a pill at the left edge for unread
/// messages, a mention count at the bottom right.
pub(super) fn rail_item(button: &gtk::Button, unread: bool, mentions: u32) -> gtk::Widget {
    let overlay = gtk::Overlay::builder().child(button).build();
    if unread {
        let pill =
            gtk::Box::builder().halign(gtk::Align::Start).valign(gtk::Align::Center).css_classes(["rail-pill"]).build();
        pill.set_can_target(false);
        overlay.add_overlay(&pill);
    }
    if let Some(badge) = badge(mentions) {
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        badge.set_margin_end(8);
        badge.set_can_target(false);
        overlay.add_overlay(&badge);
    }
    overlay.upcast()
}

/// A collapsed folder's face: its first four servers' icons in a 2×2 grid.
pub(super) fn folder_preview(guilds: &[&Guild], images: &Images) -> gtk::Widget {
    const MINI: i32 = 18;
    let grid = gtk::Grid::builder()
        .row_spacing(4)
        .column_spacing(4)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    for (i, guild) in guilds.iter().take(4).enumerate() {
        let mini: gtk::Widget = match guild.icon.as_deref() {
            Some(hash) => {
                let picture = gtk::Picture::builder()
                    .width_request(MINI)
                    .height_request(MINI)
                    .content_fit(gtk::ContentFit::Cover)
                    .overflow(gtk::Overflow::Hidden)
                    .css_classes(["mini-icon"])
                    .build();
                // Static frame only: four animations in one button is noise.
                images.set(&picture, &discord::cdn::guild_icon_still_url(guild.id.0, hash, 32), false);
                picture.upcast()
            }
            None => gtk::Label::builder()
                .label(initials(&guild.name).chars().next().map(String::from).unwrap_or_default())
                .width_request(MINI)
                .height_request(MINI)
                .css_classes(["mini-icon"])
                .build()
                .upcast(),
        };
        grid.attach(&mini, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    grid.upcast()
}
