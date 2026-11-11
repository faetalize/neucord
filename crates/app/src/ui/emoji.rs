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

use super::App;
use super::menus::{self, Entry, Item as MenuItem, Kind};
use super::picker::{self, Row, Spec, Target};
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

/// Only in Developer Mode, like the rest of the picker's menu (233503).
fn developer(app: &App) -> bool {
    app.account.state.borrow().developer_mode
}

/// The emoji a menu was opened on.
fn item(target: &Target) -> Option<&Item> {
    match target {
        Target::Emoji(item) => Some(item),
        Target::Sticker(_) | Target::Gif(_) => None,
    }
}

pub(super) const COPY_EMOJI: Entry<Target> = Entry::Item(MenuItem {
    name: "copy-emoji",
    label: "Copy _Emoji",
    shows: |app, target| developer(app) && matches!(item(target), Some(Item::Unicode(_))),
    kind: Kind::Action(|app, target| {
        if let Some(item) = item(target) {
            app.copy(&item.text());
        }
    }),
});

/// Shift copies what a message writes for it, in place of the id.
pub(super) const COPY_EMOJI_ID: Entry<Target> = Entry::Item(MenuItem {
    name: "copy-emoji-id",
    label: "Copy Emoji _ID",
    shows: |app, target| developer(app) && matches!(item(target), Some(Item::Custom(_))),
    kind: Kind::Action(|app, target| {
        if let Some(Item::Custom(named)) = item(target) {
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
    shows: |app, target| developer(app) && matches!(item(target), Some(Item::Custom(_))),
    kind: Kind::Action(|app, target| {
        if let Some(Item::Custom(named)) = item(target) {
            app.copy(&discord::cdn::emoji_url(named.emoji.id.0, named.emoji.animated, 32));
        }
    }),
});

impl App {
    /// The emoji `channel` offers: every server's, the channel's own first
    /// and then the rail's order, each checked against what we may do there.
    pub(super) fn emoji_library(&self, channel: Snowflake) -> Library {
        let state = self.account.state.borrow();
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
        let guilds = state.guilds_in_picker_order(own);
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
        let spec = Spec {
            placeholder: "Search emoji",
            columns: COLUMNS,
            cell_width: 40,
            rows: Rc::new({
                let library = library.clone();
                move |query| rows(&library, query)
            }),
            cell: Rc::new(cell_button),
            best: Rc::new(move |query| library.search(query, 1).unlocked.first().cloned()),
        };
        picker::open(self, parent, spec, picked);
    }
}

/// What the picker lists for `query`: each server's emoji and the Unicode
/// groups, or what a search finds.
fn rows(library: &Library, query: &str) -> Vec<Row<Item>> {
    let mut rows = Vec::new();
    if query.is_empty() {
        for server in library.picker() {
            Row::section(&mut rows, COLUMNS, server.name, server.emoji.into_iter().map(Item::Custom).collect());
        }
        for category in Category::ALL {
            let unicode = discord::emoji::unicode_in(category).map(Item::Unicode).collect();
            Row::section(&mut rows, COLUMNS, category.label().to_owned(), unicode);
        }
    } else {
        let found = library.search(query, SEARCH);
        Row::section(&mut rows, COLUMNS, "Search Results".to_owned(), found.unlocked);
        Row::section(&mut rows, COLUMNS, "Needs Nitro".to_owned(), found.locked);
    }
    rows
}

/// An emoji of the picker: a button that picks it, if we may send it, and
/// opens the picker's menu on a right click.
fn cell_button(app: &Rc<App>, item: &Item, picked: &Rc<dyn Fn(&Item)>) -> gtk::Widget {
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
            menus::popup(&app, menus::EXPRESSION_PICKER, Target::Emoji(item.clone()), &button, menus::At::Point(x, y));
        }
    });
    button.upcast()
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
