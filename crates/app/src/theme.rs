//! Neucord's stylesheets, in the order GTK weighs them.
//!
//! - `style.css`, ours, at application priority. Its colours are
//!   `--neucord-*` variables on `:root`, defaulting to libadwaita's colours
//!   (so dark mode and the accent colour, which libadwaita's `StyleManager`
//!   takes from `org.freedesktop.appearance`, reach them) and to shades of
//!   the text colour.
//! - The translucency switch, a few variables more, just after it.
//! - `style.css` from the user's config directory, at user priority, so its
//!   rules beat ours whatever their specificity. It is where a theme lives:
//!   it needs only to set the variables, and may style anything else too.
//!   Saving the file applies it, without a restart.
//!
//! The user stylesheet is separate from `~/.config/gtk-4.0/gtk.css`, which
//! GTK loads for every GTK app and which Neucord still honours.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio::{self, FileMonitorEvent, FileMonitorFlags, prelude::*};
use gtk::{CssProvider, gdk, glib};

use crate::config::{self, Store};

/// What a save does to the file: a write is only done once its
/// `ChangesDoneHint` arrives, and an editor that replaces the file sends a
/// create or a rename instead.
const SETTLE: Duration = Duration::from_millis(100);

/// What the translucency switch sets when off: opaque bars and no blur. A
/// user stylesheet that sets the same variables still wins.
const OPAQUE: &str = ":root { --neucord-bar-opacity: 1; --neucord-blur: 0px; }";

/// The stylesheets of the running app. Keeping it alive keeps the file watched.
pub struct Theme {
    translucency: CssProvider,
    _watch: Option<Watch>,
}

/// The user stylesheet, and the monitor that reloads it.
struct Watch {
    _monitor: gio::FileMonitor,
}

impl Theme {
    /// Install the stylesheets on the default display.
    pub fn install(config: &Store) -> Theme {
        let display = gdk::Display::default().expect("a display");

        let base = CssProvider::new();
        base.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(&display, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let translucency = CssProvider::new();
        // Added after `base`, which is what lets it override the same variables.
        gtk::style_context_add_provider_for_display(&display, &translucency, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        let theme = Theme { translucency, _watch: watch_user_stylesheet(&display, config::stylesheet_path()) };
        theme.set_translucent(config.get(&config::TRANSLUCENT));
        theme
    }

    pub fn set_translucent(&self, on: bool) {
        self.translucency.load_from_string(if on { "" } else { OPAQUE });
    }
}

/// Load `path` now and again whenever it changes. A file that doesn't exist
/// is an empty stylesheet.
fn watch_user_stylesheet(display: &gdk::Display, path: PathBuf) -> Option<Watch> {
    let provider = CssProvider::new();
    let file = path.display().to_string();
    // GTK keeps what parses and reports the rest.
    provider.connect_parsing_error(move |_, section, error| {
        eprintln!("theme: {file}:{}: {error}", section.start_location().lines() + 1);
    });
    gtk::style_context_add_provider_for_display(display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);

    let reload = {
        let path = path.clone();
        move || match std::fs::read_to_string(&path) {
            Ok(css) => provider.load_from_string(&css),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => provider.load_from_string(""),
            // Half-saved or unreadable: keep what was loaded.
            Err(e) => eprintln!("theme: could not read {}: {e}", path.display()),
        }
    };
    reload();

    // The monitor needs the directory to exist to hear the file appear.
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("theme: could not create {}: {e}", dir.display());
    }
    let monitor = match gio::File::for_path(&path).monitor_file(FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        Ok(monitor) => monitor,
        Err(e) => {
            eprintln!("theme: not watching {}: {e}", path.display());
            return None;
        }
    };
    let reload = Rc::new(RefCell::new(reload));
    let pending = Rc::new(Cell::new(false));
    monitor.connect_changed(move |_, _, _, event| {
        // `Changed` is a write in progress; the event that follows ends it.
        if event == FileMonitorEvent::Changed || pending.replace(true) {
            return;
        }
        let (reload, pending) = (reload.clone(), pending.clone());
        glib::timeout_add_local_once(SETTLE, move || {
            pending.set(false);
            (reload.borrow_mut())();
        });
    });
    Some(Watch { _monitor: monitor })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    const STYLE: &str = include_str!("style.css");
    const README: &str = include_str!("../../../README.md");

    /// Every `--neucord-*` name in `text`.
    fn variables(text: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(at) = rest.find("--neucord-") {
            let name: String = rest[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
            rest = &rest[at + name.len()..];
            // `--neucord-*` in prose.
            if !name.ends_with('-') {
                found.insert(name);
            }
        }
        found
    }

    #[test]
    fn the_sheet_defines_every_variable_it_uses() {
        let defined: BTreeSet<String> = STYLE
            .lines()
            .filter_map(|line| line.trim().strip_prefix("--neucord-").and_then(|_| line.trim().split(':').next()))
            .map(String::from)
            .collect();
        assert_eq!(variables(STYLE), defined, "used on the left, defined on the right");
    }

    #[test]
    fn the_readme_documents_exactly_the_variables_the_sheet_defines() {
        assert_eq!(variables(README), variables(STYLE), "documented on the left, in style.css on the right");
    }
}
