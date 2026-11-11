//! The GIF picker in the window. What it asks Discord and when is
//! `discord::gifs`; this draws the front page and the results, and sends a
//! pick.
//!
//! A pick sends the GIF's link as a message at once, whatever the box holds,
//! which it leaves alone (the client's `isGif` branch in `handleSendMessage`,
//! 699344). With Shift it puts the link in the box instead.
//!
//! Left out: the client's `POST /gifs/select` after each pick (`g4`, 497685),
//! which only reports which result was chosen for which search, and Neucord
//! sends no analytics (#92).
//!
//! A star on each GIF favourites it, or takes it back out, and the front
//! page leads with the Favorites (`439401`, `649529`); a favourite whose
//! preview is a video, which another client saved, shows as a button.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::Snowflake;
use discord::gifs::{Gif, Kind, Picker, Request, Screen, Tile, Typed};
use gtk::glib;

use super::App;
use super::menus::{self, Entry as MenuEntry, Item as MenuItem, Kind as MenuKind};
use super::picker::{self, Handle, Hooks, Row, Spec, Target};
use super::uploads::Stopped;

/// GIFs to a row.
const COLUMNS: usize = 3;
const GIF: (i32, i32) = (150, 100);
/// Front page tiles to a row.
const TILE_COLUMNS: usize = 2;
const TILE: (i32, i32) = (225, 100);

/// The GIF a menu was opened on.
fn gif(target: &Target) -> Option<&Gif> {
    match target {
        Target::Gif(gif) => Some(gif),
        _ => None,
    }
}

/// The picker's menu (`gif-picker`, 400017) has this one item, for any GIF
/// of the results.
pub(super) const COPY_LINK: MenuEntry<Target> = MenuEntry::Item(MenuItem {
    name: "copy-gif-link",
    label: "Copy _Link",
    shows: |_, target| gif(target).is_some(),
    kind: MenuKind::Action(|app, target| {
        if let Some(gif) = gif(target) {
            app.copy(&gif.url);
        }
    }),
});

/// A cell of the list.
#[derive(Clone)]
enum Item {
    Tile(Tile),
    Gif(Gif),
    /// A search the client suggests instead.
    Suggestion(String),
}

/// One opening of the picker.
struct Session {
    app: std::rc::Weak<App>,
    channel: Snowflake,
    view: RefCell<Picker>,
    handle: RefCell<Option<Handle>>,
    /// The search waiting out its debounce.
    debounce: RefCell<Option<glib::SourceId>>,
    /// The favourite whose picture the front page's tile shows, chosen once
    /// a time the front page shows (`favoritesTile`, 649529).
    tile_sample: std::cell::Cell<Option<usize>>,
}

impl App {
    pub(super) fn build_gifs(self: &Rc<Self>) {
        self.gif_button.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                let Some(this) = this.upgrade() else { return };
                if let Some(channel) = this.sticker_channel() {
                    this.open_gif_picker(channel, button);
                }
            }
        });
    }

    /// Open the picker over `parent`, for GIFs to send in `channel`. The
    /// front page is fetched as the client does: when it has none yet.
    fn open_gif_picker(self: &Rc<Self>, channel: Snowflake, parent: &impl IsA<gtk::Widget>) {
        let session = Rc::new(Session {
            app: Rc::downgrade(self),
            channel,
            view: RefCell::default(),
            handle: RefCell::default(),
            debounce: RefCell::default(),
            tile_sample: std::cell::Cell::default(),
        });
        let spec = Spec {
            placeholder: "Search GIFs",
            columns: COLUMNS,
            cell_width: GIF.0,
            rows: Rc::new({
                let session = session.clone();
                move |_| session.rows()
            }),
            cell: Rc::new({
                let session = session.clone();
                move |app, item, _| session.cell(app, item)
            }),
            // Enter picks nothing: a search has no best result.
            best: Rc::new(|_| None),
        };
        let hooks = Hooks {
            typed: Some(Rc::new({
                let session = session.clone();
                move |text| session.typed(text)
            })),
            scrolled: Some(Rc::new({
                let session = session.clone();
                move |remaining| session.scrolled(remaining)
            })),
            back: Some(Rc::new({
                let session = session.clone();
                move || session.back()
            })),
            ..Default::default()
        };
        let handle = picker::open_with(self, parent, spec, hooks, |_, _| ());
        *session.handle.borrow_mut() = Some(handle);
        session.fetch_tiles();
        // The favourites come from Discord the first time (`IE`, 996566), and fill in then.
        self.load_frecency({
            let session = session.clone();
            move || session.reload()
        });
    }

    /// What picking `gif` does: send its link as a message, or with Shift
    /// put it in the box.
    fn pick_gif(self: &Rc<Self>, channel: Snowflake, gif: &Gif) {
        if self.shift_held() {
            let buffer = self.composer.buffer();
            let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            if !typed.is_empty() && !typed.ends_with(' ') {
                buffer.insert_at_cursor(" ");
            }
            buffer.insert_at_cursor(&gif.url);
        } else {
            self.send_gif(channel, &gif.url);
        }
        // The popover hands focus back to the button as it closes.
        let composer = self.composer.clone();
        glib::idle_add_local_once(move || {
            composer.grab_focus();
        });
    }

    /// Send `link` as a message. The box and the files and sticker waiting
    /// with it stay; a reply goes with it, and ends. What the client checks
    /// first (`nA.i` in 710272) is checked here: length and slowmode.
    fn send_gif(self: &Rc<Self>, channel: Snowflake, link: &str) {
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest else { return };
        // A hidden message box is a channel we must never post to (#51).
        if !self.composer_box.is_visible() || !self.state.borrow().can_send(channel) {
            return self.update_notice_bar();
        }
        let too_long = self.state.borrow().limits().check_length(link);
        if let Err(too_long) = too_long {
            return self.show_too_long(too_long);
        }
        if self.slowmode_left(channel) > 0 {
            return self.update_notice_bar();
        }
        let can_reply = self.state.borrow().can_reply(channel);
        let replying = self.reply.borrow().clone().filter(|r| r.channel == channel && can_reply);
        let reply = replying.as_ref().map(|r| discord::rest::Reply {
            to: r.to,
            guild: r.guild,
            ping: r.fixed_ping.unwrap_or(self.reply_ping.is_active()),
        });
        let ping = self.reply_ping.is_active();
        self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, super::state::now_ms()));
        self.typing_message_sent(channel);
        self.set_reply(None);
        let (this, link) = (self.clone(), link.to_owned());
        glib::spawn_future_local(async move {
            match rest.send_message(channel, &link, reply, &[], None).await {
                Ok(_) => this.message_sent(channel),
                Err(e) => {
                    this.message_stopped(channel, &Stopped::Send(e));
                    if this.reply.borrow().is_none() && this.state.borrow().channel == Some(channel) {
                        this.set_reply(replying);
                        this.reply_ping.set_active(ping);
                    }
                }
            }
        });
    }
}

/// `items`, `columns` to a row.
fn cells(columns: usize, items: Vec<Item>) -> Vec<Row<Item>> {
    items.chunks(columns).map(|cells| Row::Cells(cells.to_vec())).collect()
}

impl Session {
    /// List what the picker shows now.
    fn rows(&self) -> Vec<Row<Item>> {
        let Some(app) = self.app.upgrade() else { return Vec::new() };
        let view = self.view.borrow();
        let note = |text: &str| vec![Row::Header(text.to_owned())];
        match view.screen() {
            Screen::Front => {
                let state = app.state.borrow();
                let favorites = &state.frecency.favorite_gifs;
                let sample = match self.tile_sample.get() {
                    Some(at) if at < favorites.len() => Some(at),
                    _ if favorites.is_empty() => None,
                    _ => {
                        let at = glib::random_int_range(0, favorites.len() as i32) as usize;
                        self.tile_sample.set(Some(at));
                        Some(at)
                    }
                };
                let first = Tile::favorites(sample.and_then(|at| favorites.get(at)));
                let tiles = state.gif_tiles.all();
                let mut rows =
                    cells(TILE_COLUMNS, std::iter::once(first).chain(tiles.iter().cloned()).map(Item::Tile).collect());
                if tiles.is_empty() {
                    rows.extend(note("Loading…"));
                }
                rows
            }
            Screen::Favorites => {
                let state = app.state.borrow();
                let held = &state.frecency.favorite_gifs;
                let mut rows = note("Favorites");
                if held.is_empty() {
                    rows.extend(note("Favorites will show up here!"));
                    rows.extend(note("Click the star in the corner of a gif to favorite it"));
                    return rows;
                }
                let listed = discord::gifs::favorites_for(held, view.query());
                if listed.is_empty() {
                    rows.extend(note("No favorite GIFs match your search."));
                } else {
                    rows.extend(cells(COLUMNS, listed.into_iter().map(Item::Gif).collect()));
                }
                rows
            }
            Screen::Loading => note("Loading…"),
            Screen::Empty => note("No GIFs found"),
            Screen::Results { gifs, suggestions } => {
                let mut rows = Vec::new();
                if view.page() == Some(Kind::TrendingGifs) {
                    rows.extend(note("Trending GIFs"));
                }
                rows.extend(cells(COLUMNS, gifs.iter().cloned().map(Item::Gif).collect()));
                if !suggestions.is_empty() {
                    rows.extend(note("Didn't find what you were looking for? Try:"));
                    rows.extend(cells(COLUMNS, suggestions.iter().cloned().map(Item::Suggestion).collect()));
                }
                rows
            }
        }
    }

    fn cell(self: &Rc<Self>, app: &Rc<App>, item: &Item) -> gtk::Widget {
        match item {
            Item::Gif(found) => self.gif_cell(app, found),
            Item::Tile(tile) => self.tile_cell(app, tile),
            Item::Suggestion(text) => {
                let button = gtk::Button::builder().label(text).css_classes(["pill"]).build();
                let (this, text) = (self.clone(), text.clone());
                button.connect_clicked(move |_| this.suggestion(&text));
                button.upcast()
            }
        }
    }

    /// A GIF of the results: its animated preview, which picks it, and the
    /// picker's menu on a right click, with a star in the corner that
    /// favourites it.
    fn gif_cell(self: &Rc<Self>, app: &Rc<App>, found: &Gif) -> gtk::Widget {
        let face: gtk::Widget = if found.video {
            // What another client saved as a video, which Neucord can't play here.
            gtk::Image::builder()
                .icon_name("media-playback-start-symbolic")
                .pixel_size(32)
                .width_request(GIF.0)
                .height_request(GIF.1)
                .build()
                .upcast()
        } else {
            let picture = gtk::Picture::builder()
                .width_request(GIF.0)
                .height_request(GIF.1)
                .content_fit(gtk::ContentFit::Cover)
                .build();
            app.images.set(&picture, &found.preview(), true);
            picture.upcast()
        };
        let button = gtk::Button::builder().child(&face).css_classes(["flat"]).build();
        button.connect_clicked({
            let (this, found) = (self.clone(), found.clone());
            move |_| {
                if let Some(app) = this.app.upgrade() {
                    app.pick_gif(this.channel, &found);
                }
                if let Some(handle) = &*this.handle.borrow() {
                    handle.close();
                }
            }
        });
        menus::on_secondary(&button, {
            let (app, found, button) = (Rc::downgrade(app), found.clone(), button.downgrade());
            move |x, y| {
                let (Some(app), Some(button)) = (app.upgrade(), button.upgrade()) else { return };
                menus::popup(&app, menus::GIF_PICKER, Target::Gif(found.clone()), &button, menus::At::Point(x, y));
            }
        });
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&button));
        overlay.add_overlay(&self.star(app, found));
        overlay.upcast()
    }

    /// The star that favourites `found`, or takes it back out. Its state is the
    /// account's, so it is drawn again when that changes: the Favorites page
    /// lists again, which drops a GIF no longer a favourite.
    fn star(self: &Rc<Self>, app: &Rc<App>, found: &Gif) -> gtk::Button {
        let favorite = app.is_favorite_gif(found);
        let star = gtk::Button::builder()
            .icon_name(if favorite { "starred-symbolic" } else { "non-starred-symbolic" })
            .tooltip_text(if favorite { "Remove from Favorites" } else { "Add to Favorites" })
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(4)
            .margin_end(4)
            .css_classes(["osd", "circular"])
            .build();
        star.connect_clicked({
            let (this, found) = (self.clone(), found.clone());
            move |_| {
                let Some(app) = this.app.upgrade() else { return };
                let favorite = app.is_favorite_gif(&found);
                app.favorite_gif(&found, !favorite);
                this.reload();
            }
        });
        star
    }

    /// A tile of the front page: its preview under its name.
    fn tile_cell(self: &Rc<Self>, app: &Rc<App>, tile: &Tile) -> gtk::Widget {
        let overlay = gtk::Overlay::new();
        let picture = gtk::Picture::builder()
            .width_request(TILE.0)
            .height_request(TILE.1)
            .content_fit(gtk::ContentFit::Cover)
            .build();
        if let Some(preview) = tile.preview() {
            app.images.set(&picture, &preview, true);
        }
        overlay.set_child(Some(&picture));
        overlay.add_overlay(
            &gtk::Label::builder()
                .label(&tile.name)
                .halign(gtk::Align::Start)
                .valign(gtk::Align::End)
                .margin_start(6)
                .margin_bottom(6)
                .css_classes(["osd"])
                .build(),
        );
        let button = gtk::Button::builder().child(&overlay).css_classes(["flat"]).build();
        let (this, tile) = (self.clone(), tile.clone());
        button.connect_clicked(move |_| this.open_tile(&tile));
        button.upcast()
    }

    fn reload(&self) {
        if let Some(handle) = &*self.handle.borrow() {
            handle.reload();
        }
    }

    /// Show the search box and back button as the page needs them, with
    /// `text` in the box.
    fn sync(&self, text: &str) {
        if let Some(handle) = &*self.handle.borrow() {
            let view = self.view.borrow();
            handle.set_search(text);
            handle.set_placeholder(if view.page() == Some(Kind::Favorites) {
                "Search Favorites"
            } else {
                "Search GIFs"
            });
            handle.set_chrome(view.shows_search(self.favorites()), view.has_back());
        }
        self.reload();
    }

    /// Ask for the front page, if it has none yet.
    fn fetch_tiles(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let request = app.state.borrow_mut().gif_tiles.opened();
        if let Some(request) = request {
            self.send(request);
        }
    }

    fn typed(self: &Rc<Self>, text: &str) {
        let typed = self.view.borrow_mut().typed(text);
        match typed {
            Typed::Front => {
                if let Some(handle) = &*self.handle.borrow() {
                    let view = self.view.borrow();
                    handle.set_chrome(view.shows_search(self.favorites()), view.has_back());
                }
                self.fetch_tiles();
            }
            // The page lists again for what is typed.
            Typed::Filter => self.reload(),
            Typed::Search(query) => {
                if let Some(handle) = &*self.handle.borrow() {
                    handle.set_chrome(true, true);
                }
                let this = self.clone();
                let wait = Duration::from_millis(discord::gifs::SEARCH_DEBOUNCE_MS.into());
                let source = glib::timeout_add_local_once(wait, move || {
                    this.debounce.borrow_mut().take();
                    this.send(Request::Search(query));
                });
                if let Some(old) = self.debounce.borrow_mut().replace(source) {
                    old.remove();
                }
            }
        }
    }

    fn open_tile(self: &Rc<Self>, tile: &Tile) {
        let request = self.view.borrow_mut().open(tile);
        let text = if matches!(tile.kind, Kind::TrendingGifs | Kind::Favorites) { "" } else { tile.name.as_str() };
        self.sync(text);
        if let Some(request) = request {
            self.send(request);
        }
    }

    /// How many favourites the account has.
    fn favorites(&self) -> usize {
        self.app.upgrade().map_or(0, |app| app.state.borrow().frecency.favorite_gifs.len())
    }

    fn suggestion(self: &Rc<Self>, text: &str) {
        let request = {
            let mut view = self.view.borrow_mut();
            view.clear();
            view.searched(text, Kind::Search)
        };
        self.sync(text);
        self.send(request);
    }

    fn back(self: &Rc<Self>) {
        self.view.borrow_mut().clear();
        self.tile_sample.set(None);
        self.sync("");
        self.fetch_tiles();
    }

    fn scrolled(self: &Rc<Self>, remaining: f64) {
        let request = self.view.borrow_mut().scrolled(remaining);
        if let Some(request) = request {
            self.send(request);
        }
    }

    /// Make `request` and take in the answer.
    fn send(self: &Rc<Self>, request: Request) {
        let Some(rest) = self.app.upgrade().and_then(|app| app.state.borrow().rest.clone()) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match request {
                Request::Trending => {
                    let tiles = rest.gif_tiles().await.map_err(|e| eprintln!("gifs: {e}")).ok();
                    if let Some(app) = this.app.upgrade() {
                        app.state.borrow_mut().gif_tiles.fetched(tiles);
                    }
                }
                Request::TrendingGifs => {
                    let gifs = rest.gifs_trending().await.map_err(|e| eprintln!("gifs: {e}")).ok();
                    this.view.borrow_mut().answered_trending(gifs);
                }
                Request::Search(query) => {
                    let gifs = rest.gifs_search(&query).await.map_err(|e| eprintln!("gifs: {e}")).ok();
                    this.view.borrow_mut().answered(&query, gifs);
                }
                Request::Suggest(query) => {
                    if let Ok(found) = rest.gifs_suggest(&query).await {
                        this.view.borrow_mut().suggested(found);
                    }
                }
            }
            this.reload();
        });
    }
}
