//! Neucord: a native GTK 4 and libadwaita Discord client.

mod highlight;
mod images;
mod render;
mod rich_text;
mod timelines;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio};

const APP_ID: &str = "dev.faetalize.neucord";

fn main() -> glib::ExitCode {
    let app = adw::Application::new(Some(APP_ID), gio::ApplicationFlags::default());

    app.connect_startup(|app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &adw::Application, _, _| {
                let dialog = adw::AboutDialog::builder()
                    .application_name("Neucord")
                    .version(env!("CARGO_PKG_VERSION"))
                    .license_type(gtk::License::Gpl30)
                    .website("https://github.com/faetalize/neucord")
                    .issue_url("https://github.com/faetalize/neucord/issues")
                    .build();
                dialog.present(app.active_window().as_ref());
            })
            .build();
        app.add_action_entries([quit, about]);
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
