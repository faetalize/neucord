//! The favourite emoji and the emoji used most: the settings proto of type
//! 2 (see [`discord::frecency`]). The client loads it the first time
//! something needs it, which here is the emoji picker opening; changes to
//! the favourites save as the other settings do.

use std::rc::Rc;

use discord::emoji::{Item, Library};
use discord::frecency::{Frecency, FrecencyPatch, MAX_FAVORITE_EMOJI, Setting, TooMany, toggled};
use discord::settings_sync::Schedule;
use gtk::glib;

use super::App;

impl App {
    /// Load the favourites if they aren't yet, as `loadIfNecessary` does for
    /// the emoji picker (920134), then run `loaded`.
    pub(super) fn load_frecency(self: &Rc<Self>, loaded: impl Fn() + 'static) {
        let rest = {
            let mut state = self.state.borrow_mut();
            if state.frecency_sync.is_loaded() || state.frecency_loading {
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
            match proto {
                Ok(proto) => {
                    let patch = FrecencyPatch::from_proto_bytes(&proto);
                    let offline = state.frecency_sync.connected(proto);
                    state.frecency = Frecency::default();
                    state.frecency.apply(patch.unwrap_or_default());
                    drop(state);
                    loaded();
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
        if !partial {
            state.frecency = Frecency::default();
        }
        state.frecency.apply(patch);
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
        let edited = self.state.borrow_mut().frecency_sync.edit(Setting::FavoriteEmojis(list));
        let Some(edited) = edited else { return self.toast("Favorites aren't loaded yet") };
        self.state.borrow_mut().frecency.apply(edited.patch);
        if let Some(schedule) = edited.schedule {
            self.save_frecency_after(schedule);
        }
    }

    fn save_frecency_after(self: &Rc<Self>, schedule: Schedule) {
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(schedule.after, move || {
            if let Some(this) = this.upgrade() {
                this.save_frecency(schedule.generation);
            }
        });
    }

    fn save_frecency(self: &Rc<Self>, generation: u64) {
        let (save, rest) = {
            let mut state = self.state.borrow_mut();
            (state.frecency_sync.save(generation), state.rest.clone())
        };
        let (Some(save), Some(rest)) = (save, rest) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            use discord::settings_sync::SaveError;
            match rest.save_settings(&save).await {
                Ok(saved) => {
                    let applied = this.state.borrow_mut().frecency_sync.saved(&save, &saved);
                    if let Some(applied) = applied {
                        let mut state = this.state.borrow_mut();
                        state.frecency = Frecency::default();
                        state.frecency.apply(applied.patch);
                        drop(state);
                        if let Some(schedule) = applied.schedule {
                            this.save_frecency_after(schedule);
                        }
                    }
                }
                Err(SaveError::RateLimited { retry_after }) => {
                    let retry = this.state.borrow_mut().frecency_sync.rate_limited(retry_after.as_deref());
                    this.save_frecency_after(retry);
                }
                Err(SaveError::Invalid) => {
                    // The client asks for the settings again and starts over.
                    match rest.settings_proto(2).await {
                        Ok(proto) => {
                            let patch = this.state.borrow_mut().frecency_sync.reloaded(proto);
                            let mut state = this.state.borrow_mut();
                            state.frecency = Frecency::default();
                            state.frecency.apply(patch.unwrap_or_default());
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
