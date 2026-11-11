//! The favourite emoji and the emoji used most: the settings proto of type
//! 2 (see [`discord::frecency`]). The client loads it the first time
//! something needs it, which here is the emoji picker opening; changes to
//! the favourites save as the other settings do.

use std::rc::{Rc, Weak};

use discord::emoji::{Item, Library};
use discord::frecency::{
    FrecencyPatch, MAX_FAVORITE_EMOJI, MAX_FAVORITE_STICKERS, Setting, TooMany, toggled, toggled_sticker,
};
use discord::gifs::{Starred, TooBig, favorited, is_favorite, unfavorited};
use discord::settings_sync::Schedule;
use gtk::glib;
use gtk::prelude::*;

use super::App;
use super::state::now_ms;

impl App {
    /// Load the favourites if they aren't yet, as `loadIfNecessary` does
    /// (the emoji picker opening, typing `:` to complete an emoji, the first look
    /// at what is used), then run `loaded`. Nothing runs if they already are.
    pub(super) fn load_frecency(self: &Rc<Self>, loaded: impl Fn() + 'static) {
        let rest = {
            let mut state = self.account.state.borrow_mut();
            if state.frecency_sync.is_loaded() {
                return;
            }
            state.frecency_waiting.push(Rc::new(loaded));
            if state.frecency_loading {
                return;
            }
            state.frecency_loading = true;
            state.rest.clone()
        };
        let Some(rest) = rest else { return };
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            let proto = rest.settings_proto(2).await;
            // Of an account that was left meanwhile: its state, which waited, is gone.
            if this.account.epoch.get() != epoch {
                return;
            }
            let mut state = this.account.state.borrow_mut();
            state.frecency_loading = false;
            let waiting = std::mem::take(&mut state.frecency_waiting);
            match proto {
                Ok(proto) => {
                    let patch = FrecencyPatch::from_proto_bytes(&proto);
                    let offline = state.frecency_sync.connected(proto);
                    state.frecency.load(patch.unwrap_or_default(), now_ms() as u64);
                    drop(state);
                    for loaded in waiting {
                        loaded();
                    }
                    if let Some(schedule) = offline {
                        this.save_frecency_after(schedule);
                    }
                }
                Err(e) => eprintln!("favourites: {e}"),
            }
        });
    }

    /// Another client changed the favourites.
    pub(super) fn frecency_updated(&self, patch: FrecencyPatch, proto: &[u8], partial: bool) {
        let mut state = self.account.state.borrow_mut();
        if !state.frecency_sync.is_loaded() {
            return;
        }
        state.frecency_sync.received(proto, partial);
        match partial {
            true => state.frecency.apply(patch, now_ms() as u64),
            false => state.frecency.load(patch, now_ms() as u64),
        }
    }

    /// Whether the emoji is one of the account's favourites.
    pub(super) fn is_favorite_emoji(&self, item: &Item) -> bool {
        self.account.state.borrow().frecency.favorite_emojis.contains(&Library::key(item))
    }

    /// Favourite the emoji, or not (`V4` and `Sw` in 554375): at most
    /// [`MAX_FAVORITE_EMOJI`], which the client tells the user.
    pub(super) fn favorite_emoji(self: &Rc<Self>, item: &Item, favorite: bool) {
        let key = Library::key(item);
        let current = self.account.state.borrow().frecency.favorite_emojis.clone();
        let list = match toggled(&current, &key, favorite) {
            Ok(Some(list)) => list,
            Ok(None) => return,
            Err(TooMany) => {
                return self.toast(&format!("You cannot have more than {MAX_FAVORITE_EMOJI} favorites."));
            }
        };
        self.edit_frecency(Setting::FavoriteEmojis(list));
    }

    /// Whether the sticker is one of the account's favourites.
    pub(super) fn is_favorite_sticker(&self, id: discord::Snowflake) -> bool {
        self.account.state.borrow().frecency.favorite_stickers.contains(&id.0)
    }

    /// Favourite the sticker, or not (`uK` and `vr` in 631576): at most
    /// [`MAX_FAVORITE_STICKERS`], which the client tells the user.
    pub(super) fn favorite_sticker(self: &Rc<Self>, id: discord::Snowflake, favorite: bool) {
        let current = self.account.state.borrow().frecency.favorite_stickers.clone();
        match toggled_sticker(&current, id.0, favorite) {
            Ok(Some(list)) => self.edit_frecency(Setting::FavoriteStickers(list)),
            Ok(None) => {}
            Err(TooMany) => self.toast(&format!("You cannot have more than {MAX_FAVORITE_STICKERS} favorites.")),
        }
    }

    /// Favourite the GIF, or take it back out (`wg` and `Tr` in 497685).
    pub(super) fn favorite_gif(self: &Rc<Self>, star: &Starred, favorite: bool) {
        let (held, hidden) = {
            let state = self.account.state.borrow();
            (state.frecency.favorite_gifs.clone(), state.frecency.hide_gif_tooltip)
        };
        let setting = match favorite {
            true => match favorited(star, &held, hidden) {
                Ok((gifs, hidden)) => Setting::FavoriteGifs(gifs, hidden),
                Err(TooBig) => return self.toast("You cannot have more favorites."),
            },
            false => Setting::FavoriteGifs(unfavorited(&held, &star.url), hidden),
        };
        self.edit_frecency(setting);
    }

    /// Whether the GIF is one of the account's favourites.
    pub(super) fn is_favorite_gif(&self, star: &Starred) -> bool {
        is_favorite(&self.account.state.borrow().frecency.favorite_gifs, &star.url)
    }

    /// Change a setting of the proto and save it.
    fn edit_frecency(self: &Rc<Self>, setting: Setting) {
        let edited = self.account.state.borrow_mut().frecency_sync.edit(setting);
        let Some(edited) = edited else { return self.toast("Favorites aren't loaded yet") };
        self.account.state.borrow_mut().frecency.apply(edited.patch, now_ms() as u64);
        if let Some(schedule) = edited.schedule {
            self.save_frecency_after(schedule);
        }
    }

    pub(super) fn save_frecency_after(self: &Rc<Self>, schedule: Schedule) {
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(schedule.after, move || {
            if let Some(this) = this.upgrade().filter(|this| this.account.epoch.get() == epoch) {
                this.save_frecency(schedule.generation);
            }
        });
    }

    fn save_frecency(self: &Rc<Self>, generation: u64) {
        let (save, rest, sent) = {
            let mut state = self.account.state.borrow_mut();
            // What is used goes in just before the send (`beforeSendCallbacks`), once loaded.
            let (added, sent) = match state.frecency_sync.is_loaded() {
                true => state.frecency.stores.before_send(now_ms() as u64),
                false => Default::default(),
            };
            (state.frecency_sync.save_with(generation, added), state.rest.clone(), sent)
        };
        let (Some(save), Some(rest)) = (save, rest) else { return };
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            use discord::settings_sync::SaveError;
            let answer = rest.save_settings(&save).await;
            // The answer is of an account that was left meanwhile, whose sync
            // state is gone: what it says is no one's.
            if this.account.epoch.get() != epoch {
                return;
            }
            match answer {
                Ok(saved) => {
                    // What the save carried is the account's now: the answer is what it holds.
                    let applied = {
                        let mut state = this.account.state.borrow_mut();
                        state.frecency.stores.saved(sent);
                        state.frecency_sync.saved(&save, &saved)
                    };
                    if let Some(applied) = applied {
                        this.account.state.borrow_mut().frecency.load(applied.patch, now_ms() as u64);
                        if let Some(schedule) = applied.schedule {
                            this.save_frecency_after(schedule);
                        }
                    }
                    this.usage_saved();
                }
                Err(SaveError::RateLimited { retry_after }) => {
                    let retry = this.account.state.borrow_mut().frecency_sync.rate_limited(retry_after.as_deref());
                    this.save_frecency_after(retry);
                }
                Err(SaveError::Invalid) => {
                    // The client asks for the settings again and starts over.
                    let reloaded = rest.settings_proto(2).await;
                    if this.account.epoch.get() != epoch {
                        return;
                    }
                    match reloaded {
                        Ok(proto) => {
                            let mut state = this.account.state.borrow_mut();
                            let patch = state.frecency_sync.reloaded(proto);
                            state.frecency.load(patch.unwrap_or_default(), now_ms() as u64);
                        }
                        Err(e) => eprintln!("favourites: {e}"),
                    }
                }
                Err(SaveError::Other(e)) => {
                    eprintln!("favourites: {e}");
                    this.account.state.borrow_mut().frecency_sync.failed();
                    this.toast("Could not save your favorites");
                }
            }
        });
    }
}

/// What puts the star on a GIF of a message (`FavoriteGIF`, 439401): a button
/// over the corner that shows while the GIF plays, which is always where
/// the viewer's `gif_auto_play` is on and else under the pointer or focus.
#[derive(Clone)]
pub(super) struct Stars {
    app: Weak<App>,
    autoplay: bool,
}

impl Stars {
    pub(super) fn new(app: &Rc<App>, autoplay: bool) -> Stars {
        Stars { app: Rc::downgrade(app), autoplay }
    }

    /// `child` with its star, where it has one.
    pub(super) fn over(&self, child: &impl IsA<gtk::Widget>, star: Option<&Starred>) -> gtk::Widget {
        let Some(star) = star else { return child.clone().upcast() };
        let overlay = gtk::Overlay::builder().halign(gtk::Align::Start).child(child).build();
        let button = gtk::Button::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(4)
            .margin_end(4)
            .css_classes(["osd", "circular"])
            .build();
        let shown = Rc::new(std::cell::Cell::new(self.autoplay));
        button.set_opacity(if self.autoplay { 1.0 } else { 0.0 });
        let (app, star) = (self.app.clone(), star.clone());
        // Draws the star as the account has it. The first look at it, by the
        // pointer or by focus, also asks for the favourites if they aren't
        // here yet (`FavoriteGIF` loads them on its first mouse enter or focus).
        let refresh = {
            let (app, star, button) = (app.clone(), star.clone(), button.downgrade());
            Rc::new(move || {
                let (Some(app), Some(button)) = (app.upgrade(), button.upgrade()) else { return };
                let favorite = app.is_favorite_gif(&star);
                button.set_icon_name(if favorite { "starred-symbolic" } else { "non-starred-symbolic" });
                button.set_tooltip_text(Some(if favorite { "Remove from Favorites" } else { "Add to Favorites" }));
            })
        };
        refresh();
        let look = {
            let (app, refresh) = (app.clone(), refresh.clone());
            move || {
                refresh();
                if let Some(app) = app.upgrade() {
                    app.load_frecency({
                        let refresh = refresh.clone();
                        move || refresh()
                    });
                }
            }
        };
        let reveal = {
            let (button, shown, autoplay) = (button.downgrade(), shown.clone(), self.autoplay);
            move |on: bool| {
                shown.set(on);
                if let Some(button) = button.upgrade() {
                    button.set_opacity(if on || autoplay { 1.0 } else { 0.0 });
                }
            }
        };
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let (look, reveal) = (look.clone(), reveal.clone());
            move |_, _, _| {
                look();
                reveal(true);
            }
        });
        motion.connect_leave({
            let (reveal, button) = (reveal.clone(), button.downgrade());
            move |_| {
                if !button.upgrade().is_some_and(|button| button.has_focus()) {
                    reveal(false);
                }
            }
        });
        overlay.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter({
            let (look, reveal) = (look, reveal.clone());
            move |_| {
                look();
                reveal(true);
            }
        });
        focus.connect_leave(move |_| reveal(false));
        button.add_controller(focus);
        button.connect_clicked(move |_| {
            let Some(app) = app.upgrade() else { return };
            let toggle = {
                let (app, star, refresh) = (app.clone(), star.clone(), refresh.clone());
                move || {
                    let favorite = app.is_favorite_gif(&star);
                    app.favorite_gif(&star, !favorite);
                    refresh();
                }
            };
            // Saving needs the favourites, which a click may beat.
            let loaded = app.account.state.borrow().frecency_sync.is_loaded();
            match loaded {
                true => toggle(),
                false => app.load_frecency(toggle),
            }
        });
        overlay.add_overlay(&button);
        overlay.upcast()
    }
}
