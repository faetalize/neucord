//! The privacy rows of the settings window: who may message us from servers
//! and whether it is a request, who may send friend requests, and how much
//! of the profile others see. They are the official client's Privacy &
//! Safety page (960111), saved as it saves them.
//!
//! Direct messages and message requests are the account's defaults, and
//! each server can differ; changing a default asks whether to apply it to
//! every server too, as the client does.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use discord::settings::{FriendSources, ProfileVisibility, Row, live_servers, with_server};
use discord::{Setting, Snowflake};

use super::App;
use super::settings::{Form, group};

/// Which server the rows are for: 0 is the account's defaults, then the
/// servers in the rail's order.
type Selected = Rc<Cell<usize>>;

const GUARDIAN: &str = "Check in with your guardian to change this setting for you.";

/// Put what a switch should show on it.
fn show_row(row: &adw::SwitchRow, state: Row, why_not: Option<&str>) {
    row.set_active(state.on);
    row.set_sensitive(!state.disabled);
    row.set_tooltip_text(why_not.filter(|_| state.disabled));
}

impl App {
    pub(super) fn privacy_groups(self: &Rc<Self>, form: &mut Form) -> [adw::PreferencesGroup; 3] {
        let servers: Vec<(Snowflake, String)> = {
            let state = self.state.borrow();
            state.guilds_in_picker_order(None).into_iter().map(|g| (g.id, g.name.clone())).collect()
        };
        let servers = Rc::new(servers);
        let selected: Selected = Rc::new(Cell::new(0));

        let messaging = group(
            "Server Privacy",
            "Who may message you from the servers you are in. Each server can differ from your defaults.",
        );
        let names: Vec<&str> =
            std::iter::once("All servers (defaults)").chain(servers.iter().map(|(_, name)| name.as_str())).collect();
        let selector =
            adw::ComboRow::builder().title("Server").model(&gtk::StringList::new(&names)).enable_search(true).build();
        selector.connect_selected_notify({
            let (app, selected) = (Rc::downgrade(self), selected.clone());
            move |row| {
                selected.set(row.selected() as usize);
                if let Some(app) = app.upgrade() {
                    app.settings_replaced();
                }
            }
        });
        messaging.add(&selector);

        let direct = adw::SwitchRow::builder().subtitle("Server members can message you.").build();
        direct.connect_active_notify({
            let (app, updating, selected, servers) =
                (Rc::downgrade(self), form.updating.clone(), selected.clone(), servers.clone());
            move |row| {
                if let (false, Some(app)) = (updating.get(), app.upgrade()) {
                    let server = servers.get(selected.get().wrapping_sub(1)).map(|(id, _)| *id);
                    app.set_direct_messages(server, row.is_active());
                }
            }
        });
        messaging.add(&direct);
        form.shows.push(Box::new({
            let (app, selected, servers) = (Rc::downgrade(self), selected.clone(), servers.clone());
            move |settings| {
                let Some(app) = app.upgrade() else { return };
                let server = servers.get(selected.get().wrapping_sub(1)).map(|(id, _)| *id);
                let gates = app.state.borrow().gates;
                // Direct messages and friend requests are one switch where only server members may send requests.
                let both = server.is_none() && settings.privacy.friend_sources.only_server_members();
                direct.set_title(if both { "Direct Messages and Friend Requests" } else { "Direct Messages" });
                direct.set_subtitle(match (both, server.is_some()) {
                    (true, _) => "Allow DMs and friend requests from other server members",
                    (false, false) => "Allow DMs from other server members",
                    (false, true) => "Allow DMs from other members in this server",
                });
                show_row(&direct, settings.privacy.direct_messages(&gates, server), Some(GUARDIAN));
            }
        }));

        let requests = adw::SwitchRow::builder()
            .title("Message requests")
            .subtitle("Filter messages from server members you may not know.")
            .build();
        requests.connect_active_notify({
            let (app, updating, selected, servers) =
                (Rc::downgrade(self), form.updating.clone(), selected.clone(), servers.clone());
            move |row| {
                if let (false, Some(app)) = (updating.get(), app.upgrade()) {
                    let server = servers.get(selected.get().wrapping_sub(1)).map(|(id, _)| *id);
                    app.set_message_requests(server, row.is_active());
                }
            }
        });
        messaging.add(&requests);
        form.shows.push(Box::new({
            let (app, selected, servers) = (Rc::downgrade(self), selected, servers);
            move |settings| {
                let Some(app) = app.upgrade() else { return };
                let server = servers.get(selected.get().wrapping_sub(1)).map(|(id, _)| *id);
                let gates = app.state.borrow().gates;
                show_row(&requests, settings.privacy.message_requests(&gates, server), Some(GUARDIAN));
            }
        }));

        // Who may send friend requests.
        let friends = group("Allow Friend Requests from…", "");
        let source = |title: &str,
                      subtitle: &str,
                      on: fn(FriendSources) -> bool,
                      set: fn(FriendSources, bool) -> FriendSources,
                      form: &mut Form| {
            let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).build();
            row.connect_active_notify({
                let (app, updating) = (Rc::downgrade(self), form.updating.clone());
                move |row| {
                    let Some(app) = app.upgrade().filter(|_| !updating.get()) else { return };
                    let current = app.state.borrow().settings.privacy.friend_sources;
                    app.change_setting(Setting::FriendSourceFlags(set(current, row.is_active())));
                }
            });
            friends.add(&row);
            let app = Rc::downgrade(self);
            form.shows.push(Box::new(move |settings| {
                let Some(app) = app.upgrade() else { return };
                let disabled = app.state.borrow().gates.parent_linked;
                show_row(&row, Row { on: on(settings.privacy.friend_sources), disabled }, Some(GUARDIAN));
            }));
        };
        source("Everyone", "", FriendSources::everyone, |_, on| FriendSources::with_everyone(on), form);
        source("Friend of friends", "", FriendSources::mutual_friends, FriendSources::with_mutual_friends, form);
        source(
            "Server members",
            "Server members can only send you friend requests from servers where you also allow Direct Messages.",
            FriendSources::mutual_servers,
            FriendSources::with_mutual_servers,
            form,
        );

        let profile = group("Profile Privacy", "");
        let row = adw::ComboRow::builder()
            .title("Share my full profile with")
            .model(&gtk::StringList::new(&ProfileVisibility::ALL.map(ProfileVisibility::label)))
            .build();
        row.connect_selected_notify({
            let (app, updating) = (Rc::downgrade(self), form.updating.clone());
            move |row| {
                let choice = ProfileVisibility::ALL.get(row.selected() as usize);
                if let (false, Some(app), Some(choice)) = (updating.get(), app.upgrade(), choice) {
                    app.change_setting(Setting::ProfileVisibility(*choice));
                }
            }
        });
        profile.add(&row);
        form.shows.push(Box::new(move |settings| {
            let visibility = settings.privacy.profile_visibility;
            row.set_selected(ProfileVisibility::ALL.iter().position(|v| *v == visibility).unwrap_or(0) as u32);
            row.set_subtitle(visibility.description());
        }));

        [messaging, friends, profile]
    }

    /// "Direct Messages" was switched, for the defaults or for one server.
    fn set_direct_messages(self: &Rc<Self>, server: Option<Snowflake>, allowed: bool) {
        match server {
            Some(server) => {
                let current = self.state.borrow().settings.privacy.restricted_servers.clone();
                let servers = with_server(&self.live_servers(&current), server, !allowed);
                self.change_setting(Setting::RestrictedServers(servers));
            }
            None => self.ask_to_apply(
                "Do you want to also apply this change to all your existing servers? You can also disable direct messages on individual servers by visiting that server's settings page.",
                !allowed,
                |restricted, apply_to| Setting::DefaultServersRestricted { restricted, apply_to },
            ),
        }
    }

    /// "Message requests" was switched.
    fn set_message_requests(self: &Rc<Self>, server: Option<Snowflake>, on: bool) {
        // Turning them off may need the age verification the client opens
        // a flow for.
        if !on && self.state.borrow().gates.message_requests_age_gated {
            self.settings_replaced();
            return self.settings_toast("Verify your age in the official app before turning message requests off");
        }
        match server {
            Some(server) => {
                let current = self.state.borrow().settings.privacy.message_request_restricted_servers.clone();
                let servers = with_server(&self.live_servers(&current), server, !on);
                self.change_setting(Setting::MessageRequestRestrictedServers(servers));
            }
            None => self.ask_to_apply(
                "Do you want to also apply this change to all your existing servers? You can also disable messages requests on individual servers by visiting that servers setting's page.",
                !on,
                |restricted, apply_to| Setting::DefaultMessageRequestsRestricted { restricted, apply_to },
            ),
        }
    }

    /// The servers `ids` name that we are still in, as the client reads the
    /// lists of restrictions.
    fn live_servers(&self, ids: &[Snowflake]) -> Vec<Snowflake> {
        let state = self.state.borrow();
        let ours: Vec<Snowflake> = state.guilds.iter().map(|g| g.id).collect();
        live_servers(ids, &ours, state.unavailable_guilds)
    }

    /// Changing a default asks whether every server follows (`No` changes
    /// only the default, `Yes` sets every server to it), as the client does.
    /// Closing the question changes nothing.
    fn ask_to_apply(
        self: &Rc<Self>,
        body: &str,
        restricted: bool,
        setting: fn(bool, Option<Vec<Snowflake>>) -> Setting,
    ) {
        let dialog = adw::AlertDialog::new(Some("Server Privacy"), Some(body));
        dialog.add_responses(&[("no", "_No"), ("yes", "_Yes")]);
        dialog.set_close_response("close");
        dialog.connect_response(None, {
            let this = Rc::downgrade(self);
            move |_, response| {
                let Some(this) = this.upgrade() else { return };
                match response {
                    "no" => this.change_setting(setting(restricted, None)),
                    "yes" => {
                        // Every server we are in, or none when allowing.
                        let all = this.state.borrow().guilds.iter().map(|g| g.id).collect();
                        this.change_setting(setting(restricted, Some(if restricted { all } else { Vec::new() })));
                    }
                    _ => this.settings_replaced(),
                }
            }
        });
        dialog.present(Some(&self.settings_parent()));
    }
}
