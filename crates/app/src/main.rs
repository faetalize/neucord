//! Neucord: a native GTK 4 Discord client.

mod images;
mod render;
mod ui;

use gtk::prelude::*;
use gtk::{gdk, gio};

const APP_ID: &str = "dev.faetalize.neucord";

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::default());

    app.connect_startup(|app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let quit = gio::ActionEntry::builder("quit").activate(|app: &gtk::Application, _, _| app.quit()).build();
        app.add_action_entries([quit]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
    });

    app.connect_activate(|app| {
        // Single window: a second launch just raises it.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        ui::App::new(app).start();
    });

    app.run()
}
