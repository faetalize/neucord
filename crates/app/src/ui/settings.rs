//! The settings window: the account's settings Neucord honours, and the
//! account-wide toggles beside them, each saved to Discord as it changes
//! (see [`discord::settings_sync`]). The official client's User Settings has
//! far more; this is what the protobuf settings carry that a chat window
//! uses.
//!
//! Neucord's own options are the last page. They aren't the account's, so they
//! save to this computer ([`crate::config`]) and nothing is sent to Discord.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use discord::Setting;
use discord::locales::{self, LOCALES};
use discord::rest::Rest;
use discord::settings::{self, ReactionNotifications, Settings, SettingsPatch, StickerAnimation};
use discord::settings_sync::{Save, SaveError, Schedule};
use gtk::glib;

use super::App;
use crate::config::{self, Key};

/// Sets a row to the settings.
pub(super) type Show = Box<dyn Fn(&Settings)>;

/// The open window. Settings that change elsewhere (another client, or the
/// answer to a save) show in it through `show`.
pub(super) struct Window {
    dialog: adw::PreferencesDialog,
    show: Show,
}

/// The rows of the window being built, and what sets each from the settings.
pub(super) struct Form {
    pub(super) app: Weak<App>,
    /// Rows are set from the settings, which must not save them again.
    pub(super) updating: Rc<Cell<bool>>,
    pub(super) shows: Vec<Show>,
}

impl Form {
    fn switch(
        &mut self,
        group: &adw::PreferencesGroup,
        title: &str,
        subtitle: &str,
        get: fn(&Settings) -> bool,
        set: fn(bool) -> Setting,
    ) {
        let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).build();
        let (app, updating) = (self.app.clone(), self.updating.clone());
        row.connect_active_notify(move |row| {
            if let (false, Some(app)) = (updating.get(), app.upgrade()) {
                app.change_setting(set(row.is_active()));
            }
        });
        group.add(&row);
        self.shows.push(Box::new(move |settings| row.set_active(get(settings))));
    }

    /// A row choosing among `options`, by their position.
    fn choice(
        &mut self,
        group: &adw::PreferencesGroup,
        title: &str,
        options: &[&str],
        get: fn(&Settings) -> u32,
        set: fn(u32) -> Option<Setting>,
    ) {
        let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(options)).build();
        let (app, updating) = (self.app.clone(), self.updating.clone());
        row.connect_selected_notify(move |row| {
            if let (false, Some(app), Some(setting)) = (updating.get(), app.upgrade(), set(row.selected())) {
                app.change_setting(setting);
            }
        });
        group.add(&row);
        self.shows.push(Box::new(move |settings| row.set_selected(get(settings))));
    }
}

/// A switch for one of Neucord's own options. It reads the option once, as
/// nothing else changes it while the window is open.
fn option_switch(
    app: &Rc<App>,
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    key: &'static Key,
    changed: fn(&Rc<App>, bool),
) {
    let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(app.config.get(key)).build();
    let app = Rc::downgrade(app);
    row.connect_active_notify(move |row| {
        let Some(app) = app.upgrade() else { return };
        match app.config.set(key, row.is_active()) {
            Ok(()) => changed(&app, row.is_active()),
            Err(e) => {
                eprintln!("config: {e}");
                app.settings_toast("Could not save that option");
            }
        }
    });
    group.add(&row);
}

pub(super) fn group(title: &str, description: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).description(description).build()
}

fn page(title: &str, icon: &str, groups: &[&adw::PreferencesGroup]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(title).icon_name(icon).build();
    for group in groups {
        page.add(*group);
    }
    page
}

impl App {
    pub(super) fn show_settings(self: &Rc<Self>) {
        if let Some(open) = self.settings_window.borrow().as_ref() {
            return open.dialog.present(Some(&self.window));
        }
        let window = self.build_settings();
        window.dialog.present(Some(&self.window));
        *self.settings_window.borrow_mut() = Some(window);
    }

    fn build_settings(self: &Rc<Self>) -> Window {
        let mut form = Form { app: Rc::downgrade(self), updating: Rc::new(Cell::new(false)), shows: Vec::new() };

        let messages = group("Messages", "");
        form.switch(
            &messages,
            "Compact messages",
            "Kept for your other clients; Neucord draws one layout.",
            |s| s.message_display_compact,
            Setting::MessageDisplayCompact,
        );
        form.switch(&messages, "Show embeds and link previews", "", |s| s.media.render_embeds, Setting::RenderEmbeds);
        form.switch(&messages, "Show emoji reactions", "", |s| s.media.render_reactions, Setting::RenderReactions);
        let inline = group("Show images and videos", "");
        form.switch(
            &inline,
            "When posted as links to chat",
            "",
            |s| s.media.inline_embed_media,
            Setting::InlineEmbedMedia,
        );
        form.switch(
            &inline,
            "When uploaded directly to Discord",
            "",
            |s| s.media.inline_attachment_media,
            Setting::InlineAttachmentMedia,
        );
        let animation = group("Animation", "");
        form.switch(&animation, "Play GIFs automatically", "", |s| s.media.gif_auto_play, Setting::GifAutoPlay);
        form.switch(&animation, "Play animated emoji", "", |s| s.media.animate_emoji, Setting::AnimateEmoji);
        form.choice(
            &animation,
            "Play sticker animations",
            &["Always animate", "Animate on interaction", "Never animate"],
            |s| s.media.animate_stickers.raw() as u32,
            |n| StickerAnimation::from_raw(n.into()).map(Setting::AnimateStickers),
        );
        let developer = group("Developer", "");
        form.switch(
            &developer,
            "Developer Mode",
            "Menus offer to copy IDs.",
            |s| s.developer_mode,
            Setting::DeveloperMode,
        );
        let chat = page("Chat", "chat-message-new-symbolic", &[&messages, &inline, &animation, &developer]);

        let notify = group("Notify me when…", "Saved to your account, for every device you use.");
        form.choice(
            &notify,
            "Someone reacts to my messages",
            &["All Messages", "Only Direct Messages", "Never"],
            |s| s.notifications.reactions.raw() as u32,
            |n| ReactionNotifications::from_raw(n.into()).map(Setting::ReactionNotifications),
        );
        form.switch(
            &notify,
            "A friend and I reach a friendship anniversary",
            "",
            |s| s.notifications.friend_anniversary,
            Setting::FriendAnniversaryNotifications,
        );
        form.switch(
            &notify,
            "Friends come online",
            "",
            |s| s.notifications.friend_online,
            Setting::FriendOnlineNotifications,
        );
        form.switch(
            &notify,
            "Friends update their profile",
            "",
            |s| s.notifications.profile_updates,
            Setting::ProfileUpdatesNotifications,
        );
        form.switch(
            &notify,
            "People I know start streaming in small servers",
            "",
            |s| s.notifications.go_live,
            Setting::GoLiveNotifications,
        );
        // Only for the accounts Discord is trying it on.
        let upcoming = {
            let state = self.account.state.borrow();
            state.me.as_ref().is_some_and(|me| settings::upcoming_event_toggle(&state.experiments, me.id))
        };
        if upcoming {
            form.switch(
                &notify,
                "A server has an upcoming event",
                "",
                |s| s.notifications.upcoming_event,
                Setting::UpcomingEventNotifications,
            );
        }
        let notifications = page("Notifications", "preferences-system-notifications-symbolic", &[&notify]);

        let joining = group("Game joining", "Neucord shares no games, so this is for your other clients.");
        form.switch(
            &joining,
            "Allow friends to join my game",
            "",
            |s| s.privacy.friends_join_game,
            Setting::FriendsJoinGame,
        );
        form.switch(
            &joining,
            "Allow voice channel participants to join my game",
            "",
            |s| s.privacy.voice_participants_join_game,
            Setting::VoiceParticipantsJoinGame,
        );
        let [messaging, requests, profile] = self.privacy_groups(&mut form);
        let privacy = page("Privacy", "channel-secure-symbolic", &[&messaging, &requests, &profile, &joining]);

        let language = group("Language", "Neucord shows English whatever this is; your other clients follow it.");
        form.choice(
            &language,
            "Language",
            &LOCALES.map(|(_, name)| name),
            // An account that never picked one has the default, which is English.
            |s| {
                locales::position(s.locale.as_deref().unwrap_or("en-US"))
                    .map_or(gtk::INVALID_LIST_POSITION, |n| n as u32)
            },
            |n| LOCALES.get(n as usize).map(|(code, _)| Setting::Locale((*code).to_owned())),
        );
        let language = page("Language", "preferences-desktop-locale-symbolic", &[&language]);

        let typing = group("Typing", "");
        option_switch(
            self,
            &typing,
            "Let others see when I'm typing",
            "You still see who else is typing.",
            &config::SEND_TYPING,
            |app, sending| app.account.state.borrow_mut().typing.set_sending(sending),
        );
        let appearance = group(
            "Appearance",
            &format!(
                "Colours follow your system's light or dark preference and accent colour. \
                 Put your own stylesheet in {}.",
                glib::markup_escape_text(&config::stylesheet_path().display().to_string())
            ),
        );
        option_switch(
            self,
            &appearance,
            "Translucent bars and panels",
            "Let the chat show through, blurred, behind the headers, composer and sliding sidebar.",
            &config::TRANSLUCENT,
            |app, on| app.theme.set_translucent(on),
        );
        let background = group("Background", "");
        option_switch(
            self,
            &background,
            "Minimize Neucord to the system tray",
            "Clicking X minimizes Neucord instead of closing it. Needs a panel with tray icons (on GNOME, the \
             AppIndicator extension); without one, closing quits.",
            &config::MINIMIZE_TO_TRAY,
            |app, _| app.sync_background(false),
        );
        option_switch(
            self,
            &background,
            "Open Neucord when your computer starts up",
            "It starts hidden, behind the tray icon.",
            &config::OPEN_ON_STARTUP,
            |app, _| app.sync_background(false),
        );
        option_switch(
            self,
            &background,
            "Show unread messages on the tray icon",
            "Adds a badge when you have unread messages or mentions.",
            &config::UNREAD_BADGE,
            |app, _| app.refresh_tray(),
        );
        let neucord = page("Neucord", "preferences-system-symbolic", &[&appearance, &typing, &background]);
        neucord.set_description("These are saved on this computer only, not to your Discord account.");

        let dialog = adw::PreferencesDialog::builder().title("Settings").build();
        dialog.add(&chat);
        dialog.add(&notifications);
        dialog.add(&privacy);
        dialog.add(&language);
        dialog.add(&neucord);
        dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.settings_window.take();
                }
            }
        });

        let Form { updating, shows, .. } = form;
        let show = move |settings: &Settings| {
            updating.set(true);
            shows.iter().for_each(|show| show(settings));
            updating.set(false);
        };
        show(&self.account.state.borrow().settings);
        Window { dialog, show: Box::new(show) }
    }

    /// What a question about a setting is put to: the window, which covers
    /// the app's own.
    pub(super) fn settings_parent(&self) -> gtk::Widget {
        match self.settings_window.borrow().as_ref() {
            Some(window) => window.dialog.clone().upcast(),
            None => self.window.clone().upcast(),
        }
    }

    /// Say how a save went where it's seen: the window covers the app's own
    /// toasts, so it shows its own.
    pub(super) fn settings_toast(&self, text: &str) {
        match self.settings_window.borrow().as_ref() {
            Some(window) => window.dialog.add_toast(adw::Toast::new(text)),
            None => self.toast(text),
        }
    }

    /// Show the settings in the window, if it's open.
    pub(super) fn settings_replaced(&self) {
        if let Some(window) = self.settings_window.borrow().as_ref() {
            (window.show)(&self.account.state.borrow().settings);
        }
    }

    /// Settings that changed, here or elsewhere, take effect where they show.
    pub(super) fn apply_settings(self: &Rc<Self>, patch: SettingsPatch, replace: bool) {
        let (media, folders, status) = {
            let mut state = self.account.state.borrow_mut();
            let before = (state.settings.media, state.settings.guild_folders.clone(), state.settings.status.clone());
            if replace {
                state.settings.replace(patch);
            } else {
                state.settings.apply(patch);
            }
            (
                before.0 != state.settings.media,
                before.1 != state.settings.guild_folders,
                before.2 != state.settings.status,
            )
        };
        if folders {
            self.rebuild_rail();
        }
        if media {
            self.images.motion().set_media(self.account.state.borrow().settings.media);
            self.refresh_rows();
        }
        if status {
            self.refresh_status();
            self.report_presence();
            self.manage_expiries();
        }
        self.settings_replaced();
    }

    /// The user changed a setting in the window.
    pub(super) fn change_setting(self: &Rc<Self>, setting: Setting) {
        let edited = self.account.state.borrow_mut().sync.edit(setting);
        let Some(edited) = edited else {
            // Before READY there's nothing to change.
            self.settings_replaced();
            return self.settings_toast("Settings aren't loaded yet");
        };
        self.apply_settings(edited.patch, false);
        if let Some(schedule) = edited.schedule {
            self.save_settings_after(schedule);
        }
    }

    /// Save when `schedule` says. A timer replaced since finds the save
    /// gone and does nothing, and so does one of an account that was left.
    pub(super) fn save_settings_after(self: &Rc<Self>, schedule: Schedule) {
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(schedule.after, move || {
            if let Some(this) = this.upgrade().filter(|this| this.account.epoch.get() == epoch) {
                this.save_settings(schedule.generation);
            }
        });
    }

    fn save_settings(self: &Rc<Self>, generation: u64) {
        let (save, rest) = {
            let mut state = self.account.state.borrow_mut();
            (state.sync.save(generation), state.rest.clone())
        };
        let (Some(save), Some(rest)) = (save, rest) else { return };
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            let answer = rest.save_settings(&save).await;
            // The answer is of an account that was left meanwhile, whose
            // sync state is gone: what it says is no one's.
            if this.account.epoch.get() == epoch {
                this.settings_answered(&rest, &save, answer, epoch).await;
            }
        });
    }

    async fn settings_answered(
        self: &Rc<Self>,
        rest: &Rest,
        save: &Save,
        answer: Result<discord::settings_sync::Saved, SaveError>,
        epoch: u64,
    ) {
        match answer {
            Ok(saved) => {
                let applied = self.account.state.borrow_mut().sync.saved(save, &saved);
                if let Some(applied) = applied {
                    self.apply_settings(applied.patch, true);
                    if let Some(schedule) = applied.schedule {
                        self.save_settings_after(schedule);
                    }
                }
            }
            Err(SaveError::RateLimited { retry_after }) => {
                let retry = self.account.state.borrow_mut().sync.rate_limited(retry_after.as_deref());
                self.save_settings_after(retry);
            }
            Err(SaveError::Invalid) => {
                // The client asks for the settings again and starts over.
                self.settings_toast("Discord refused those settings. Loading them again");
                let reloaded = rest.settings_proto(1).await;
                if self.account.epoch.get() != epoch {
                    return;
                }
                match reloaded {
                    Ok(proto) => {
                        let patch = self.account.state.borrow_mut().sync.reloaded(proto);
                        if let Some(patch) = patch {
                            self.apply_settings(patch, true);
                        }
                    }
                    Err(e) => eprintln!("settings: {e}"),
                }
            }
            Err(SaveError::Other(e)) => {
                eprintln!("settings: {e}");
                self.account.state.borrow_mut().sync.failed();
                self.settings_toast("Could not save your settings");
            }
        }
    }
}
