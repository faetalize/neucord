//! The Activity Privacy page of the settings window (404997): what activity
//! is shared (and with whom friends are told what), where it is shared by
//! default and in which servers, and who may join the games played. The
//! settings are the account's, saved as they change; what Neucord does not
//! have, games it could detect, is left to the clients that do.
//!
//! Changing the default asks whether the servers follow it, as the client
//! does (`g8` in 365258); the question and the lists are in
//! [`discord::activity_sharing`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::activity_sharing::{self, Listed, Order, Server, after_default};
use discord::settings::{ActivityDefault, Settings, share_custom_status_toggle};
use discord::{Setting, Snowflake};
use gtk::glib;

use super::App;
use super::settings::{Form, group};
use super::state::now_ms;

/// The servers listed before "Show more servers" (`slice(0, 10)`).
const SHOWN: usize = 10;

/// What the default can be, in the order the client lists them, with its
/// words (`m3oL7Q`, `5+lnTA`, `egr+VZ`).
const DEFAULTS: [(ActivityDefault, &str); 3] = [
    (ActivityDefault::Off, "Share in all servers"),
    (ActivityDefault::LargeServers, "Share in servers with 200 or fewer members"),
    (ActivityDefault::On, "Do not share in any servers"),
];

/// The server list's state: what is typed, how it is ordered, whether it is
/// all shown, and the rows by server.
struct Servers {
    list: gtk::ListBox,
    notice: gtk::Label,
    count: gtk::Label,
    toggle_all: gtk::Button,
    query: RefCell<String>,
    order: Cell<Order>,
    expanded: Cell<bool>,
    /// The order the rows are listed in, as it was when the sorting was picked.
    sorted: RefCell<Vec<Snowflake>>,
    rows: RefCell<Vec<(Snowflake, adw::SwitchRow)>>,
    updating: Rc<Cell<bool>>,
}

fn plural(count: usize, one: &str, other: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { other })
}

impl App {
    /// The page's groups: what is shared, where, and who may join.
    pub(super) fn activity_privacy_groups(self: &Rc<Self>, form: &mut Form) -> [adw::PreferencesGroup; 3] {
        let sharing = group("What I Share", "");
        form.switch(
            &sharing,
            "Share my activity",
            "Share activity information from games and connected apps I use, including when and how I engage.",
            Settings::shares_activity,
            Setting::ShowCurrentGame,
        );
        form.switch(
            &sharing,
            "Share when I come online",
            "Allow friends to receive a push notification when you come online.",
            |s| s.notifications.share_online,
            Setting::ShareOnline,
        );
        let custom = {
            let state = self.account.state.borrow();
            state.me.as_ref().is_some_and(|me| share_custom_status_toggle(&state.experiments, me.id))
        };
        if custom {
            form.switch(
                &sharing,
                "Share when I update my custom status",
                "Allow friends to receive a push notification when you update your custom status.",
                |s| s.notifications.share_custom_status,
                Setting::ShareCustomStatus,
            );
        }
        form.switch(
            &sharing,
            "Share when I start streaming",
            "Allow members that you interact with the most to get a notification when you start streaming in small \
             servers.",
            |s| s.notifications.share_go_live,
            Setting::ShareGoLive,
        );

        let places = group("Where I Share Activity", "");
        self.activity_default_row(&places, form);
        self.activity_server_list(&places, form);

        let joining = group(
            "Activity Joining",
            "Some games allow users to join the game you're playing when viewing your activity.",
        );
        form.switch(
            &joining,
            "Allow friends to join my game",
            "Allows friends to join without sending a request.",
            |s| s.privacy.friends_join_game,
            Setting::FriendsJoinGame,
        );
        form.switch(
            &joining,
            "Allow voice channel participants to join my game",
            "Allow anyone in a shared voice channel to join my game without sending a request. This feature only \
             works in non-Community enabled servers.",
            |s| s.privacy.voice_participants_join_game,
            Setting::VoiceParticipantsJoinGame,
        );
        let blurb = gtk::Label::builder()
            .label(
                "Looking for per-server controls? Navigate to the server, select the dropdown menu next to the \
                 server name, then select <b>Privacy Settings</b>. From there, you can toggle on or off Activity \
                 Joining for just that server.",
            )
            .use_markup(true)
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .margin_top(6)
            .build();
        joining.add(&blurb);
        [sharing, places, joining]
    }

    /// "Automatically share my activity when joining a server". The setting
    /// is saved first, then the servers are asked about.
    fn activity_default_row(self: &Rc<Self>, places: &adw::PreferencesGroup, form: &mut Form) {
        let labels: Vec<&str> = DEFAULTS.iter().map(|(_, label)| *label).collect();
        let row = adw::ComboRow::builder()
            .title("Automatically share my activity when joining a server")
            .subtitle("Your activity is always shared with your friends.")
            .model(&gtk::StringList::new(&labels))
            .build();
        row.connect_selected_notify({
            let (app, updating) = (Rc::downgrade(self), form.updating.clone());
            move |row| {
                let (Some(app), false) = (app.upgrade(), updating.get()) else { return };
                if let Some((choice, _)) = DEFAULTS.get(row.selected() as usize) {
                    app.set_activity_default(*choice);
                }
            }
        });
        places.add(&row);
        let app = Rc::downgrade(self);
        form.shows.push(Box::new(move |settings| {
            let Some(app) = app.upgrade() else { return };
            let current = settings.privacy.activity_default(&app.account.state.borrow().gates);
            row.set_selected(DEFAULTS.iter().position(|(d, _)| *d == current).unwrap_or(0) as u32);
        }));
    }

    /// Save a new default, and ask whether the servers that differ follow.
    fn set_activity_default(self: &Rc<Self>, new: ActivityDefault) {
        let (old, restricted, servers) = {
            let state = self.account.state.borrow();
            let old = state.settings.privacy.activity_default(&state.gates);
            let ours: Vec<Snowflake> = state.guilds.iter().map(|g| g.id).collect();
            let restricted = discord::settings::live_servers(
                &state.settings.privacy.activity_restricted_servers,
                &ours,
                state.unavailable_guilds,
            );
            let servers: Vec<Server> = state
                .guilds_in_picker_order(None)
                .into_iter()
                .map(|g| Server {
                    id: g.id,
                    joined_at: g.joined_at,
                    member_count: state.member_counts.get(&g.id).copied().or(g.member_count),
                })
                .collect();
            (old, restricted, servers)
        };
        if old == new {
            return;
        }
        // What is set first, as the client does, then asked about; the
        // restricted list is the one as it stands, which an answer adds to.
        let listed = self.account.state.borrow().settings.privacy.activity_restricted_servers.clone();
        self.change_setting(Setting::ActivityDefault { default: new, restricted_servers: listed });
        let Some(change) = after_default(old, new, &restricted, &servers) else { return };
        self.ask_servers_to_follow(change);
    }

    /// The card of servers the default would change, and the question.
    fn ask_servers_to_follow(self: &Rc<Self>, change: activity_sharing::Change) {
        let (first, count) = {
            let mut state = self.account.state.borrow_mut();
            let state = &mut *state;
            let scores = state.frecency.stores.channels.history.scores(now_ms() as u64);
            let score = |id: &Snowflake| scores.get(&id.0.to_string()).copied().unwrap_or(0.0);
            let mut ranked = change.servers.clone();
            ranked.sort_by(|a, b| score(b).total_cmp(&score(a)));
            let first = ranked.first().and_then(|id| state.guilds.iter().find(|g| g.id == *id)).map(|g| g.name.clone());
            (first.unwrap_or_default(), ranked.len())
        };
        let name = glib::markup_escape_text(change.setting_name);
        let dialog = adw::AlertDialog::builder()
            .heading(change.title())
            .body(change.question(&format!("<b>{name}</b>")))
            .body_use_markup(true)
            .build();
        let card = adw::ActionRow::builder()
            .title(plural(count, "Server", "Servers"))
            .subtitle(format!(
                "{} \u{b7} {}{}",
                change.card_state(),
                glib::markup_escape_text(&first),
                if count > 1 { " and others" } else { "" }
            ))
            .build();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        list.append(&card);
        dialog.set_extra_child(Some(&list));
        dialog.add_responses(&[("cancel", "Not Now"), ("confirm", change.confirm())]);
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("confirm"));
        dialog.set_close_response("cancel");
        dialog.connect_response(None, {
            let this = Rc::downgrade(self);
            move |_, response| {
                let (Some(this), "confirm") = (this.upgrade(), response) else { return };
                // The list as it is now, as `gF` reads it.
                let current = this.account.state.borrow().settings.privacy.activity_restricted_servers.clone();
                let restricted_servers = change.restricted_after(&this.live_servers(&current));
                this.change_setting(Setting::ActivityDefault { default: change.default, restricted_servers });
                this.settings_toast(change.toast());
            }
        });
        dialog.present(Some(&self.settings_parent()));
    }

    /// The servers, each with a switch for sharing in it, which can be
    /// searched, sorted, shown in full and all switched together.
    fn activity_server_list(self: &Rc<Self>, places: &adw::PreferencesGroup, form: &mut Form) {
        let servers = Rc::new(Servers {
            list: gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build(),
            notice: gtk::Label::builder()
                .label("Server settings ignored when not sharing")
                .xalign(0.0)
                .css_classes(["warning"])
                .visible(false)
                .build(),
            count: gtk::Label::builder().xalign(0.0).css_classes(["dim-label"]).build(),
            toggle_all: gtk::Button::builder().valign(gtk::Align::Center).build(),
            query: RefCell::default(),
            order: Cell::default(),
            expanded: Cell::new(false),
            sorted: RefCell::default(),
            rows: RefCell::default(),
            updating: form.updating.clone(),
        });
        let search = gtk::SearchEntry::builder().placeholder_text("Search my servers").hexpand(true).build();
        let order = adw::ComboRow::builder()
            .title("Sort by")
            .model(&gtk::StringList::new(&Order::ALL.map(Order::label)))
            .build();
        search.connect_search_changed({
            let (app, servers) = (Rc::downgrade(self), servers.clone());
            move |entry| {
                *servers.query.borrow_mut() = entry.text().to_string();
                if let Some(app) = app.upgrade() {
                    app.fill_activity_servers(&servers);
                }
            }
        });
        order.connect_selected_notify({
            let (app, servers) = (Rc::downgrade(self), servers.clone());
            move |row| {
                let Some(app) = app.upgrade() else { return };
                servers.order.set(Order::ALL.get(row.selected() as usize).copied().unwrap_or_default());
                // Ordered by how things are now, and kept so while they are toggled.
                app.sort_activity_servers(&servers);
                app.fill_activity_servers(&servers);
            }
        });
        servers.toggle_all.connect_clicked({
            let app = Rc::downgrade(self);
            move |_| {
                let Some(app) = app.upgrade() else { return };
                let (restricted, all) = {
                    let state = app.account.state.borrow();
                    (
                        state.settings.privacy.activity_restricted_servers.clone(),
                        state.guilds_in_picker_order(None).into_iter().map(|g| g.id).collect::<Vec<_>>(),
                    )
                };
                app.change_setting(Setting::ActivityRestrictedServers(activity_sharing::toggle_all(&restricted, &all)));
            }
        });
        let bar = gtk::Box::builder().spacing(6).build();
        bar.append(&search);
        bar.append(&servers.toggle_all);
        places.add(&servers.notice);
        places.add(&bar);
        places.add(&order);
        places.add(&servers.count);
        places.add(&servers.list);

        self.sort_activity_servers(&servers);
        self.fill_activity_servers(&servers);
        let app = Rc::downgrade(self);
        form.shows.push(Box::new(move |settings| {
            let Some(app) = app.upgrade() else { return };
            app.show_activity_servers(&servers, settings);
        }));
    }

    /// The servers as `servers.order` puts them, as they are now.
    fn sort_activity_servers(&self, servers: &Servers) {
        let state = self.account.state.borrow();
        let restricted = &state.settings.privacy.activity_restricted_servers;
        let all = self.activity_listed(&state);
        let sorted = activity_sharing::listed(&all, "", servers.order.get(), restricted);
        *servers.sorted.borrow_mut() = sorted.into_iter().map(|s| s.id).collect();
    }

    fn activity_listed(&self, state: &super::state::State) -> Vec<Listed> {
        state
            .guilds_in_picker_order(None)
            .into_iter()
            .map(|g| Listed { id: g.id, name: g.name.clone(), joined_at: g.joined_at })
            .collect()
    }

    /// Build the rows for what is typed and how it is sorted.
    fn fill_activity_servers(self: &Rc<Self>, servers: &Rc<Servers>) {
        while let Some(row) = servers.list.first_child() {
            servers.list.remove(&row);
        }
        servers.rows.borrow_mut().clear();
        let (matching, counts) = {
            let state = self.account.state.borrow();
            let all = self.activity_listed(&state);
            let order = servers.sorted.borrow().clone();
            let mut by_order: Vec<Listed> =
                order.iter().filter_map(|id| all.iter().find(|s| s.id == *id).cloned()).collect();
            // Servers joined since the sorting was picked go last.
            by_order.extend(all.iter().filter(|s| !order.contains(&s.id)).cloned());
            let query = servers.query.borrow().to_lowercase();
            by_order.retain(|s| query.is_empty() || s.name.to_lowercase().contains(&query));
            let counts: Vec<(Snowflake, usize)> = by_order
                .iter()
                .map(|s| {
                    let known = state.member_counts.get(&s.id).copied();
                    let held = state.guilds.iter().find(|g| g.id == s.id).and_then(|g| g.member_count);
                    (s.id, known.or(held).unwrap_or(0) as usize)
                })
                .collect();
            (by_order, counts)
        };
        let all_shown = servers.expanded.get() || !servers.query.borrow().is_empty() || matching.len() <= SHOWN;
        for (n, server) in matching.iter().enumerate() {
            if !all_shown && n >= SHOWN {
                break;
            }
            let members = counts.iter().find(|(id, _)| *id == server.id).map_or(0, |(_, c)| *c);
            let row = adw::SwitchRow::builder()
                .title(glib::markup_escape_text(&server.name))
                .subtitle(plural(members, "member", "members"))
                .build();
            row.connect_active_notify({
                let (app, updating, id) = (Rc::downgrade(self), servers.updating.clone(), server.id);
                move |row| {
                    let (Some(app), false) = (app.upgrade(), updating.get()) else { return };
                    // Switched on is shared: the list holds those that are not.
                    let mut current = app.account.state.borrow().settings.privacy.activity_restricted_servers.clone();
                    current.retain(|s| *s != id);
                    if !row.is_active() {
                        current.push(id);
                    }
                    app.change_setting(Setting::ActivityRestrictedServers(current));
                }
            });
            servers.list.append(&row);
            servers.rows.borrow_mut().push((server.id, row));
        }
        if matching.is_empty() {
            servers
                .list
                .append(&gtk::Label::builder().label("No results found").margin_top(12).margin_bottom(12).build());
        } else if servers.query.borrow().is_empty() && matching.len() > SHOWN {
            let more = matching.len() - SHOWN;
            let button = gtk::Button::builder()
                .label(if all_shown {
                    "Show fewer servers".to_owned()
                } else {
                    format!("Show {}", plural(more, "more server", "more servers"))
                })
                .css_classes(["flat"])
                .build();
            button.connect_clicked({
                let (app, servers) = (Rc::downgrade(self), servers.clone());
                move |_| {
                    servers.expanded.set(!servers.expanded.get());
                    if let Some(app) = app.upgrade() {
                        app.fill_activity_servers(&servers);
                    }
                }
            });
            servers.list.append(&button);
        }
        let settings = self.account.state.borrow().settings.clone();
        self.show_activity_servers(servers, &settings);
    }

    /// Set the switches, the count and the button to the settings.
    fn show_activity_servers(&self, servers: &Servers, settings: &Settings) {
        let restricted = &settings.privacy.activity_restricted_servers;
        // Put back as it was: the window sets all its rows under this flag too.
        let before = servers.updating.replace(true);
        for (id, row) in servers.rows.borrow().iter() {
            row.set_active(!restricted.contains(id));
        }
        servers.updating.set(before);
        servers.notice.set_visible(!settings.shares_activity());
        servers.count.set_label(&plural(servers.rows.borrow().len(), "server", "servers"));
        servers.toggle_all.set_label(activity_sharing::toggle_all_label(restricted));
    }
}
