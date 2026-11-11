//! Emoji in the window: the library of the open channel, the picker, and
//! the pieces `:name:` autocomplete draws. What they offer and what a pick
//! writes is `discord::emoji`; the picker takes a callback for the pick, so
//! anything that wants an emoji (the message box, adding a reaction) opens
//! it the same way.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use discord::emoji::{Category, Intention, Item, Library, Place, Scores, Tone};
use discord::permissions::{self, Permissions};
use discord::{Setting, Snowflake};

use super::App;
use super::menus::{self, Entry, Item as MenuItem, Kind};
use super::picker::{self, Row, Spec, Target};
use super::state::{now_ms, now_unix};
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
    app.state.borrow().settings.developer_mode
}

/// The emoji a menu was opened on.
fn item(target: &Target) -> Option<&Item> {
    match target {
        Target::Emoji(item) => Some(item),
        Target::Sticker(_) | Target::Gif(_) => None,
    }
}

/// Favorite and Unfavorite lead the emoji's menu, whether or not Developer
/// Mode is on (253925).
pub(super) const FAVORITE: Entry<Target> = Entry::Item(MenuItem {
    name: "favorite-emoji",
    label: "_Favorite",
    shows: |app, target| item(target).is_some_and(|item| !app.is_favorite_emoji(item)),
    kind: Kind::Action(|app, target| {
        if let Some(item) = item(target) {
            app.favorite_emoji(item, true);
        }
    }),
});

pub(super) const UNFAVORITE: Entry<Target> = Entry::Item(MenuItem {
    name: "unfavorite-emoji",
    label: "_Unfavorite",
    shows: |app, target| item(target).is_some_and(|item| app.is_favorite_emoji(item)),
    kind: Kind::Action(|app, target| {
        if let Some(item) = item(target) {
            app.favorite_emoji(item, false);
        }
    }),
});

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
        intention: Intention,
        picked: impl Fn(&Rc<App>, &Item) + 'static,
    ) {
        let library = self.emoji_library(channel);
        self.open_library_picker(library, parent, intention, picked);
    }

    /// The picker over `library`, wherever its emoji are for.
    pub(super) fn open_library_picker(
        self: &Rc<Self>,
        library: Library,
        parent: &impl IsA<gtk::Widget>,
        intention: Intention,
        picked: impl Fn(&Rc<App>, &Item) + 'static,
    ) {
        let library = Rc::new(library);
        let tone = {
            let app = Rc::downgrade(self);
            move || app.upgrade().and_then(|app| app.state.borrow().settings.skin_tone)
        };
        let spec = Spec {
            placeholder: "Search emoji",
            columns: COLUMNS,
            cell_width: 40,
            rows: Rc::new({
                let (library, tone, app) = (library.clone(), tone.clone(), Rc::downgrade(self));
                move |query| {
                    let app = app.upgrade();
                    let shortcuts =
                        app.as_ref().map(|app| app.emoji_shortcuts(&library, intention)).unwrap_or_default();
                    let scores = app.as_ref().map(|app| app.emoji_scores(intention)).unwrap_or_default();
                    rows(&library, query, tone(), shortcuts, &scores)
                }
            }),
            cell: Rc::new(cell_button),
            best: Rc::new({
                let app = Rc::downgrade(self);
                move |query| {
                    let scores = app.upgrade().map(|app| app.emoji_scores(intention)).unwrap_or_default();
                    library.search(query, 1, &scores).unlocked.first().cloned().map(|item| toned(item, tone()))
                }
            }),
        };
        // The skin tone button lists the emoji again once a tone is chosen.
        let handle: Rc<RefCell<Option<picker::Handle>>> = Rc::default();
        let hooks = picker::Hooks {
            trailing: Some(self.tone_button({
                let handle = handle.clone();
                move || {
                    if let Some(handle) = handle.borrow().as_ref() {
                        handle.reload();
                    }
                }
            })),
            ..Default::default()
        };
        *handle.borrow_mut() = Some(picker::open_with(self, parent, spec, hooks, picked));
        // The favourites come from Discord the first time, and fill in then.
        self.load_frecency({
            let handle = handle.clone();
            move || {
                if let Some(handle) = handle.borrow().as_ref() {
                    handle.reload();
                }
            }
        });
    }

    /// The emoji the picker lists first: the favourites, then those used
    /// most (reactions for a reaction picker), as far as `library` has them.
    fn emoji_shortcuts(&self, library: &Library, intention: Intention) -> Shortcuts {
        let mut state = self.state.borrow_mut();
        let favorites = state.frecency.favorite_emojis.iter().filter_map(|key| library.item(key)).collect();
        let stores = &mut state.frecency.stores;
        let history = match intention {
            Intention::Chat => &mut stores.emoji.history,
            Intention::Reaction => &mut stores.reactions.history,
        };
        let frequent = history.frequently(now_ms() as u64, |key| library.item(key).is_some());
        Shortcuts { favorites, frequent: frequent.iter().filter_map(|key| library.item(key)).collect() }
    }

    /// How much each emoji was used, for a search to rank by.
    pub(super) fn emoji_scores(&self, intention: Intention) -> Scores {
        let mut state = self.state.borrow_mut();
        let stores = &mut state.frecency.stores;
        match intention {
            Intention::Chat => stores.emoji.history.scores(now_ms() as u64),
            Intention::Reaction => stores.reactions.history.scores(now_ms() as u64),
        }
    }

    /// The skin tone button of the picker: the tone emoji take by default,
    /// chosen from a list, saved as the client saves it. `chosen` runs once
    /// the choice is made.
    fn tone_button(self: &Rc<Self>, chosen: impl Fn() + Clone + 'static) -> gtk::Widget {
        let current = self.state.borrow().settings.skin_tone;
        let sample = |tone: Option<Tone>| discord::emoji::unicode("wave").map_or("👋", |e| e.toned(tone).glyph);
        let button =
            gtk::MenuButton::builder().label(sample(current)).tooltip_text("Skin tone").css_classes(["flat"]).build();
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let choices = std::iter::once(None).chain(Tone::ALL.into_iter().map(Some));
        for tone in choices {
            let row = gtk::Button::builder()
                .css_classes(["flat"])
                .tooltip_text(tone.map_or("Default Skin Tone", Tone::label))
                .label(sample(tone))
                .build();
            row.connect_clicked({
                let (this, button, popover, chosen) =
                    (Rc::downgrade(self), button.clone(), popover.clone(), chosen.clone());
                move |_| {
                    popover.popdown();
                    let Some(this) = this.upgrade() else { return };
                    button.set_label(sample(tone));
                    this.change_setting(Setting::SkinTone(tone));
                    chosen();
                }
            });
            list.append(&row);
        }
        popover.set_child(Some(&list));
        button.set_popover(Some(&popover));
        button.upcast()
    }

    /// The emoji a status may carry: every server's, as in a DM, where the
    /// client applies the same Nitro rules (`getEmojiUnavailableReason`,
    /// 690521, with the status intention and no channel).
    pub(super) fn status_emoji_library(&self) -> Library {
        let state = self.state.borrow();
        Library::new(state.guilds_in_picker_order(None), &state.limits(), &Place { guild: None, external: true })
    }
}

/// What the picker lists for `query`: each server's emoji and the Unicode
/// groups, or what a search finds.
/// The emoji the picker lists before the servers and groups.
#[derive(Default)]
struct Shortcuts {
    favorites: Vec<Item>,
    frequent: Vec<Item>,
}

fn rows(library: &Library, query: &str, tone: Option<Tone>, shortcuts: Shortcuts, scores: &Scores) -> Vec<Row<Item>> {
    let mut rows = Vec::new();
    if query.is_empty() {
        let toned = |items: Vec<Item>| items.into_iter().map(|item| toned(item, tone)).collect();
        Row::section(&mut rows, COLUMNS, "Favorites".to_owned(), toned(shortcuts.favorites));
        Row::section(&mut rows, COLUMNS, "Frequently Used".to_owned(), toned(shortcuts.frequent));
        for server in library.picker() {
            Row::section(&mut rows, COLUMNS, server.name, server.emoji.into_iter().map(Item::Custom).collect());
        }
        for category in Category::ALL {
            let unicode = discord::emoji::unicode_in(category).map(|e| Item::Unicode(e.toned(tone))).collect();
            Row::section(&mut rows, COLUMNS, category.label().to_owned(), unicode);
        }
    } else {
        let found = library.search(query, SEARCH, scores);
        let toned = |items: Vec<Item>| items.into_iter().map(|item| toned(item, tone)).collect();
        Row::section(&mut rows, COLUMNS, "Search Results".to_owned(), toned(found.unlocked));
        Row::section(&mut rows, COLUMNS, "Needs Nitro".to_owned(), toned(found.locked));
    }
    rows
}

/// A Unicode emoji in the account's skin tone, if it has one.
pub(super) fn toned(item: Item, tone: Option<Tone>) -> Item {
    match item {
        Item::Unicode(e) => Item::Unicode(e.toned(tone)),
        custom => custom,
    }
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
