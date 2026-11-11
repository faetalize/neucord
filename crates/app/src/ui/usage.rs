//! Counting what the account uses, and saving the counts as the client does
//! (module 416380): emoji sent and reacted with, stickers sent, servers and
//! channels opened. Slash commands, apps and sounds, which the client counts
//! too, have no surface here. What is counted waits as pending, on disk
//! so a closed app keeps it, until a save of the second settings proto
//! carries it (see [`discord::usage`]).
//!
//! The client looks at its pending uses 10 ms to 10 s after connecting, every
//! two hours and a bit, and when the connection closes. Each time, with
//! something pending, it loads the proto if it hasn't (`R`) and asks for a
//! save at once, which adds the uses just before it sends.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use discord::Snowflake;
use gtk::glib;

use super::App;
use super::state::now_ms;

/// How long after connecting the first look takes: 10 ms and up to 10
/// seconds more (`p` in 416380, where the 10 is meant as seconds).
fn first_wait() -> Duration {
    Duration::from_micros(10_000 + glib::random_int_range(0, 10_000_000) as u64)
}

/// Between looks: two hours and up to ten minutes, which the client picks
/// once for a session (`m`).
fn look_interval() -> Duration {
    Duration::from_secs(2 * 3600) + Duration::from_secs(glib::random_int_range(0, 600) as u64)
}

/// Where the pending uses of an account are kept: a file for each, so no
/// account's uses are saved as another's.
fn usage_file_of(id: Snowflake) -> PathBuf {
    glib::user_data_dir().join("neucord").join(format!("usage-{}.json", id.0))
}

/// Forget what is pending for an account for good, as logging it out does.
pub(super) fn forget_usage(id: Snowflake) {
    let _ = std::fs::remove_file(usage_file_of(id));
}

impl App {
    /// Where the pending uses of the shown account are kept.
    fn usage_file(&self) -> Option<PathBuf> {
        let me = self.account.state.borrow().me.as_ref()?.id;
        Some(usage_file_of(me))
    }

    /// Take back what a closed app left pending.
    pub(super) fn restore_usage(&self) {
        let Some(file) = self.usage_file() else { return };
        let Ok(json) = std::fs::read_to_string(file) else { return };
        let mut state = self.account.state.borrow_mut();
        if !state.frecency.stores.has_pending() {
            state.frecency.stores.restore_pending(&json);
        }
    }

    /// Keep what is pending, or nothing if there is none. Private to the
    /// user, as nothing else Neucord keeps is more telling.
    fn persist_usage(&self) {
        let Some(file) = self.usage_file() else { return };
        let (pending, json) = {
            let state = self.account.state.borrow();
            (state.frecency.stores.has_pending(), state.frecency.stores.pending_json())
        };
        if !pending {
            let _ = std::fs::remove_file(file);
            return;
        }
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(file.parent().unwrap_or(&file))?;
            let mut out =
                std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&file)?;
            out.write_all(json.as_bytes())
        };
        if let Err(e) = write() {
            eprintln!("usage: {e}");
        }
    }

    /// A message went out with these emoji (see [`discord::emoji::Translated::used`])
    /// and sticker: the client counts them once Discord took it
    /// (`EMOJI_TRACK_USAGE` and `STICKER_TRACK_USAGE`, 138399).
    pub(super) fn track_message_use(&self, emoji: &[String], sticker: Option<Snowflake>) {
        if emoji.is_empty() && sticker.is_none() {
            return;
        }
        let now = now_ms() as u64;
        {
            let stores = &mut self.account.state.borrow_mut().frecency.stores;
            for key in emoji {
                stores.emoji.track(key, now);
            }
            if let Some(sticker) = sticker {
                stores.stickers.track(&sticker.0.to_string(), now);
            }
        }
        self.persist_usage();
    }

    /// We reacted with the emoji `key` (see [`discord::emoji::reaction_key`]),
    /// which the client counts as an emoji and as a reaction
    /// (`MESSAGE_REACTION_ADD` when it is ours, 121681).
    pub(super) fn track_reaction_use(&self, key: &str) {
        let now = now_ms() as u64;
        {
            let stores = &mut self.account.state.borrow_mut().frecency.stores;
            stores.reactions.track(key, now);
            stores.emoji.track(key, now);
        }
        self.persist_usage();
    }

    /// A channel was opened: the channel, and its server if that is not the
    /// one counted last (`CHANNEL_SELECT`, 205761). Each is counted only
    /// when it differs from the one before.
    pub(super) fn track_channel_use(&self, channel: Snowflake) {
        let now = now_ms() as u64;
        {
            let mut state = self.account.state.borrow_mut();
            let guild = state.find_channel(channel).map(|(guild, _)| guild.map(|g| g.id));
            // A channel we can't place has no server to compare, and the client would count the channel.
            let guild = guild.unwrap_or(None);
            if state.last_counted_channel != Some(channel) {
                state.last_counted_channel = Some(channel);
                state.frecency.stores.channels.track(&channel.0.to_string(), now);
            }
            if state.last_counted_guild != guild {
                state.last_counted_guild = guild;
                if let Some(guild) = guild {
                    state.frecency.stores.channels.track(&guild.0.to_string(), now);
                }
            }
        }
        self.persist_usage();
    }

    /// The connection is open, or resumed (`POST_CONNECTION_OPEN`).
    pub(super) fn usage_connected(self: &Rc<Self>) {
        self.account.state.borrow_mut().usage_active = true;
        self.usage_look_after(first_wait(), true);
    }

    /// The connection closed: look now (`CONNECTION_CLOSED`), before the
    /// settings sync sets a save aside as an offline edit.
    pub(super) fn usage_closed(self: &Rc<Self>) {
        if !self.account.state.borrow().usage_active {
            return;
        }
        self.account.state.borrow_mut().usage_timer += 1;
        self.usage_look(false);
    }

    fn usage_look_after(self: &Rc<Self>, after: Duration, first: bool) {
        let generation = {
            let mut state = self.account.state.borrow_mut();
            state.usage_timer += 1;
            state.usage_timer
        };
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(after, move || {
            if let Some(this) = this.upgrade()
                && this.account.epoch.get() == epoch
                && this.account.state.borrow().usage_timer == generation
            {
                this.usage_look(first);
            }
        });
    }

    /// `R` in 416380: look again later, load the proto if something waits
    /// for it, then ask for a save. `first` is the look after connecting,
    /// which servers and channels alone don't make load the proto.
    fn usage_look(self: &Rc<Self>, first: bool) {
        let (wait, loaded) = {
            let mut state = self.account.state.borrow_mut();
            let wait = *state.usage_interval.get_or_insert_with(look_interval);
            (wait, state.frecency_sync.is_loaded())
        };
        self.usage_look_after(wait, false);
        let waiting = {
            let stores = &self.account.state.borrow().frecency.stores;
            stores.has_pending_beyond_channels() || (stores.has_pending() && !first)
        };
        if loaded {
            return self.usage_ask_for_save();
        }
        if waiting {
            let this = self.clone();
            self.load_frecency(move || this.usage_ask_for_save());
        }
    }

    /// `markDirtyIfHasPendingChange`: a save at once, if something is pending
    /// and the proto is loaded.
    fn usage_ask_for_save(self: &Rc<Self>) {
        let schedule = {
            let mut state = self.account.state.borrow_mut();
            if state.frecency.stores.has_pending() { state.frecency_sync.touch() } else { None }
        };
        if let Some(schedule) = schedule {
            self.save_frecency_after(schedule);
        }
    }

    /// Pending uses were saved: keep what is left.
    pub(super) fn usage_saved(&self) {
        self.persist_usage();
    }
}
