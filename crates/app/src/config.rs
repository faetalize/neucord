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

/// Bars and panels that let the content scrolling beneath them show through,
/// blurred (`--neucord-bar-opacity` and `--neucord-blur` in `style.css`).
pub const TRANSLUCENT: Key = Key { group: "Appearance", name: "translucent", default: true };

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
        assert!(!dir.file().exists(), "reading creates nothing");
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
