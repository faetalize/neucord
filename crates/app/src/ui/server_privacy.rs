//! A server's Privacy Settings, from its icon's menu (`guild-context`,
//! 871638; the dialog is 632038): the switches of the Privacy page for that
//! server alone, and the two that exist only here, saved as the page saves
//! them. They are the account's lists of servers (`restricted_guild_ids` and
//! its kin), so what changes here shows on the page and the other way round.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use discord::Setting;
use discord::Snowflake;
use discord::settings::{Row, Settings, direct_messages_text, message_requests_note, with_server};

use super::App;
use super::menus::{Entry, Item, Kind};
use super::privacy::show_row;
use super::settings::Show;

/// The server whose menu opened.
pub(super) struct GuildTarget(pub(super) Snowflake);

/// Privacy Settings is in every server's menu, between Server Settings and
/// Edit Per-server Profile.
pub(super) const PRIVACY_SETTINGS: Entry<GuildTarget> = Entry::Item(Item {
    name: "server-privacy-settings",
    label: "_Privacy Settings",
    shows: |app, target| app.state.borrow().guilds.iter().any(|g| g.id == target.0),
    kind: Kind::Action(|app, target| app.show_server_privacy(target.0)),
});

/// The open dialog: what shows the settings in it, and where a toast goes.
pub(super) struct Dialog {
    dialog: adw::Dialog,
    toasts: adw::ToastOverlay,
    show: Show,
}

impl Dialog {
    pub(super) fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    pub(super) fn show(&self, settings: &Settings) {
        (self.show)(settings);
    }
}

/// What a row says under its title, from the settings.
type Note = Box<dyn Fn(&Settings) -> (String, String)>;
/// What a row shows.
type Getter = Box<dyn Fn(&App, &Settings) -> Row>;

/// Where a policy page of Discord's is.
const POLICIES: &str = "https://discord.com/safety/360043709612-our-policies";

impl App {
    /// Open the dialog for `server`, or bring forward the one that is open.
    pub(super) fn show_server_privacy(self: &Rc<Self>, server: Snowflake) {
        if let Some(open) = self.server_privacy.borrow().as_ref() {
            return open.dialog.present(Some(&self.window));
        }
        let Some((name, hub)) = self
            .state
            .borrow()
            .guilds
            .iter()
            .find(|g| g.id == server)
            .map(|g| (g.name.clone(), g.features.iter().any(|f| f == "HUB")))
        else {
            return;
        };
        let updating = Rc::new(Cell::new(false));
        let mut shows: Vec<Show> = Vec::new();
        let group = adw::PreferencesGroup::new();

        // One row: it saves what its switch says through `set`, and shows what `get` finds.
        let mut row = |title: &str,
                       note: Note,
                       get: Getter,
                       set: fn(&Rc<App>, Snowflake, bool),
                       why_not: Option<&'static str>| {
            let switch = adw::SwitchRow::builder().title(title).build();
            switch.connect_active_notify({
                let (app, updating) = (Rc::downgrade(self), updating.clone());
                move |switch| {
                    if let (false, Some(app)) = (updating.get(), app.upgrade()) {
                        set(&app, server, switch.is_active());
                    }
                }
            });
            group.add(&switch);
            let app = Rc::downgrade(self);
            shows.push(Box::new(move |settings| {
                let Some(app) = app.upgrade() else { return };
                let (title, subtitle) = note(settings);
                switch.set_title(&title);
                switch.set_subtitle(&subtitle);
                show_row(&switch, get(&app, settings), why_not);
            }));
        };
        row(
            "Direct Messages",
            Box::new(move |settings| {
                let (title, note) =
                    direct_messages_text(settings.privacy.friend_sources.only_server_members(), Some(hub));
                (title.to_owned(), note.to_owned())
            }),
            Box::new(move |app, settings| settings.privacy.direct_messages(&app.state.borrow().gates, Some(server))),
            |app, server, allowed| app.set_direct_messages(Some(server), allowed),
            None,
        );
        row(
            "Message requests",
            Box::new(move |_| ("Message requests".to_owned(), message_requests_note(hub).to_owned())),
            Box::new(move |app, settings| settings.privacy.message_requests(&app.state.borrow().gates, Some(server))),
            |app, server, on| app.set_message_requests(Some(server), on),
            Some(super::privacy::GUARDIAN),
        );
        row(
            "Share my activity",
            Box::new(|_| {
                (
                    "Share my activity".to_owned(),
                    "Share activity information from games and connected apps I use, including when and how I engage."
                        .to_owned(),
                )
            }),
            Box::new(move |_, settings| settings.privacy.activity_status(server)),
            |app, server, on| {
                let current = app.state.borrow().settings.privacy.activity_restricted_servers.clone();
                let servers = with_server(&app.live_servers(&current), server, !on);
                app.change_setting(Setting::ActivityRestrictedServers(servers));
            },
            None,
        );
        row(
            "Activity joining",
            Box::new(|_| {
                ("Activity joining".to_owned(), "Allow users to join your activity in this server.".to_owned())
            }),
            Box::new(move |_, settings| settings.privacy.activity_joining(server)),
            |app, server, on| {
                let current = app.state.borrow().settings.privacy.activity_joining_restricted_servers.clone();
                let servers = with_server(&app.live_servers(&current), server, !on);
                app.change_setting(Setting::ActivityJoiningRestrictedServers(servers));
            },
            None,
        );

        let policies = adw::PreferencesGroup::new();
        let note = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .xalign(0.0)
            .label(format!(
                "Learn about enhanced safety measures in Community Servers <a href=\"{POLICIES}\">here</a>."
            ))
            .build();
        policies.add(&note);
        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.add(&policies);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&page));
        let done = gtk::Button::builder()
            .label("Done")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&toasts));
        view.add_bottom_bar(&done);
        let dialog = adw::Dialog::builder()
            .title(format!("Privacy Settings\u{2014}{name}"))
            .content_width(480)
            .child(&view)
            .build();
        done.connect_clicked({
            let dialog = dialog.downgrade();
            move |_| {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            }
        });
        dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.server_privacy.take();
                }
            }
        });

        let show = {
            let updating = updating.clone();
            move |settings: &Settings| {
                updating.set(true);
                shows.iter().for_each(|show| show(settings));
                updating.set(false);
            }
        };
        show(&self.state.borrow().settings);
        dialog.present(Some(&self.window));
        *self.server_privacy.borrow_mut() = Some(Dialog { dialog, toasts, show: Box::new(show) });
    }
}
