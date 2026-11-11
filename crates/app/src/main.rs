//! Neucord: a native GTK 4 and libadwaita Discord client.

mod background;
mod config;
mod desktop;
mod highlight;
mod images;
mod kept;
mod recorder;
mod render;
mod rich_text;
mod theme;
mod timelines;
mod tray;
mod ui;

use adw::prelude::*;
use gtk::gio;

const APP_ID: &str = "dev.faetalize.neucord";

fn main() -> glib::ExitCode {
    let app = adw::Application::new(Some(APP_ID), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.add_main_option(
        background::FLAG,
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start hidden, behind the tray icon",
        None,
    );

    app.connect_startup(|app| {
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

    app.connect_activate(|app| launch(app, false));

    // The instance that is running gets the flags of a later launch. A
    // `--background` launch (login autostart) never raises a window that
    // exists already.
    app.connect_command_line(|app, command| {
        let background = command.options_dict().contains(background::FLAG);
        if !(background && app.active_window().is_some()) {
            launch(app, background);
        }
        glib::ExitCode::SUCCESS
    });

    app.run()
}

fn launch(app: &adw::Application, background: bool) {
    // Single window: a second launch just raises it.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    ui::App::new(app).start(background);
}
