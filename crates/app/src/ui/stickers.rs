//! Stickers in the window: the library of the open channel, the picker, and
//! the sticker waiting above the message box. What the picker offers and
//! whether we may send a sticker is `discord::stickers` and `limits`; the
//! picker is `picker`'s, with the emoji picker's menu.
//!
//! A pick sends at once from an empty box, as in the client (`nm` in
//! 989161); with text or files in it, or another sticker waiting, it waits
//! above the box instead, and goes with the message.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use discord::Snowflake;
use discord::limits::StickerUnavailable;
use discord::permissions::{self, Permissions};
use discord::stickers::{Entry, Library, Listed, Place};
use gtk::glib;

use super::App;
use super::chat::{self, MessageTarget};
use super::menus::{self, Entry as MenuEntry, Item as MenuItem, Kind};
use super::picker::{self, Row, Spec, Target};
use super::state::now_unix;

/// Stickers to a row of the picker.
const COLUMNS: usize = 4;
/// What a sticker is shown at in the picker (the client's small size).
const SIZE: i32 = 80;
/// The size of the one waiting above the message box.
const WAITING: i32 = 48;

/// Only in Developer Mode, like the rest of the picker's menu (233503).
fn developer(app: &App) -> bool {
    app.state.borrow().developer_mode
}

fn sticker(target: &Target) -> Option<&discord::Sticker> {
    match target {
        Target::Sticker(sticker) => Some(sticker),
        Target::Emoji(_) | Target::Gif(_) => None,
    }
}

pub(super) const COPY_STICKER_ID: MenuEntry<Target> = MenuEntry::Item(MenuItem {
    name: "copy-sticker-id",
    label: "Copy Sticker _ID",
    shows: |app, target| developer(app) && sticker(target).is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(sticker) = sticker(target) {
            app.copy(&sticker.id.to_string());
        }
    }),
});

pub(super) const COPY_STICKER_LINK: MenuEntry<Target> = MenuEntry::Item(MenuItem {
    name: "copy-sticker-link",
    label: "Copy Sticker Image _Link",
    shows: |app, target| developer(app) && sticker(target).and_then(|s| s.link()).is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(link) = sticker(target).and_then(|s| s.link()) {
            app.copy(&link);
        }
    }),
});

/// "Message Details" where a sticker was right-clicked, in Developer Mode: the
/// client (761304) shows it in place of Copy Message ID, with that, the
/// sticker's id and its image link, and a way to open the link. It's the same
/// for an emoji, which Neucord's message menu has nothing for yet.
pub(super) const MESSAGE_DETAILS: MenuEntry<MessageTarget> = MenuEntry::Item(MenuItem {
    name: "message-details",
    label: "Message _Details",
    shows: |app, target| developer(app) && target.sticker().is_some(),
    kind: Kind::Submenu(&[&[DETAILS_MESSAGE_ID, DETAILS_STICKER_ID, DETAILS_STICKER_LINK, DETAILS_OPEN_LINK]]),
});

const DETAILS_MESSAGE_ID: MenuEntry<MessageTarget> = MenuEntry::Item(MenuItem {
    name: "details-copy-message-id",
    label: "Copy Message _ID",
    shows: |_, _| true,
    kind: Kind::Action(chat::copy_message_id),
});

const DETAILS_STICKER_ID: MenuEntry<MessageTarget> = MenuEntry::Item(MenuItem {
    name: "details-copy-sticker-id",
    label: "Copy Sticker _ID",
    shows: |_, target| target.sticker().is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(sticker) = target.sticker() {
            app.copy(&sticker.id.to_string());
        }
    }),
});

const DETAILS_STICKER_LINK: MenuEntry<MessageTarget> = MenuEntry::Item(MenuItem {
    name: "details-copy-sticker-link",
    label: "Copy Sticker Image _Link",
    shows: |_, target| target.sticker().and_then(|s| s.link()).is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(link) = target.sticker().and_then(|s| s.link()) {
            app.copy(&link);
        }
    }),
});

const DETAILS_OPEN_LINK: MenuEntry<MessageTarget> = MenuEntry::Item(MenuItem {
    name: "details-open-sticker-link",
    label: "_Open Sticker Image Link",
    shows: |_, target| target.sticker().and_then(|s| s.link()).is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(link) = target.sticker().and_then(|s| s.link()) {
            gtk::UriLauncher::new(&link).launch(Some(&app.window), gtk::gio::Cancellable::NONE, |_| {});
        }
    }),
});

/// The sticker waiting to be sent with the next message, by channel, and
/// the strip above the message box that shows it. The client keeps one a
/// channel, and a new pick replaces it (`StickerMessagePreviewStore`).
pub(super) struct Waiting {
    by_channel: RefCell<HashMap<Snowflake, Listed>>,
    bar: gtk::Box,
    image: gtk::Picture,
    name: gtk::Label,
    remove: gtk::Button,
}

impl Waiting {
    pub(super) fn new() -> Self {
        let image = gtk::Picture::builder()
            .width_request(WAITING)
            .height_request(WAITING)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        let name = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove sticker")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let bar = gtk::Box::builder().spacing(8).margin_start(12).margin_top(4).visible(false).build();
        bar.append(&image);
        bar.append(&name);
        bar.append(&remove);
        Waiting { by_channel: RefCell::default(), bar, image, name, remove }
    }

    pub(super) fn of(&self, channel: Snowflake) -> Option<Listed> {
        self.by_channel.borrow().get(&channel).cloned()
    }

    pub(super) fn set(&self, channel: Snowflake, listed: Option<Listed>) {
        match listed {
            Some(listed) => self.by_channel.borrow_mut().insert(channel, listed),
            None => self.by_channel.borrow_mut().remove(&channel),
        };
    }
}

impl App {
    /// The open channel, if a sticker or a GIF may be sent to it now: the
    /// message box shows and no edit has it, as the client's edit box takes
    /// none.
    pub(super) fn sticker_channel(&self) -> Option<Snowflake> {
        let channel = self.state.borrow().channel?;
        (self.composer_box.is_visible() && self.editing().is_none()).then_some(channel)
    }

    /// Where stickers would be sent: the channel's server and what we may
    /// use from others there.
    fn sticker_place(&self, channel: Snowflake) -> Place {
        let state = self.state.borrow();
        let found = state.find_channel(channel);
        let (emojis, stickers) = match (found, &state.account) {
            // A DM needs no permission.
            (Some((None, _)), _) => (true, true),
            (Some((Some(guild), channel)), Some(me)) => {
                let can = permissions::in_channel(guild, channel, me, now_unix());
                (can.contains(Permissions::USE_EXTERNAL_EMOJIS), can.contains(Permissions::USE_EXTERNAL_STICKERS))
            }
            _ => (false, false),
        };
        Place {
            guild: found.and_then(|(guild, _)| guild).map(|g| g.id),
            external_emojis: emojis,
            external_stickers: stickers,
        }
    }

    /// The stickers `channel` offers: every server's, the channel's own
    /// first and then the rail's order, and the standard packs.
    fn sticker_library(&self, channel: Snowflake) -> Library {
        let place = self.sticker_place(channel);
        let state = self.state.borrow();
        let guilds = state.guilds_in_picker_order(place.guild);
        Library::new(guilds, state.sticker_packs.all(), &state.limits(), &place)
    }

    /// Whether `listed` can still be sent in `channel`: what was true when
    /// it was picked may not be, as the box follows our permissions.
    pub(super) fn sticker_sendable(&self, channel: Snowflake, listed: &Listed) -> Result<(), StickerUnavailable> {
        self.state.borrow().limits().sticker(&listed.origin, &self.sticker_place(channel))
    }

    pub(super) fn build_stickers(self: &Rc<Self>, area: &gtk::Box) {
        area.append(&self.waiting.bar);
        self.waiting.remove.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade()
                    && let Some(channel) = this.state.borrow().channel
                {
                    this.set_waiting_sticker(channel, None);
                }
            }
        });
        self.sticker_button.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                let Some(this) = this.upgrade() else { return };
                if let Some(channel) = this.sticker_channel() {
                    this.open_sticker_picker(channel, button);
                }
            }
        });
    }

    /// Show the sticker button and the waiting sticker as they are now for
    /// the open channel.
    pub(super) fn refresh_stickers(self: &Rc<Self>) {
        let channel = self.sticker_channel();
        self.sticker_button.set_visible(channel.is_some());
        // The GIF button shows wherever the sticker button does.
        self.gif_button.set_visible(channel.is_some());
        let waiting = channel.and_then(|c| self.waiting.of(c));
        self.waiting.bar.set_visible(waiting.is_some());
        if let Some(listed) = waiting {
            self.waiting.name.set_label(&listed.sticker.name);
            match listed.sticker.still(request(self)) {
                Some(url) => self.images.set(&self.waiting.image, &url),
                None => self.waiting.image.set_paintable(None::<&gtk::gdk::Paintable>),
            }
        }
    }

    /// Wait to send `listed` with the next message in `channel`, or stop
    /// waiting (`None`).
    pub(super) fn set_waiting_sticker(self: &Rc<Self>, channel: Snowflake, listed: Option<Listed>) {
        self.waiting.set(channel, listed);
        self.refresh_stickers();
    }

    /// Open the picker over `parent`, for stickers to send in `channel`.
    /// The standard packs are fetched as the client does when its sticker
    /// view opens: unless a fetch is running or one finished within the hour.
    pub(super) fn open_sticker_picker(self: &Rc<Self>, channel: Snowflake, parent: &impl IsA<gtk::Widget>) {
        let library = Rc::new(RefCell::new(Rc::new(self.sticker_library(channel))));
        let spec = Spec {
            placeholder: "Search stickers",
            columns: COLUMNS,
            cell_width: SIZE + 8,
            rows: Rc::new({
                let library = library.clone();
                move |query| rows(&library.borrow(), query)
            }),
            cell: Rc::new(cell_button),
            best: Rc::new({
                let library = library.clone();
                move |query| library.borrow().search(query).unlocked.first().cloned()
            }),
        };
        let reload = picker::open(self, parent, spec, move |app, entry: &Entry| app.pick_sticker(channel, entry));
        let rest = {
            let mut state = self.state.borrow_mut();
            let now = (glib::monotonic_time() / 1000) as u64;
            let rest = state.rest.clone().filter(|_| state.sticker_packs.wants_fetch(now));
            if rest.is_some() {
                state.sticker_packs.fetch_started();
            }
            rest
        };
        if let Some(rest) = rest {
            let this = self.clone();
            glib::spawn_future_local(async move {
                match rest.sticker_packs().await {
                    Ok(packs) => {
                        let now = (glib::monotonic_time() / 1000) as u64;
                        this.state.borrow_mut().sticker_packs.fetched(packs, now);
                        *library.borrow_mut() = Rc::new(this.sticker_library(channel));
                        reload();
                    }
                    Err(e) => eprintln!("stickers: the standard packs weren't fetched: {e}"),
                }
            });
        }
    }

    /// What a pick does: send the sticker at once from an empty box, else
    /// wait above the box for the message.
    fn pick_sticker(self: &Rc<Self>, channel: Snowflake, entry: &Entry) {
        let buffer = self.composer.buffer();
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let alone =
            typed.trim().is_empty() && self.files_for_message(channel).is_empty() && self.waiting.of(channel).is_none();
        self.set_waiting_sticker(channel, Some(entry.listed.clone()));
        if alone {
            self.send_composer();
            // What stopped the send (slowmode, say) isn't kept waiting: the
            // client sends from an empty box or not at all.
            self.set_waiting_sticker(channel, None);
        }
        // The popover hands focus back to the button as it closes.
        let composer = self.composer.clone();
        glib::idle_add_local_once(move || {
            composer.grab_focus();
        });
    }
}

/// The size to ask the media proxy for.
fn request(app: &App) -> u32 {
    crate::images::request_size((WAITING, WAITING), app.images.scale(), None).0
}

/// What the picker lists for `query`: each server's stickers and the
/// standard packs, or what a search finds.
fn rows(library: &Library, query: &str) -> Vec<Row<Entry>> {
    let mut rows = Vec::new();
    if query.is_empty() {
        for section in library.servers.iter().chain(&library.packs) {
            Row::section(&mut rows, COLUMNS, section.name.clone(), section.stickers.clone());
        }
    } else {
        let found = library.search(query);
        Row::section(&mut rows, COLUMNS, "Search Results".to_owned(), found.unlocked);
        Row::section(&mut rows, COLUMNS, "Needs Nitro".to_owned(), found.locked);
    }
    rows
}

/// A sticker of the picker: a button that picks it, if we may send it, and
/// opens the picker's menu on a right click. A Lottie sticker, which Neucord
/// can't draw yet (#46), shows its name.
fn cell_button(app: &Rc<App>, entry: &Entry, picked: &Rc<dyn Fn(&Entry)>) -> gtk::Widget {
    let sticker = &entry.listed.sticker;
    let size = crate::images::request_size((SIZE, SIZE), app.images.scale(), None).0;
    let face: gtk::Widget = match sticker.still(size) {
        Some(url) => {
            let picture = gtk::Picture::builder()
                .width_request(SIZE)
                .height_request(SIZE)
                .content_fit(gtk::ContentFit::Contain)
                .build();
            // A still, even for an animated sticker: a screenful of them
            // moving would be a lot of drawing.
            app.images.set(&picture, &url);
            picture.upcast()
        }
        None => gtk::Label::builder()
            .label(&sticker.name)
            .width_request(SIZE)
            .height_request(SIZE)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            .css_classes(["dimmed", "caption"])
            .build()
            .upcast(),
    };
    let button = gtk::Button::builder().child(&face).css_classes(["flat"]).tooltip_text(&sticker.name).build();
    if entry.status.is_err() {
        // A locked sticker sends nothing, and the client offers Nitro on a
        // click, which Neucord doesn't copy.
        button.set_opacity(0.4);
    } else {
        let (entry, picked) = (entry.clone(), picked.clone());
        button.connect_clicked(move |_| picked(&entry));
    }
    menus::on_secondary(&button, {
        let (app, sticker, button) = (Rc::downgrade(app), sticker.clone(), button.downgrade());
        move |x, y| {
            let (Some(app), Some(button)) = (app.upgrade(), button.upgrade()) else { return };
            menus::popup(
                &app,
                menus::EXPRESSION_PICKER,
                Target::Sticker(sticker.clone()),
                &button,
                menus::At::Point(x, y),
            );
        }
    });
    button.upcast()
}
