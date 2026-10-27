//! The server rail: Home, servers and folders, with unread pills and
//! mention counts.

use std::fmt::Write as _;
use std::rc::Rc;

use discord::settings::RailEntry;
use discord::{Guild, Snowflake};
use gtk::prelude::*;

use super::App;
use super::widgets::{initials, set_badge};
use crate::images::Images;

/// What a rail button stands for, to work out its markers.
pub(super) enum Mark {
    Home,
    Guild(Snowflake),
    /// A collapsed folder, carrying its servers' markers.
    Folder(Vec<Snowflake>),
}

/// A rail button and its markers, kept so they can change in place: a
/// rebuilt button would show its new state without animating to it.
pub(super) struct RailItem {
    mark: Mark,
    button: gtk::Button,
    pill: gtk::Box,
    badge: gtk::Label,
}

impl App {
    /// Bring the rail up to date: rebuild it if servers, folders or their
    /// looks changed, else only move its markers.
    pub(super) fn rebuild_rail(self: &Rc<Self>) {
        let shape = self.state.borrow().rail_shape();
        if *self.rail_shape.borrow() != shape {
            self.build_rail();
            *self.rail_shape.borrow_mut() = shape;
        }
        self.mark_rail();
    }

    fn build_rail(self: &Rc<Self>) {
        for column in [&self.home, &self.rail] {
            while let Some(child) = column.first_child() {
                column.remove(&child);
            }
        }
        let mut items = Vec::new();
        let state = self.state.borrow();

        let home = gtk::Button::builder()
            .icon_name("user-home-symbolic")
            .tooltip_text("Direct Messages")
            .css_classes(["guild", "home"])
            .build();
        home.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.select_home();
                }
            }
        });
        self.home.append(&rail_item(&home, Mark::Home, &mut items));
        self.home.append(&gtk::Separator::builder().css_classes(["rail-separator"]).build());

        let mut folder_css = String::new();
        let guild = |id: Snowflake| state.guilds.iter().find(|g| g.id == id);
        for entry in state.rail_layout() {
            match entry {
                RailEntry::Guild(id) => {
                    if let Some(guild) = guild(id) {
                        self.rail.append(&self.guild_item(guild, &mut items));
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
                    button.connect_clicked({
                        let this = Rc::downgrade(self);
                        move |_| {
                            if let Some(this) = this.upgrade() {
                                this.toggle_folder(id);
                            }
                        }
                    });
                    if expanded {
                        button.add_css_class("expanded");
                        button.set_child(Some(&gtk::Image::from_icon_name("folder-symbolic")));
                        let group = gtk::Box::builder()
                            .orientation(gtk::Orientation::Vertical)
                            .css_classes(["folder-open"])
                            .build();
                        group.append(&rail_item(&button, Mark::Folder(Vec::new()), &mut items));
                        for member in &members {
                            group.append(&self.guild_item(member, &mut items));
                        }
                        self.rail.append(&group);
                    } else {
                        button.set_child(Some(&folder_preview(&members, &self.images)));
                        let ids = members.iter().map(|g| g.id).collect();
                        self.rail.append(&rail_item(&button, Mark::Folder(ids), &mut items));
                    }
                }
            }
        }
        self.folder_css.load_from_string(&folder_css);
        *self.rail_items.borrow_mut() = items;
    }

    /// Set every rail button's markers from state: open, unread, mentions.
    fn mark_rail(&self) {
        let state = self.state.borrow();
        self.drawn.borrow_mut().0 = state.rail_signature();
        let badge = |id: &Snowflake| state.guilds.iter().find(|g| g.id == *id).map(|g| state.guild_badge(g));
        for item in self.rail_items.borrow().iter() {
            let (active, unread, mentions) = match &item.mark {
                Mark::Home => (state.guild.is_none(), false, state.home_badge()),
                Mark::Guild(id) => {
                    let (unread, mentions) = badge(id).unwrap_or_default();
                    (state.guild == Some(*id), unread, mentions)
                }
                Mark::Folder(ids) => ids.iter().fold((false, false, 0), |(active, unread, mentions), id| {
                    let (u, m) = badge(id).unwrap_or_default();
                    (active || state.guild == Some(*id), unread || u, mentions + m)
                }),
            };
            for widget in [item.button.upcast_ref::<gtk::Widget>(), item.pill.upcast_ref()] {
                if active {
                    widget.add_css_class("active");
                } else {
                    widget.remove_css_class("active");
                }
            }
            if unread {
                item.pill.add_css_class("unread");
            } else {
                item.pill.remove_css_class("unread");
            }
            set_badge(&item.badge, mentions);
        }
    }

    /// One server's rail button.
    fn guild_item(self: &Rc<Self>, guild: &Guild, items: &mut Vec<RailItem>) -> gtk::Widget {
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
        let id = guild.id;
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.select_guild(id);
            }
        });
        rail_item(&button, Mark::Guild(id), items)
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
                self.update_notice_bar();
                self.show_blank();
                self.header.set("No text channels you can see here", None);
                self.set_content_title("Neucord");
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

/// A rail button with its markers: a pill at the left edge, which grows from
/// unread to hovered to open, and a mention count at the bottom right.
fn rail_item(button: &gtk::Button, mark: Mark, items: &mut Vec<RailItem>) -> gtk::Widget {
    let overlay = gtk::Overlay::builder().child(button).css_classes(["rail-item"]).build();
    let pill =
        gtk::Box::builder().halign(gtk::Align::Start).valign(gtk::Align::Center).css_classes(["rail-pill"]).build();
    pill.set_can_target(false);
    overlay.add_overlay(&pill);
    let badge = gtk::Label::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .margin_end(8)
        .visible(false)
        .css_classes(["badge"])
        .build();
    badge.set_can_target(false);
    overlay.add_overlay(&badge);
    items.push(RailItem { mark, button: button.clone(), pill, badge });
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
