//! Neucord's own options and what it remembers between runs, in two small
//! key files (the INI format GSettings' key-file backend and desktop files
//! use).
//!
//! `config.ini` under `$XDG_CONFIG_HOME/neucord/` holds the options the user
//! sets in the settings window. It is meant to be edited by hand too, next to
//! the user stylesheet (`style.css`, see `theme`) that also lives there, so a
//! save rewrites only the keys it owns and leaves comments and keys it doesn't
//! know. What the app writes on its own, such as whether the member pane is
//! open, goes to `state.ini` under `$XDG_STATE_HOME/neucord/`, so a
//! hand-edited config isn't rewritten every time a pane is toggled.
//!
//! A key file instead of GSettings: that needs a compiled schema installed
//! system-wide, which packaging provides (#26) and `cargo run` doesn't.
//! Another crate for TOML or JSON would buy nothing GLib's key files lack,
//! and JSON has no comments. Nothing here is account data: what Discord
//! stores for the account is saved to Discord (see `ui::settings`).

use std::path::PathBuf;

use gtk::glib::{self, KeyFile, KeyFileFlags};

/// A boolean option or remembered value: where it is in its file, and what
/// it is until the file says otherwise. A new one is a constant here, not a
/// new file format.
pub struct Key {
    group: &'static str,
    name: &'static str,
    default: bool,
}

/// Whether others see us typing (`POST /channels/{id}/typing`).
pub const SEND_TYPING: Key = Key { group: "Chat", name: "send-typing", default: true };

/// A send button at the end of the message box (`submitButtonEnabled` in
/// the client's `AccessibilityStore`, off unless chosen).
pub const SEND_BUTTON: Key = Key { group: "Chat", name: "send-button", default: false };

/// The message box's Apps button (`appsButtonEnabled` in the client's
/// `AccessibilityStore`, on unless chosen), which, like the three below,
/// counts only for accounts in `2026-06-chat-bar-customization`.
pub const APPS_BUTTON: Key = Key { group: "Chat", name: "apps-button", default: true };

/// The emoji button (`emojiButtonEnabled`, on).
pub const EMOJI_BUTTON: Key = Key { group: "Chat", name: "emoji-button", default: true };

/// The GIF button (`gifButtonEnabled`, on).
pub const GIF_BUTTON: Key = Key { group: "Chat", name: "gif-button", default: true };

/// The sticker button (`stickerButtonEnabled`, on).
pub const STICKER_BUTTON: Key = Key { group: "Chat", name: "sticker-button", default: true };

/// Spelling is checked in the message box (`SpellcheckStore.enabled`, on
/// unless turned off).
pub const SPELLCHECK: Key = Key { group: "Chat", name: "spellcheck", default: true };

/// Bars and panels that let the content scrolling beneath them show through,
/// blurred (`--neucord-bar-opacity` and `--neucord-blur` in `style.css`).
pub const TRANSLUCENT: Key = Key { group: "Appearance", name: "translucent", default: true };

/// Which colours the window uses: what the desktop prefers (`System`, as the
/// client's "Sync with computer"), or Light or Dark whatever it prefers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

impl ColorScheme {
    pub const ALL: [ColorScheme; 3] = [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark];

    /// What the options file calls it.
    pub fn name(self) -> &'static str {
        match self {
            ColorScheme::System => "system",
            ColorScheme::Light => "light",
            ColorScheme::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorScheme::System => "System",
            ColorScheme::Light => "Light",
            ColorScheme::Dark => "Dark",
        }
    }

    /// A name that isn't one of ours is the default, as a missing one is.
    pub fn from_name(name: &str) -> ColorScheme {
        Self::ALL.into_iter().find(|scheme| scheme.name() == name).unwrap_or_default()
    }
}

/// Sounds at all: the client's "Disable All Notification Sounds"
/// (`disableAllSounds`, 803224), turned round so that on is the default. Off
/// keeps the preferences below, which come back when it is on. It alone
/// governs the sound of a voice message recording stopped by another
/// (`voice_message_recording_stopped`), which the client lists nowhere else.
pub const ALL_SOUNDS: Key = Key { group: "Notifications", name: "all-sounds", default: true };

/// The sound of a new message (`message1` in 212909's list of sounds, the
/// one this plays; the voice ones have nothing to play yet).
pub const MESSAGE_SOUND: Key = Key { group: "Notifications", name: "message-sound", default: true };

/// The inbox's mentions include @everyone and @here (`everyoneFilter` in
/// `recentMentionFilterSettings`, 849171; the client's default is on).
pub const INBOX_EVERYONE: Key = Key { group: "Inbox", name: "everyone", default: true };

/// ... and @role mentions (`roleFilter`).
pub const INBOX_ROLES: Key = Key { group: "Inbox", name: "roles", default: true };

/// ... or only the open server's (`guildFilter` THIS_SERVER; default all).
pub const INBOX_THIS_SERVER: Key = Key { group: "Inbox", name: "this-server", default: false };

/// Closing the window leaves Neucord running behind its tray icon, as the
/// client's "Minimize Discord to System Tray" does (on, like there).
pub const MINIMIZE_TO_TRAY: Key = Key { group: "Background", name: "minimize-to-tray", default: true };

/// Neucord starts with the user's session. The client defaults this on,
/// because its installer sets it up; Neucord has none yet (#26), and an
/// autostart entry written on the first run would point at wherever the
/// binary happened to be, such as a `cargo run` build.
pub const OPEN_ON_STARTUP: Key = Key { group: "Background", name: "open-on-startup", default: false };

/// Unread messages show on the tray icon, and as a dot on the app icon and
/// in the title (the client's "Enable Unread Message Badge", on by default).
pub const UNREAD_BADGE: Key = Key { group: "Background", name: "unread-badge", default: true };

/// The member pane, docked beside the chat, was closed by the user.
pub const MEMBERS_CLOSED: Key = Key { group: "Window", name: "members-closed", default: false };

/// The user's options.
pub fn config_path() -> PathBuf {
    glib::user_config_dir().join("neucord").join("config.ini")
}

/// The user's stylesheet, loaded after Neucord's own.
pub fn stylesheet_path() -> PathBuf {
    glib::user_config_dir().join("neucord").join("style.css")
}

/// What the app remembers.
pub fn state_path() -> PathBuf {
    glib::user_state_dir().join("neucord").join("state.ini")
}

/// One key file, read once and written whenever a value changes.
pub struct Store {
    path: PathBuf,
    file: KeyFile,
}

impl Store {
    /// Read `path`. A missing file is an empty one. A file that doesn't parse
    /// is moved aside to `<name>.invalid`, which keeps the user's edits
    /// where they can fix them, and the next save starts a fresh file.
    pub fn open(path: PathBuf) -> Store {
        let file = KeyFile::new();
        // Comments stay so that saving doesn't strip a hand-edited file.
        let flags = KeyFileFlags::KEEP_COMMENTS | KeyFileFlags::KEEP_TRANSLATIONS;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if let Err(e) = file.load_from_data(&text, flags) {
                    let aside = path.with_extension("ini.invalid");
                    eprintln!("config: {} is not valid ({e}); moved to {}", path.display(), aside.display());
                    if std::fs::rename(&path, &aside).is_err() {
                        eprintln!("config: could not move {} aside", path.display());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("config: could not read {}: {e}", path.display()),
        }
        Store { path, file }
    }

    /// The value of `key`; the default for a key the file lacks or holds a
    /// non-boolean in.
    pub fn get(&self, key: &Key) -> bool {
        self.file.boolean(key.group, key.name).unwrap_or(key.default)
    }

    /// Set `key` and write the file. A value already stored is not written
    /// again.
    pub fn set(&self, key: &Key, value: bool) -> Result<(), glib::Error> {
        if self.file.boolean(key.group, key.name).is_ok_and(|stored| stored == value) {
            return Ok(());
        }
        self.file.set_boolean(key.group, key.name, value);
        self.save()
    }

    /// The numbers under `group`, by key: what a map keeps, as ids and times.
    pub fn numbers(&self, group: &str) -> Vec<(String, u64)> {
        let Ok(keys) = self.file.keys(group) else { return Vec::new() };
        keys.iter()
            .filter_map(|key| {
                let value = self.file.int64(group, key).ok()?;
                Some((key.to_string(), u64::try_from(value).ok()?))
            })
            .collect()
    }

    /// Make `group` exactly `entries`, and write the file if that changed it.
    pub fn set_numbers(&self, group: &str, entries: &[(String, u64)]) -> Result<(), glib::Error> {
        let mut before = self.numbers(group);
        before.sort();
        let mut after = entries.to_vec();
        after.sort();
        if before == after {
            return Ok(());
        }
        let _ = self.file.remove_group(group);
        for (key, value) in entries {
            self.file.set_int64(group, key, i64::try_from(*value).unwrap_or(i64::MAX));
        }
        self.save()
    }

    /// The colour scheme chosen, [`ColorScheme::System`] if none was.
    pub fn color_scheme(&self) -> ColorScheme {
        self.file
            .string("Appearance", "color-scheme")
            .map_or_else(|_| ColorScheme::default(), |name| ColorScheme::from_name(&name))
    }

    pub fn set_color_scheme(&self, scheme: ColorScheme) -> Result<(), glib::Error> {
        if self.color_scheme() == scheme && self.file.has_key("Appearance", "color-scheme").unwrap_or(false) {
            return Ok(());
        }
        self.file.set_string("Appearance", "color-scheme", scheme.name());
        self.save()
    }

    fn save(&self) -> Result<(), glib::Error> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| glib::Error::new(glib::FileError::Failed, &e.to_string()))?;
        }
        self.file.save_to_file(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("neucord-config-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Dir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("neucord").join("config.ini")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = Dir::new("missing");
        let store = Store::open(dir.file());
        assert!(store.get(&SEND_TYPING));
        assert!(store.get(&TRANSLUCENT));
        assert!(!store.get(&MEMBERS_CLOSED));
        assert!(store.get(&MINIMIZE_TO_TRAY), "as the client has it");
        assert!(store.get(&UNREAD_BADGE), "as the client has it");
        assert!(!store.get(&OPEN_ON_STARTUP), "nothing is written to the session until asked");
        assert!(!store.get(&SEND_BUTTON), "as the client has it");
        assert!(store.get(&SPELLCHECK), "as the client has it");
        for key in [&APPS_BUTTON, &EMOJI_BUTTON, &GIF_BUTTON, &STICKER_BUTTON] {
            assert!(store.get(key), "{}: as the client has it", key.name);
        }
        assert!(!dir.file().exists(), "reading creates nothing");
    }

    #[test]
    fn numbers_under_a_group_are_replaced_as_a_whole() {
        let dir = Dir::new("numbers");
        let store = Store::open(dir.file());
        assert!(store.numbers("Viewed").is_empty());
        store.set_numbers("Viewed", &[("1".into(), 10), ("2".into(), 20)]).unwrap();
        store.set_numbers("Viewed", &[("2".into(), 20), ("3".into(), 30)]).unwrap();
        let mut stored = Store::open(dir.file()).numbers("Viewed");
        stored.sort();
        assert_eq!(stored, [("2".to_owned(), 20), ("3".to_owned(), 30)]);
    }

    #[test]
    fn the_colour_scheme_follows_the_system_until_chosen() {
        let dir = Dir::new("scheme");
        let store = Store::open(dir.file());
        assert_eq!(store.color_scheme(), ColorScheme::System);
        store.set_color_scheme(ColorScheme::Dark).unwrap();
        assert_eq!(Store::open(dir.file()).color_scheme(), ColorScheme::Dark);
        store.set_color_scheme(ColorScheme::System).unwrap();
        assert_eq!(Store::open(dir.file()).color_scheme(), ColorScheme::System);
        assert_eq!(ColorScheme::from_name("sepia"), ColorScheme::System, "an unknown name is the default");
        for scheme in ColorScheme::ALL {
            assert_eq!(ColorScheme::from_name(scheme.name()), scheme);
        }
    }

    #[test]
    fn sounds_are_on_until_turned_off() {
        let dir = Dir::new("sounds");
        let store = Store::open(dir.file());
        assert!(store.get(&ALL_SOUNDS) && store.get(&MESSAGE_SOUND));
        store.set(&MESSAGE_SOUND, false).unwrap();
        let again = Store::open(dir.file());
        assert!(!again.get(&MESSAGE_SOUND) && again.get(&ALL_SOUNDS), "one by one");
    }

    #[test]
    fn a_value_set_is_there_after_reopening() {
        let dir = Dir::new("roundtrip");
        let store = Store::open(dir.file());
        store.set(&SEND_TYPING, false).unwrap();
        assert!(!store.get(&SEND_TYPING));
        assert!(!Store::open(dir.file()).get(&SEND_TYPING), "the directory was made and the file written");
        store.set(&SEND_TYPING, true).unwrap();
        assert!(Store::open(dir.file()).get(&SEND_TYPING));
    }

    #[test]
    fn saving_keeps_comments_and_keys_it_does_not_own() {
        let dir = Dir::new("keeps");
        std::fs::create_dir_all(dir.0.join("neucord")).unwrap();
        std::fs::write(dir.file(), "# mine\n[Chat]\nsend-typing=true\nfuture-option=yes\n\n[Elsewhere]\nx=1\n")
            .unwrap();
        let store = Store::open(dir.file());
        store.set(&SEND_TYPING, false).unwrap();
        let text = std::fs::read_to_string(dir.file()).unwrap();
        assert!(text.contains("# mine"), "{text}");
        assert!(text.contains("future-option=yes"), "{text}");
        assert!(text.contains("[Elsewhere]\nx=1"), "{text}");
        assert!(text.contains("send-typing=false"), "{text}");
    }

    #[test]
    fn a_value_that_is_not_a_boolean_reads_as_the_default() {
        let dir = Dir::new("not-boolean");
        std::fs::create_dir_all(dir.0.join("neucord")).unwrap();
        std::fs::write(dir.file(), "[Chat]\nsend-typing=maybe\n").unwrap();
        assert!(Store::open(dir.file()).get(&SEND_TYPING));
    }

    #[test]
    fn a_file_that_does_not_parse_is_kept_aside_and_replaced() {
        let dir = Dir::new("invalid");
        std::fs::create_dir_all(dir.0.join("neucord")).unwrap();
        std::fs::write(dir.file(), "this is not a key file\n[Chat\n").unwrap();
        let store = Store::open(dir.file());
        assert!(store.get(&SEND_TYPING), "the defaults");
        assert!(!dir.file().exists());
        assert_eq!(
            std::fs::read_to_string(dir.file().with_extension("ini.invalid")).unwrap(),
            "this is not a key file\n[Chat\n"
        );
        store.set(&SEND_TYPING, false).unwrap();
        assert!(!Store::open(dir.file()).get(&SEND_TYPING));
    }

    #[test]
    fn a_value_already_stored_is_not_written_again() {
        let dir = Dir::new("unchanged");
        let store = Store::open(dir.file());
        store.set(&MEMBERS_CLOSED, true).unwrap();
        std::fs::write(dir.file(), "[Window]\nmembers-closed=true\n# edited\n").unwrap();
        store.set(&MEMBERS_CLOSED, true).unwrap();
        assert!(std::fs::read_to_string(dir.file()).unwrap().contains("# edited"));
    }
}
