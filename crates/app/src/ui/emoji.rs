//! Emoji in the window: the library of the open channel, the picker, and
//! the pieces `:name:` autocomplete draws. What they offer and what a pick
//! writes is `discord::emoji`; the picker takes a callback for the pick, so
//! anything that wants an emoji (the message box, adding a reaction) opens
//! it the same way.

use std::rc::Rc;

use adw::prelude::*;
use discord::Snowflake;
use discord::emoji::{Category, Item, Library, Place};
use discord::permissions::{self, Permissions};
use discord::settings::RailEntry;
use gtk::{gio, glib};

use super::App;
use super::menus::{self, Entry, Item as MenuItem, Kind};
use super::state::now_unix;
use crate::images::Images;

/// Emoji to a row of the picker.
const COLUMNS: usize = 9;
/// The size emoji are shown at in the picker and in autocomplete.
const SIZE: i32 = 24;
/// The most emoji a search in the picker lists.
const SEARCH: usize = 50;
/// What autocomplete lists: the client asks for 50 where it has no stickers
/// or sounds to give room to.
pub(super) const AUTOCOMPLETE: usize = 50;

/// What the picker's menu was opened on.
pub(super) struct Target {
    item: Item,
}

/// Only in Developer Mode, like the rest of the picker's menu (233503).
fn developer(app: &App, _: &Target) -> bool {
    app.state.borrow().developer_mode
}

pub(super) const COPY_EMOJI: Entry<Target> = Entry::Item(MenuItem {
    name: "copy-emoji",
    label: "Copy _Emoji",
    shows: |app, target| developer(app, target) && matches!(target.item, Item::Unicode(_)),
    kind: Kind::Action(|app, target| app.copy(&target.item.text())),
});

/// Shift copies what a message writes for it, in place of the id.
pub(super) const COPY_EMOJI_ID: Entry<Target> = Entry::Item(MenuItem {
    name: "copy-emoji-id",
    label: "Copy Emoji _ID",
    shows: |app, target| developer(app, target) && matches!(target.item, Item::Custom(_)),
    kind: Kind::Action(|app, target| {
        if let Item::Custom(named) = &target.item {
            app.copy(&match app.shift_held() {
                true => named.emoji.markup(),
                false => named.emoji.id.to_string(),
            });
        }
    }),
});

pub(super) const COPY_EMOJI_LINK: Entry<Target> = Entry::Item(MenuItem {
    name: "copy-emoji-link",
    label: "Copy Emoji Image _Link",
    shows: |app, target| developer(app, target) && matches!(target.item, Item::Custom(_)),
    kind: Kind::Action(|app, target| {
        if let Item::Custom(named) = &target.item {
            app.copy(&discord::cdn::emoji_url(named.emoji.id.0, named.emoji.animated, 32));
        }
    }),
});

/// One line of the picker's list.
enum Row {
    Header(String),
    Cells(Vec<Item>),
}

impl App {
    /// The emoji `channel` offers: every server's, the channel's own first
    /// and then the rail's order, each checked against what we may do there.
    pub(super) fn emoji_library(&self, channel: Snowflake) -> Library {
        let state = self.state.borrow();
        let found = state.find_channel(channel);
        let own = found.and_then(|(guild, _)| guild).map(|g| g.id);
        // Server channels need Use External Emojis; a DM needs nothing.
        let external = match (found, &state.account) {
            (Some((None, _)), _) => true,
            (Some((Some(guild), channel)), Some(me)) => {
                permissions::in_channel(guild, channel, me, now_unix()).contains(Permissions::USE_EXTERNAL_EMOJIS)
            }
            _ => false,
        };
        let rail = state.rail_layout().into_iter().flat_map(|entry| match entry {
            RailEntry::Guild(id) => vec![id],
            RailEntry::Folder { guilds, .. } => guilds,
        });
        let order: Vec<Snowflake> = own.into_iter().chain(rail.filter(|id| Some(*id) != own)).collect();
        let guilds = order.iter().filter_map(|id| state.guilds.iter().find(|g| g.id == *id));
        Library::new(guilds, &state.limits(), &Place { guild: own, external })
    }

    /// Open the picker over `parent`, for emoji to write in `channel`.
    /// `picked` hears each emoji chosen; Shift keeps the picker open for
    /// more, as in the client.
    pub(super) fn open_emoji_picker(
        self: &Rc<Self>,
        channel: Snowflake,
        parent: &impl IsA<gtk::Widget>,
        picked: impl Fn(&Rc<App>, &Item) + 'static,
    ) {
        let library = Rc::new(self.emoji_library(channel));
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let search = gtk::SearchEntry::builder().placeholder_text("Search emoji").build();
        let popover = gtk::Popover::new();
        let picked: Rc<dyn Fn(&Item)> = Rc::new({
            let (this, popover) = (Rc::downgrade(self), popover.downgrade());
            move |item| {
                let (Some(this), Some(popover)) = (this.upgrade(), popover.upgrade()) else { return };
                picked(&this, item);
                if !this.shift_held() {
                    popover.popdown();
                }
            }
        });

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_activatable(false);
            item.set_selectable(false);
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        });
        factory.connect_bind({
            let (this, picked) = (Rc::downgrade(self), picked.clone());
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                let (Some(this), Some(row), Some(line)) = (
                    this.upgrade(),
                    item.item().and_downcast::<glib::BoxedAnyObject>(),
                    item.child().and_downcast::<gtk::Box>(),
                ) else {
                    return;
                };
                match &*row.borrow::<Row>() {
                    Row::Header(title) => {
                        let label = gtk::Label::builder()
                            .label(title)
                            .xalign(0.0)
                            .margin_top(6)
                            .css_classes(["heading"])
                            .build();
                        line.append(&label);
                    }
                    Row::Cells(cells) => {
                        for cell in cells {
                            line.append(&cell_button(&this, cell, &picked));
                        }
                    }
                }
            }
        });
        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(line) = item.child().and_downcast::<gtk::Box>() {
                while let Some(child) = line.first_child() {
                    line.remove(&child);
                }
            }
        });
        let list = gtk::ListView::new(Some(gtk::NoSelection::new(Some(store.clone()))), Some(factory));

        let show = {
            let (store, library) = (store.clone(), library.clone());
            move |query: &str| {
                let rows: Vec<glib::BoxedAnyObject> =
                    rows(&library, query).into_iter().map(glib::BoxedAnyObject::new).collect();
                store.splice(0, store.n_items(), &rows);
            }
        };
        show("");
        search.connect_search_changed(move |search| show(search.text().trim()));
        // Enter takes the best match, the one first in the list.
        search.connect_activate({
            let (library, picked) = (library.clone(), picked.clone());
            move |search| {
                if let Some(item) = library.search(search.text().trim(), 1).unlocked.first() {
                    picked(item);
                }
            }
        });

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(COLUMNS as i32 * 40)
            .min_content_height(360)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&search);
        content.append(&scroller);
        popover.set_child(Some(&content));
        popover.set_parent(parent);
        popover.connect_closed(|popover| {
            // Once the pick has been handled, which may come after closing.
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        search.grab_focus();
    }
}

/// What the picker lists for `query`: each server's emoji and the Unicode
/// groups, or what a search finds.
fn rows(library: &Library, query: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut section = |title: String, items: Vec<Item>| {
        if !items.is_empty() {
            rows.push(Row::Header(title));
            rows.extend(items.chunks(COLUMNS).map(|cells| Row::Cells(cells.to_vec())));
        }
    };
    if query.is_empty() {
        for server in library.picker() {
            section(server.name, server.emoji.into_iter().map(Item::Custom).collect());
        }
        for category in Category::ALL {
            section(category.label().to_owned(), discord::emoji::unicode_in(category).map(Item::Unicode).collect());
        }
    } else {
        let found = library.search(query, SEARCH);
        section("Search Results".to_owned(), found.unlocked);
        section("Needs Nitro".to_owned(), found.locked);
    }
    rows
}

/// An emoji of the picker: a button that picks it, if we may send it, and
/// opens the picker's menu on a right click.
fn cell_button(app: &Rc<App>, item: &Item, picked: &Rc<dyn Fn(&Item)>) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(&glyph(item, &app.images))
        .css_classes(["flat"])
        .tooltip_text(format!(":{}:", item.name()))
        .build();
    if item.unavailable().is_some() {
        // A locked emoji sends nothing, and the client offers Nitro on a
        // click, which Neucord doesn't copy.
        button.set_opacity(0.4);
    } else {
        let (item, picked) = (item.clone(), picked.clone());
        button.connect_clicked(move |_| picked(&item));
    }
    menus::on_secondary(&button, {
        let (app, item, button) = (Rc::downgrade(app), item.clone(), button.downgrade());
        move |x, y| {
            let (Some(app), Some(button)) = (app.upgrade(), button.upgrade()) else { return };
            menus::popup(
                &app,
                menus::EXPRESSION_PICKER,
                Target { item: item.clone() },
                &button,
                menus::At::Point(x, y),
            );
        }
    });
    button
}

/// The emoji as a widget of [`SIZE`]: the character, or the image.
pub(super) fn glyph(item: &Item, images: &Images) -> gtk::Widget {
    match item {
        Item::Unicode(e) => gtk::Label::builder().label(e.glyph).css_classes(["title-3"]).build().upcast(),
        Item::Custom(named) => {
            let picture = gtk::Picture::builder()
                .width_request(SIZE)
                .height_request(SIZE)
                .content_fit(gtk::ContentFit::Contain)
                .build();
            let size = ((SIZE * images.scale()) as u32).next_power_of_two().clamp(16, 4096) as u16;
            // A still, even for an animated emoji: a screenful of them
            // moving would be a lot of drawing.
            images.set(&picture, &discord::cdn::emoji_url(named.emoji.id.0, false, size), false);
            picture.upcast()
        }
    }
}
