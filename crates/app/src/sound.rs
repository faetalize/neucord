//! Sounds of Neucord's own, taken from the desktop's sound theme: the
//! freedesktop sound theme spec's lookup, then a GStreamer `playbin`, so no
//! sound file ships with Neucord and the one played is the one the desktop's
//! theme has for the event. Silent where there is none.
//!
//! Notification sounds are the notification server's to play (see
//! `desktop.rs`); this is for what the client plays itself.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gst::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib};

/// The theme every desktop has: the spec ends its search there.
const FALLBACK_THEME: &str = "freedesktop";
/// The sound files the spec names, in its order: a `.disabled` file means the
/// theme wants the sound silent.
const EXTENSIONS: [&str; 4] = ["disabled", "oga", "ogg", "wav"];

/// Play the theme's sound for the event `name` (`complete`, say) at `volume`,
/// 1.0 being the file as it is. Nothing happens if the desktop has turned
/// event sounds off or its theme has none.
pub fn play(name: &'static str, volume: f64) {
    let settings = desktop_settings();
    if settings.as_ref().is_some_and(|s| !s.boolean("event-sounds")) {
        return;
    }
    let theme = settings.map(|s| s.string("theme-name").to_string());
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    glib::spawn_future_local(async move {
        // Reading the theme's files is disk work.
        let found = gio::spawn_blocking(move || find(name, theme.as_deref(), &dirs)).await;
        if let Ok(Some(path)) = found {
            start(&path, volume);
        }
    });
}

/// The GNOME sound settings, where the desktop has them: the theme it names
/// and whether event sounds are on. Other desktops go by the default theme.
fn desktop_settings() -> Option<gio::Settings> {
    const SCHEMA: &str = "org.gnome.desktop.sound";
    // `Settings::new` aborts on a schema that isn't installed.
    gio::SettingsSchemaSource::default()?.lookup(SCHEMA, true)?;
    Some(gio::Settings::new(SCHEMA))
}

/// The file for event `name` by the sound theme spec: in `theme`, then the
/// themes it inherits, then `freedesktop`; in each, the `stereo` profile of
/// every data directory, user's first. A `.disabled` file ends the search
/// with nothing.
fn find(name: &str, theme: Option<&str>, dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut themes: Vec<String> = Vec::new();
    for start in theme.filter(|t| !t.is_empty()).into_iter().chain([FALLBACK_THEME]) {
        collect(start, dirs, &mut themes);
    }
    for theme in &themes {
        for dir in dirs {
            let profile = dir.join("sounds").join(theme).join("stereo");
            for extension in EXTENSIONS {
                let file = profile.join(format!("{name}.{extension}"));
                if file.is_file() {
                    return (extension != "disabled").then_some(file);
                }
            }
        }
    }
    None
}

/// `theme` and, after it, the themes it inherits, once each.
fn collect(theme: &str, dirs: &[PathBuf], themes: &mut Vec<String>) {
    // A theme name is a directory name: nothing that leaves its directory.
    if themes.iter().any(|t| t == theme) || theme.contains(['/', '\\']) || theme.starts_with('.') {
        return;
    }
    themes.push(theme.to_owned());
    for dir in dirs {
        let Ok(index) = std::fs::read_to_string(dir.join("sounds").join(theme).join("index.theme")) else { continue };
        for parent in inherits(&index) {
            collect(&parent, dirs, themes);
        }
        break;
    }
}

/// The `Inherits` of a theme's `index.theme`.
fn inherits(index: &str) -> Vec<String> {
    index
        .lines()
        .find_map(|line| line.trim().strip_prefix("Inherits="))
        .map(|list| list.split(',').map(|name| name.trim().to_owned()).filter(|name| !name.is_empty()).collect())
        .unwrap_or_default()
}

/// Play `path` once. The pipeline lives until it ends or fails, then goes.
fn start(path: &Path, volume: f64) {
    if gst::init().is_err() {
        return;
    }
    let uri = gio::File::for_path(path).uri();
    let Ok(playbin) =
        gst::ElementFactory::make("playbin").property("uri", uri.as_str()).property("volume", volume).build()
    else {
        return;
    };
    let Some(bus) = playbin.bus() else { return };
    // The watch is dropped from outside its own callback, once it has seen the end.
    let watch: Rc<RefCell<Option<gst::bus::BusWatchGuard>>> = Rc::default();
    let guard = bus.add_watch_local({
        let (playbin, watch) = (playbin.clone(), watch.clone());
        move |_, message| {
            if matches!(message.view(), gst::MessageView::Eos(_) | gst::MessageView::Error(_)) {
                let _ = playbin.set_state(gst::State::Null);
                let watch = watch.clone();
                glib::idle_add_local_once(move || drop(watch.take()));
            }
            glib::ControlFlow::Continue
        }
    });
    let Ok(guard) = guard else { return };
    *watch.borrow_mut() = Some(guard);
    if playbin.set_state(gst::State::Playing).is_err() {
        let _ = playbin.set_state(gst::State::Null);
        drop(watch.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Home {
            let dir = std::env::temp_dir().join(format!("neucord-sound-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Home(dir)
        }

        /// A sound file of `theme` under data directory `dir`, as `<dir>/sounds/<theme>/stereo/<file>`.
        fn put(&self, dir: &str, theme: &str, file: &str) -> PathBuf {
            let path = self.0.join(dir).join("sounds").join(theme).join("stereo").join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
            path
        }

        fn index(&self, dir: &str, theme: &str, text: &str) {
            let path = self.0.join(dir).join("sounds").join(theme).join("index.theme");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn dirs(&self, names: &[&str]) -> Vec<PathBuf> {
            names.iter().map(|name| self.0.join(name)).collect()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_desktops_theme_comes_before_freedesktop() {
        let home = Home::new("theme");
        let wanted = home.put("share", "mine", "complete.oga");
        home.put("share", "freedesktop", "complete.oga");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["share"])), Some(wanted));
    }

    #[test]
    fn a_theme_without_the_sound_falls_back_to_freedesktop() {
        let home = Home::new("fallback");
        home.put("share", "mine", "other.oga");
        let standard = home.put("share", "freedesktop", "complete.oga");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["share"])), Some(standard.clone()));
        assert_eq!(find("complete", None, &home.dirs(&["share"])), Some(standard), "no theme named");
    }

    #[test]
    fn an_inherited_theme_comes_before_freedesktop() {
        let home = Home::new("inherits");
        home.index("share", "mine", "[Sound Theme]\nName=Mine\nInherits=parent, other\n");
        let parent = home.put("share", "parent", "complete.ogg");
        home.put("share", "freedesktop", "complete.oga");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["share"])), Some(parent));
    }

    #[test]
    fn inheriting_in_a_circle_ends() {
        let home = Home::new("circle");
        home.index("share", "a", "Inherits=b\n");
        home.index("share", "b", "Inherits=a\n");
        assert_eq!(find("complete", Some("a"), &home.dirs(&["share"])), None);
    }

    #[test]
    fn the_users_directory_comes_before_the_systems() {
        let home = Home::new("dirs");
        let theirs = home.put("user", "freedesktop", "complete.oga");
        home.put("system", "freedesktop", "complete.oga");
        assert_eq!(find("complete", None, &home.dirs(&["user", "system"])), Some(theirs));
        let only = home.put("system", "mine", "complete.wav");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["user", "system"])), Some(only));
    }

    #[test]
    fn a_disabled_file_is_a_theme_wanting_silence() {
        let home = Home::new("disabled");
        home.put("share", "mine", "complete.disabled");
        home.put("share", "freedesktop", "complete.oga");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["share"])), None);
    }

    #[test]
    fn no_file_is_no_sound() {
        let home = Home::new("none");
        assert_eq!(find("complete", Some("mine"), &home.dirs(&["share"])), None);
    }

    #[test]
    fn a_theme_name_cant_leave_its_directory() {
        let home = Home::new("escape");
        // `share/sounds/../../stereo` is this one.
        let outside = home.0.join("stereo");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("complete.oga"), b"x").unwrap();
        std::fs::create_dir_all(home.0.join("share/sounds")).unwrap();
        assert_eq!(find("complete", Some("../.."), &home.dirs(&["share"])), None);
    }
}
