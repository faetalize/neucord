//! Spellcheck in the message box, as the browser gives the web client's box
//! (on by default, `SpellcheckStore`): libspelling's checker over the box's
//! buffer, and its section of the box's menu (corrections, Add to
//! Dictionary, Ignore, Check Spelling, Languages), the desktop client's
//! groups 3 and 4 of 194400. Learned words go to enchant's personal
//! dictionary; the language follows the system's.
//!
//! The box is a source view only for this: libspelling checks a
//! `GtkSourceBuffer`. Its highlighting, bracket matching and style scheme
//! stay off, so it draws as a text view does.

use std::rc::Rc;

use adw::prelude::*;
use libspelling::{Checker, TextBufferAdapter};
use sourceview::prelude::*;

use super::App;
use super::menus::{Entry, Item, Kind};
use crate::config::{self, Store};

/// libspelling's section, as it keeps it for the word at the cursor.
pub(super) const SPELLING: Entry<()> = Entry::Item(Item {
    name: "spelling",
    label: "Spelling",
    shows: |_, _| true,
    kind: Kind::Linked(|app, _| Some(app.spelling.menu_model())),
});

/// The message box, and the checker over it, on or off as `config` says.
pub(super) fn composer(config: &Rc<Store>) -> (sourceview::View, TextBufferAdapter) {
    let buffer = sourceview::Buffer::new(None);
    buffer.set_highlight_syntax(false);
    buffer.set_highlight_matching_brackets(false);
    buffer.set_style_scheme(None);
    let view = sourceview::View::builder()
        .buffer(&buffer)
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .indent_on_tab(false)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["composer-input"])
        .build();
    let adapter = TextBufferAdapter::new(&buffer, &Checker::default());
    adapter.set_enabled(config.get(&config::SPELLCHECK));
    view.insert_action_group("spelling", Some(&adapter));
    // Connected after the first value, so starting up writes nothing.
    let config = config.clone();
    adapter.connect_enabled_notify(move |adapter| {
        if let Err(e) = config.set(&config::SPELLCHECK, adapter.is_enabled()) {
            eprintln!("config: {e}");
        }
    });
    (view, adapter)
}

impl App {
    /// Ready the corrections for the word the menu opens on, as the desktop
    /// client waits for the clicked word's (442433): a click outside the
    /// selection moves the cursor there first.
    pub(super) fn spelling_opening(&self, at: Option<(f64, f64)>) {
        if let Some((x, y)) = at {
            let view = &self.composer;
            let (x, y) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = view.iter_at_location(x, y) {
                let buffer = view.buffer();
                if !buffer.selection_bounds().is_some_and(|(start, end)| iter.in_range(&start, &end)) {
                    buffer.place_cursor(&iter);
                }
            }
        }
        self.spelling.update_corrections();
    }
}
