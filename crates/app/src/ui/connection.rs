//! Logging out, the gateway connection (reconnects and resumes), and
//! routing gateway events to the rest of the UI.

use std::rc::Rc;
use std::time::Duration;

use discord::{Channel, Event, Gateway, Guild, Ready, Rest, Snowflake, session};
use gtk::glib;
use gtk::prelude::*;

use super::App;
use super::state::{Page, State};

impl App {
    pub(super) fn logout(self: &Rc<Self>) {
        let rest = std::mem::take(&mut *self.state.borrow_mut()).rest;
        self.show_blank();
        self.timelines.borrow_mut().clear();
        // Keeps the app running until Discord has heard, even if the window
        // is closed right away.
        let hold = self.window.application().map(|app| app.hold());
        glib::spawn_future_local(async move {
            let _hold = hold;
            // Forget the token before the request, which can take a while:
            // a quick login meanwhile would store a new one, and clearing
            // after it would delete that instead.
            if let Err(e) = session::clear_token().await {
                eprintln!("logout: {e}");
            }
            // Offline or already revoked, the token is forgotten all the same.
            if let Some(rest) = rest
                && let Err(e) = rest.logout().await
            {
                eprintln!("logout: {e}");
            }
        });
        self.show_login(None);
    }

    pub(super) fn connect(self: &Rc<Self>, token: String, persisted: bool) {
        self.login.stop_qr();
        {
            let mut state = self.state.borrow_mut();
            state.rest = Some(Rest::new(token.clone()));
            state.token = Some(token.clone());
            state.persisted = persisted;
            state.gateway = None;
        }
        if !self.state.borrow().ready_seen {
            self.show_loading("Connecting to Discord…");
        }
        let resume = self.state.borrow().resume.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            match Gateway::connect(&token, resume).await {
                Ok((gateway, events)) => {
                    this.state.borrow_mut().gateway = Some(gateway);
                    this.run_events(events).await;
                }
                Err(e) => this.connection_lost(&e.to_string()),
            }
        });
    }

    /// Transport-level failure or a non-fatal close: retry with backoff if
    /// this token is known good, otherwise send the user back to login.
    pub(super) fn connection_lost(self: &Rc<Self>, why: &str) {
        let (known_good, attempts, token, resumable) = {
            let mut state = self.state.borrow_mut();
            state.gateway = None;
            state.reconnect_attempts += 1;
            (state.persisted || state.ready_seen, state.reconnect_attempts, state.token.clone(), state.resume.is_some())
        };
        let Some(token) = token.filter(|_| known_good) else {
            return self.show_login(Some(why));
        };
        // A resumable session is worth retrying at once: Discord only keeps
        // it for a short while. After that, back off.
        let delay = if resumable && attempts == 1 { 1 } else { 2u64.saturating_pow(attempts.min(6)).min(60) };
        let status = format!("Disconnected ({why}). Reconnecting in {delay}s…");
        if self.state.borrow().ready_seen {
            self.me_label.set_label(&status);
        } else {
            self.show_loading(&status);
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            if let Some(this) = this.upgrade() {
                // Logged out meanwhile?
                if this.state.borrow().token.as_ref() == Some(&token) {
                    let persisted = this.state.borrow().persisted;
                    this.connect(token, persisted);
                }
            }
        });
    }

    pub(super) async fn run_events(self: &Rc<Self>, events: async_channel::Receiver<Event>) {
        while let Ok(event) = events.recv().await {
            match event {
                Event::Ready(ready) => self.on_ready(*ready),
                Event::GuildCreate(guild) => self.on_guild(guild),
                Event::PrivateChannelCreate(channel) | Event::PrivateChannelUpdate(channel) => {
                    self.on_private_channel(channel)
                }
                Event::PrivateChannelDelete(id) => self.on_private_channel_delete(id),
                Event::MessageCreate(message) => self.on_message(*message),
                Event::MessageUpdate(edit) => self.on_message_update(edit),
                Event::MessageDelete { id, channel_id } => self.on_message_delete(id, channel_id),
                Event::MessageAck { channel_id, message_id } => {
                    self.state.borrow_mut().read.ack(channel_id, message_id);
                    self.withdraw_notification(channel_id);
                    self.refresh_badges();
                }
                Event::ChannelUnreadUpdate { channels, .. } => {
                    {
                        let mut state = self.state.borrow_mut();
                        for (channel, last) in channels {
                            state.bump_last_message(channel, last);
                        }
                    }
                    self.refresh_badges();
                }
                Event::MediaSettingsUpdate(patch) => {
                    self.state.borrow_mut().media.apply(patch);
                    self.refresh_rows();
                }
                Event::RelationshipAdd { relationship, notify } => {
                    self.learn_users(std::iter::once(&relationship.user));
                    let blocked = relationship.kind == discord::relationships::Kind::Blocked;
                    let request = notify
                        .then_some(&relationship)
                        .filter(|r| r.kind == discord::relationships::Kind::Incoming && !r.spam);
                    if let Some(request) = request {
                        self.notify_friend_request(request);
                    } else {
                        self.withdraw_friend_request(relationship.user.id);
                    }
                    let was_blocked = self.state.borrow().relationships.is_blocked(relationship.user.id);
                    self.state.borrow_mut().relationships.add(relationship);
                    self.relationships_changed(blocked != was_blocked);
                }
                Event::RelationshipUpdate { user_id, nickname } => {
                    self.state.borrow_mut().relationships.update(user_id, nickname);
                    self.relationships_changed(false);
                }
                Event::RelationshipRemove(user_id) => {
                    self.withdraw_friend_request(user_id);
                    let was_blocked = self.state.borrow().relationships.is_blocked(user_id);
                    self.state.borrow_mut().relationships.remove(user_id);
                    self.relationships_changed(was_blocked);
                }
                Event::PresenceUpdate { user_id, presence } => {
                    if self.state.borrow_mut().relationships.set_presence(user_id, presence) {
                        self.queue_friends_refresh();
                    }
                }
                Event::GuildFoldersUpdate(folders) => {
                    self.state.borrow_mut().guild_folders = folders;
                    self.rebuild_rail();
                }
                Event::NotificationSettingsUpdate(settings) => {
                    self.state.borrow_mut().notifications.update(settings);
                    self.refresh_badges();
                }
                Event::Resumed => {
                    let name = {
                        let mut state = self.state.borrow_mut();
                        state.reconnect_attempts = 0;
                        state.me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default()
                    };
                    self.me_label.set_label(&name);
                    // Replayed messages have already updated the open channel.
                }
                Event::Closed { code, reason, resume } => {
                    self.state.borrow_mut().resume = resume;
                    if Event::is_fatal_close(code) {
                        *self.state.borrow_mut() = State::default();
                        glib::spawn_future_local(async {
                            let _ = session::clear_token().await;
                        });
                        self.show_login(Some(&format!(
                            "Discord rejected the token (close code {code}). Log in again."
                        )));
                    } else if self.state.borrow().token.is_some() {
                        let why = if reason.is_empty() { format!("code {code}") } else { reason };
                        self.connection_lost(&why);
                    }
                    return;
                }
            }
        }
    }

    pub(super) fn on_ready(self: &Rc<Self>, ready: Ready) {
        let Ready {
            user,
            guilds,
            private_channels,
            relationships,
            read_states,
            notification_settings,
            guild_folders,
            media_settings,
        } = ready;
        self.learn_users(private_channels.iter().flat_map(|c| &c.recipients));
        self.learn_users(
            [
                discord::relationships::List::All,
                discord::relationships::List::Pending,
                discord::relationships::List::Blocked,
            ]
            .into_iter()
            .flat_map(|list| relationships.list(list))
            .map(|r| &r.user),
        );
        let (store, was_ready) = {
            let mut state = self.state.borrow_mut();
            let was_ready = state.ready_seen;
            state.ready_seen = true;
            state.reconnect_attempts = 0;
            state.guilds = guilds;
            state.private_channels = private_channels;
            state.relationships = relationships;
            state.read = read_states;
            state.guild_folders = guild_folders;
            state.notifications = notification_settings;
            state.media = media_settings;
            let store = (!state.persisted).then(|| (state.token.clone(), user.username.clone()));
            state.persisted = true;
            state.users.insert(user.id.0, user.display_name().to_string());
            state.me = Some(user);
            (store, was_ready)
        };
        if let Some((Some(token), username)) = store {
            glib::spawn_future_local(async move {
                if let Err(e) = session::store_token(&token, &format!("Neucord ({username})")).await {
                    eprintln!("could not save token: {e}");
                }
            });
        }

        let me = self.state.borrow().me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default();
        self.me_label.set_label(&me);
        self.stack.set_visible_child_name("main");

        // Stay on a Home page across reconnects.
        let page = self.state.borrow().page.filter(|_| was_ready);
        match page {
            Some(Page::Friends) => return self.show_friends(),
            Some(Page::MessageRequests) => return self.show_requests(),
            None => {}
        }

        // Keep the open guild (or Home) across reconnects; on first connect
        // open the first guild, or Home if there are none.
        let open = {
            let state = self.state.borrow();
            match state.guild {
                Some(id) if state.guilds.iter().any(|g| g.id == id) => Some(id),
                None if was_ready => None,
                _ => state.first_guild(),
            }
        };
        // After a reconnect that could not resume, stay in the open channel.
        let keep = {
            let state = self.state.borrow();
            state.channel.filter(|&c| was_ready && state.find_channel(c).is_some_and(|(g, _)| g.map(|g| g.id) == open))
        };
        match (keep, open) {
            (Some(channel), _) => {
                self.state.borrow_mut().guild = open;
                self.rebuild_rail();
                self.select_channel(channel);
            }
            (None, Some(id)) => self.select_guild(id),
            (None, None) => self.select_home(),
        }
    }

    /// A DM was created or changed (a message request accepted, a group
    /// renamed). Also applies the channel a REST call returned.
    pub(super) fn on_private_channel(self: &Rc<Self>, mut channel: Channel) {
        self.learn_users(channel.recipients.iter());
        let (home, open) = {
            let mut state = self.state.borrow_mut();
            if let Some(old) = state.private_channels.iter().find(|c| c.id == channel.id) {
                // Updates may leave out recipients, and can't know of newer messages.
                if channel.recipients.is_empty() {
                    channel.recipients = old.recipients.clone();
                }
                channel.last_message_id = channel.last_message_id.max(old.last_message_id);
            }
            state.private_channels.retain(|c| c.id != channel.id);
            let open = state.channel == Some(channel.id);
            state.private_channels.push(channel);
            (state.guild.is_none(), open)
        };
        self.refresh_home(home, open);
    }

    pub(super) fn on_private_channel_delete(self: &Rc<Self>, id: Snowflake) {
        let (home, was_open, was_request, requests_left) = {
            let mut state = self.state.borrow_mut();
            let was_request = state.private_channels.iter().any(|c| c.id == id && c.is_request());
            state.private_channels.retain(|c| c.id != id);
            (state.guild.is_none(), state.channel == Some(id), was_request, !state.requests().is_empty())
        };
        if was_open && was_request && requests_left {
            self.show_requests();
        } else if was_open {
            self.select_home();
        } else {
            self.refresh_home(home, false);
        }
    }

    /// Redraw what shows DMs: the sidebar and requests page on Home, and the
    /// request bar if the changed DM is open.
    fn refresh_home(self: &Rc<Self>, home: bool, open: bool) {
        if open {
            self.update_request_bar();
        }
        if home {
            self.rebuild_sidebar();
        }
        if self.state.borrow().page == Some(Page::MessageRequests) {
            self.rebuild_requests();
        }
        self.refresh_badges();
    }

    /// After a relationship changed: badges, the friends page, and, if
    /// someone was blocked or unblocked, the messages on screen.
    fn relationships_changed(self: &Rc<Self>, blocks_changed: bool) {
        self.refresh_badges();
        self.queue_friends_refresh();
        if blocks_changed {
            self.refresh_rows();
        }
    }

    pub(super) fn on_guild(self: &Rc<Self>, guild: Guild) {
        let id = guild.id;
        let is_open = {
            let mut state = self.state.borrow_mut();
            match state.guilds.iter_mut().find(|g| g.id == id) {
                Some(existing) => *existing = guild,
                None => state.guilds.push(guild),
            }
            state.guild == Some(id)
        };
        self.rebuild_rail();
        if is_open {
            self.rebuild_sidebar();
        }
    }
}
