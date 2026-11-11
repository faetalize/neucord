//! The favourite emoji and the emoji used most: the settings proto of type
//! 2 (see [`discord::frecency`]). The client loads it the first time
//! something needs it, which here is the emoji picker opening; changes to
//! the favourites save as the other settings do.

use std::rc::Rc;

use discord::emoji::{Item, Library};
use discord::frecency::{
    FrecencyPatch, MAX_FAVORITE_EMOJI, MAX_FAVORITE_STICKERS, Setting, TooMany, toggled, toggled_sticker,
};
use discord::gifs::{Gif, TooBig, favorited, unfavorited};
use discord::settings_sync::Schedule;
use gtk::glib;

use super::App;
use super::state::now_ms;

impl App {
    /// Load the favourites if they aren't yet, as `loadIfNecessary` does
    /// (the emoji picker opening, typing `:` to complete an emoji, the first look
    /// at what is used), then run `loaded`. Nothing runs if they already are.
    pub(super) fn load_frecency(self: &Rc<Self>, loaded: impl Fn() + 'static) {
        let rest = {
            let mut state = self.state.borrow_mut();
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
        glib::spawn_future_local(async move {
            let proto = rest.settings_proto(2).await;
            let mut state = this.state.borrow_mut();
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
        let mut state = self.state.borrow_mut();
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
        self.state.borrow().frecency.favorite_emojis.contains(&Library::key(item))
    }

    /// Favourite the emoji, or not (`V4` and `Sw` in 554375): at most
    /// [`MAX_FAVORITE_EMOJI`], which the client tells the user.
    pub(super) fn favorite_emoji(self: &Rc<Self>, item: &Item, favorite: bool) {
        let key = Library::key(item);
        let current = self.state.borrow().frecency.favorite_emojis.clone();
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
        self.state.borrow().frecency.favorite_stickers.contains(&id.0)
    }

    /// Favourite the sticker, or not (`uK` and `vr` in 631576): at most
    /// [`MAX_FAVORITE_STICKERS`], which the client tells the user.
    pub(super) fn favorite_sticker(self: &Rc<Self>, id: discord::Snowflake, favorite: bool) {
        let current = self.state.borrow().frecency.favorite_stickers.clone();
        match toggled_sticker(&current, id.0, favorite) {
            Ok(Some(list)) => self.edit_frecency(Setting::FavoriteStickers(list)),
            Ok(None) => {}
            Err(TooMany) => self.toast(&format!("You cannot have more than {MAX_FAVORITE_STICKERS} favorites.")),
        }
    }

    /// Favourite the GIF, or take it back out (`wg` and `Tr` in 497685).
    pub(super) fn favorite_gif(self: &Rc<Self>, gif: &Gif, favorite: bool) {
        let (held, hidden) = {
            let state = self.state.borrow();
            (state.frecency.favorite_gifs.clone(), state.frecency.hide_gif_tooltip)
        };
        let setting = match favorite {
            true => match favorited(gif, &held, hidden) {
                Ok((gifs, hidden)) => Setting::FavoriteGifs(gifs, hidden),
                Err(TooBig) => return self.toast("You cannot have more favorites."),
            },
            false => Setting::FavoriteGifs(unfavorited(&held, &gif.url), hidden),
        };
        self.edit_frecency(setting);
    }

    /// Whether the GIF is one of the account's favourites.
    pub(super) fn is_favorite_gif(&self, gif: &Gif) -> bool {
        gif.is_favorite(&self.state.borrow().frecency.favorite_gifs)
    }

    /// Change a setting of the proto and save it.
    fn edit_frecency(self: &Rc<Self>, setting: Setting) {
        let edited = self.state.borrow_mut().frecency_sync.edit(setting);
        let Some(edited) = edited else { return self.toast("Favorites aren't loaded yet") };
        self.state.borrow_mut().frecency.apply(edited.patch, now_ms() as u64);
        if let Some(schedule) = edited.schedule {
            self.save_frecency_after(schedule);
        }
    }

    pub(super) fn save_frecency_after(self: &Rc<Self>, schedule: Schedule) {
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(schedule.after, move || {
            if let Some(this) = this.upgrade() {
                this.save_frecency(schedule.generation);
            }
        });
    }

    fn save_frecency(self: &Rc<Self>, generation: u64) {
        let (save, rest, sent) = {
            let mut state = self.state.borrow_mut();
            // What is used goes in just before the send (`beforeSendCallbacks`), once loaded.
            let (added, sent) = match state.frecency_sync.is_loaded() {
                true => state.frecency.stores.before_send(now_ms() as u64),
                false => Default::default(),
            };
            (state.frecency_sync.save_with(generation, added), state.rest.clone(), sent)
        };
        let (Some(save), Some(rest)) = (save, rest) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            use discord::settings_sync::SaveError;
            match rest.save_settings(&save).await {
                Ok(saved) => {
                    // What the save carried is the account's now: the answer is what it holds.
                    let applied = {
                        let mut state = this.state.borrow_mut();
                        state.frecency.stores.saved(sent);
                        state.frecency_sync.saved(&save, &saved)
                    };
                    if let Some(applied) = applied {
                        this.state.borrow_mut().frecency.load(applied.patch, now_ms() as u64);
                        if let Some(schedule) = applied.schedule {
                            this.save_frecency_after(schedule);
                        }
                    }
                    this.usage_saved();
                }
                Err(SaveError::RateLimited { retry_after }) => {
                    let retry = this.state.borrow_mut().frecency_sync.rate_limited(retry_after.as_deref());
                    this.save_frecency_after(retry);
                }
                Err(SaveError::Invalid) => {
                    // The client asks for the settings again and starts over.
                    match rest.settings_proto(2).await {
                        Ok(proto) => {
                            let mut state = this.state.borrow_mut();
                            let patch = state.frecency_sync.reloaded(proto);
                            state.frecency.load(patch.unwrap_or_default(), now_ms() as u64);
                        }
                        Err(e) => eprintln!("favourites: {e}"),
                    }
                }
                Err(SaveError::Other(e)) => {
                    eprintln!("favourites: {e}");
                    this.state.borrow_mut().frecency_sync.failed();
                    this.toast("Could not save your favorites");
                }
            }
        });
    }
}
