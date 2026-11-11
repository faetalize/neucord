//! Logging out, the gateway connection (reconnects and resumes), and
//! routing gateway events to the rest of the UI.

use std::rc::Rc;
use std::time::Duration;

use discord::accounts::Answer;
use discord::{Channel, Event, Gateway, Guild, Member, Ready, Rest, Snowflake, session};
use gtk::glib;
use gtk::prelude::*;

use super::App;
use super::accounts::Origin;
use super::state::{Page, now_ms};

impl App {
    /// Let go of the shown account without telling Discord: its gateway
    /// closes, what the window holds of it goes, and so do its desktop
    /// notifications, its unread count and its tray icon's alert. Gives what
    /// answered its requests.
    pub(super) fn leave_account(self: &Rc<Self>) -> Option<Rest> {
        let (rest, notified) = self.account.leave();
        self.withdraw_notifications(notified);
        // Folds it hadn't saved are not the next account's to send.
        if let Some(timer) = self.fold_save.take() {
            timer.remove();
        }
        self.show_blank();
        self.refresh_badges();
        self.refresh_typing();
        self.set_reply(None);
        self.end_edit();
        self.cancel_command();
        self.refresh_command_status();
        self.discard_drafts();
        rest
    }

    /// End the shown account's session at Discord and forget it here.
    pub(super) fn logout(self: &Rc<Self>) {
        let (id, migrating) = {
            let state = self.account.state.borrow();
            (state.known_id(), state.migrating)
        };
        let rest = self.leave_account();
        // Keeps the app running until Discord has heard, even if the window
        // is closed right away.
        let hold = self.window.application().map(|app| app.hold());
        let this = self.clone();
        glib::spawn_future_local(async move {
            let _hold = hold;
            // Forget the token before the request, which can take a while:
            // a quick login meanwhile would store a new one, and clearing
            // after it would delete that instead.
            this.forget_account(id, migrating).await;
            // Offline or already revoked, the token is forgotten all the same.
            if let Some(rest) = rest
                && let Err(e) = rest.logout().await
            {
                eprintln!("logout: {e}");
            }
        });
        self.show_signed_out(None);
    }

    /// Take an account off the list and its token out of the keyring, and
    /// what was kept pending for it off the disk. `migrating` is a token that
    /// is still in the entry of earlier versions.
    pub(super) async fn forget_account(self: &Rc<Self>, id: Option<Snowflake>, migrating: bool) {
        if let Some(id) = id {
            self.roster.borrow_mut().remove(id);
            self.refresh_account_views();
            super::usage::forget_usage(id);
            if let Err(e) = session::clear_token(id).await {
                eprintln!("logout: {e}");
            }
            self.save_roster().await;
        }
        if migrating && let Err(e) = session::clear_legacy_token().await {
            eprintln!("logout: {e}");
        }
    }

    pub(super) fn connect(self: &Rc<Self>, token: String, persisted: bool) {
        self.login.stop_qr();
        {
            let mut state = self.account.state.borrow_mut();
            let this = Rc::downgrade(self);
            let pauses = Rc::new(move |wait| {
                if let Some(this) = this.upgrade() {
                    this.show_paused(wait);
                }
            });
            let rest = Rest::new(token.clone()).solving_captchas(super::captcha::solver(&self.window));
            state.rest = Some(rest.reporting_pauses(pauses));
            state.token = Some(token.clone());
            state.persisted = persisted;
            state.gateway = None;
        }
        if !self.account.state.borrow().ready_seen {
            self.show_loading("Connecting to Discord…");
        }
        let resume = self.account.state.borrow().resume.clone();
        let epoch = self.account.epoch.get();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let connected = Gateway::connect(&token, resume).await;
            // The account was left meanwhile.
            if this.account.epoch.get() != epoch {
                return;
            }
            match connected {
                Ok((gateway, events)) => {
                    this.account.state.borrow_mut().gateway = Some(gateway);
                    this.run_events(events, epoch).await;
                }
                Err(e) => this.connection_lost(&e.to_string()),
            }
        });
    }

    /// Transport-level failure or a non-fatal close: retry with backoff if
    /// this token is known good, otherwise send the user back to login.
    pub(super) fn connection_lost(self: &Rc<Self>, why: &str) {
        let (known_good, attempts, token, resumable) = {
            let mut state = self.account.state.borrow_mut();
            state.gateway = None;
            state.reconnect_attempts += 1;
            (state.persisted || state.ready_seen, state.reconnect_attempts, state.token.clone(), state.resume.is_some())
        };
        let Some(token) = token.filter(|_| known_good) else {
            return self.show_signed_out(Some(why));
        };
        // A resumable session is worth retrying at once: Discord only keeps
        // it for a short while. After that, back off.
        let delay = if resumable && attempts == 1 { 1 } else { 2u64.saturating_pow(attempts.min(6)).min(60) };
        let status = format!("Disconnected ({why}). Reconnecting in {delay}s…");
        if self.account.state.borrow().ready_seen {
            self.me_label.set_label(&status);
        } else {
            self.show_loading(&status);
        }
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            if let Some(this) = this.upgrade() {
                // Logged out or switched meanwhile?
                if this.account.epoch.get() == epoch && this.account.state.borrow().token.as_ref() == Some(&token) {
                    let persisted = this.account.state.borrow().persisted;
                    this.connect(token, persisted);
                }
            }
        });
    }

    pub(super) async fn run_events(self: &Rc<Self>, events: async_channel::Receiver<Event>, epoch: u64) {
        while let Ok(event) = events.recv().await {
            // What a left account's connection still says is no one's.
            if self.account.epoch.get() != epoch {
                return;
            }
            match event {
                Event::Ready(ready) => self.on_ready(*ready),
                Event::GuildCreate(guild) => self.on_guild(guild),
                Event::GuildVoiceStates { guild_id, states } => {
                    self.account.state.borrow_mut().voice.replace_guild(guild_id, states);
                    self.refresh_badges();
                }
                Event::VoiceStates(updates) => {
                    let mut changed = false;
                    {
                        let mut state = self.account.state.borrow_mut();
                        for update in updates {
                            changed |= state.voice.apply(update);
                        }
                    }
                    if changed {
                        self.refresh_badges();
                    }
                }
                Event::PrivateChannelCreate(channel) | Event::PrivateChannelUpdate(channel) => {
                    self.on_private_channel(channel)
                }
                Event::PrivateChannelDelete(id) => self.on_private_channel_delete(id),
                Event::GuildChannelUpdate { guild_id, channel } => self.on_guild_channel(guild_id, channel),
                Event::ThreadUpdate { guild_id, thread } => {
                    self.on_threads(guild_id, |g| g.set_thread(thread.clone()));
                    self.forum_thread_updated(&thread);
                }
                Event::ThreadCreate { guild_id, thread, newly_created } => {
                    self.on_threads(guild_id, |g| g.set_thread(thread.clone()));
                    self.forum_thread_created(guild_id, &thread, newly_created);
                }
                Event::ThreadDelete { guild_id, id } => self.on_thread_delete(guild_id, id),
                Event::ThreadListSync { guild_id, threads, members, complete } => {
                    self.on_threads(guild_id, |g| {
                        for thread in threads {
                            g.set_thread(thread);
                        }
                        for (thread, member) in members {
                            g.set_thread_member(thread, Some(member));
                        }
                    });
                    self.forum_synced(guild_id, complete);
                }
                Event::ForumUnreads { threads, .. } => self.forum_unreads_answered(&threads),
                Event::ThreadMember { guild_id, thread, member } => {
                    self.on_threads(guild_id, |g| g.set_thread_member(thread, member))
                }
                Event::MemberUpdate { guild_id, member } => self.on_member(guild_id, member),
                Event::Members { guild_id, members, not_found } => {
                    let (open, changed) = {
                        let mut state = self.account.state.borrow_mut();
                        let changed = state.members.add(guild_id, members, &not_found);
                        (state.guild == Some(guild_id), changed)
                    };
                    // Member lists carry members all the time, mostly as we know them.
                    if open && changed {
                        self.refresh_rows();
                        self.refresh_member_rows();
                    }
                }
                Event::MemberList(update) => self.on_member_list(*update),
                Event::GuildUpdate(update) => {
                    let id = update.id;
                    if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == id) {
                        guild.update(update);
                    }
                    self.rebuild_rail();
                    self.permissions_changed(Some(id));
                }
                Event::GuildEmojisUpdate { guild_id, emojis } => {
                    if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == guild_id) {
                        guild.emojis = emojis;
                    }
                }
                Event::GuildStickersUpdate { guild_id, stickers } => {
                    if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == guild_id) {
                        guild.stickers = stickers;
                    }
                }
                Event::RoleUpdate { guild_id, role } => {
                    if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == guild_id) {
                        guild.set_role(role);
                    }
                    self.permissions_changed(Some(guild_id));
                }
                Event::RoleDelete { guild_id, role_id } => {
                    if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == guild_id) {
                        guild.roles.retain(|r| r.id != role_id);
                    }
                    self.permissions_changed(Some(guild_id));
                }
                Event::AccountUpdate(account, nitro) => {
                    {
                        let mut state = self.account.state.borrow_mut();
                        state.account = Some(account);
                        state.nitro = nitro;
                    }
                    // 2FA turned on or off changes moderators' permissions.
                    self.permissions_changed(None);
                    // Nitro changes the length limit.
                    self.update_length_indicator();
                }
                Event::TypingStart { channel_id, user_id, user } => self.on_typing(channel_id, user_id, user.as_ref()),
                Event::MessageCreate(message) => self.on_message(*message),
                Event::MessageUpdate(edit) => self.on_message_update(edit),
                Event::Interaction { nonce, outcome, reason } => self.on_interaction(nonce, outcome, reason),
                Event::InteractionCreated { nonce } => self.command_created(nonce),
                Event::Autocomplete { nonce, choices } => self.on_autocomplete(nonce, &choices),
                Event::CommandIndexChanged { guild, version } => self.command_index_changed(guild, version),
                Event::UserApplicationChanged => self.account.state.borrow_mut().commands.user_changed(),
                Event::MessageDelete { id, channel_id } => self.on_message_delete(id, channel_id),
                Event::MessageDeleteBulk { ids, channel_id } => self.on_message_delete_bulk(ids, channel_id),
                Event::PollVote { channel_id, message_id, votes, added, many } => {
                    self.on_poll_vote(channel_id, message_id, votes, added, many)
                }
                Event::Reaction(reacted) => self.on_reaction(&reacted),
                Event::ReactionsAdded { channel_id, message_id, reactions } => {
                    self.on_reactions_added(channel_id, message_id, &reactions)
                }
                Event::ReactionsCleared { channel_id, message_id } => {
                    self.on_reactions_cleared(channel_id, message_id, None)
                }
                Event::EmojiReactionsCleared { channel_id, message_id, emoji } => {
                    self.on_reactions_cleared(channel_id, message_id, Some(&emoji))
                }
                Event::MessageAck { channel_id, message_id, manual } => {
                    match manual {
                        Some(mentions) => {
                            self.account.state.borrow_mut().read.mark_unread(channel_id, message_id, mentions)
                        }
                        None => self.account.state.borrow_mut().read.ack(channel_id, message_id),
                    }
                    self.forum_read(channel_id);
                    self.withdraw_notification(channel_id);
                    self.refresh_badges();
                }
                Event::ChannelUnreadUpdate { channels, .. } => {
                    {
                        let mut state = self.account.state.borrow_mut();
                        for (channel, last) in channels {
                            state.bump_last_message(channel, last);
                        }
                    }
                    self.refresh_badges();
                }
                Event::RequiredActionUpdate(action) => {
                    self.account.state.borrow_mut().required_action = action;
                    self.apply_required_action();
                }
                Event::SettingsUpdate { patch, replace, proto } => {
                    if let Some(update) = proto {
                        self.account.state.borrow_mut().sync.received(&update, !replace);
                    }
                    self.apply_settings(patch, replace);
                }
                Event::FrecencyUpdate { proto, partial } => self.frecency_updated(&proto, partial),
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
                    let was_blocked = self.account.state.borrow().relationships.is_blocked(relationship.user.id);
                    self.account.state.borrow_mut().profiles.doubt(relationship.user.id);
                    self.account.state.borrow_mut().relationships.add(relationship);
                    self.relationships_changed(blocked != was_blocked);
                }
                Event::RelationshipUpdate { user_id, nickname } => {
                    self.account.state.borrow_mut().profiles.doubt(user_id);
                    self.account.state.borrow_mut().relationships.update(user_id, nickname);
                    self.relationships_changed(false);
                    // What we call them shows in DMs.
                    self.refresh_rows();
                }
                Event::RelationshipRemove(user_id) => {
                    self.withdraw_friend_request(user_id);
                    let was_blocked = self.account.state.borrow().relationships.is_blocked(user_id);
                    self.account.state.borrow_mut().profiles.doubt(user_id);
                    self.account.state.borrow_mut().relationships.remove(user_id);
                    self.relationships_changed(was_blocked);
                }
                Event::UserNoteUpdate { user_id, note } => self.on_note(user_id, note),
                Event::PresenceUpdate { user_id, guild_id, presence } => {
                    self.on_list_presence(guild_id, user_id, &presence);
                    if self.account.state.borrow_mut().relationships.set_presence(user_id, presence) {
                        self.queue_friends_refresh();
                        // A group DM's friends.
                        self.refresh_member_rows();
                        self.refresh_profile_views();
                    }
                }
                Event::NotificationSettingsUpdate(settings) => {
                    self.account.state.borrow_mut().notifications.update(settings);
                    self.refresh_badges();
                }
                Event::Resumed => {
                    {
                        let mut state = self.account.state.borrow_mut();
                        state.sync.disconnected();
                        state.frecency_sync.disconnected();
                    }
                    self.usage_connected();
                    self.account.timelines.borrow_mut().held.resumed();
                    self.account.state.borrow_mut().members.resumed();
                    self.send_member_requests();
                    self.subscribe_session(false);
                    self.resend_presence();
                    let name = {
                        let mut state = self.account.state.borrow_mut();
                        state.reconnect_attempts = 0;
                        state.me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default()
                    };
                    self.me_label.set_label(&name);
                    // Replayed messages have already updated the open channel.
                }
                Event::Closed { code, reason, resume } => {
                    // A rejected token has nothing to save to.
                    if !Event::is_fatal_close(code) {
                        self.usage_closed();
                    }
                    {
                        let mut state = self.account.state.borrow_mut();
                        state.sync.disconnected();
                        state.frecency_sync.disconnected();
                    }
                    self.account.timelines.borrow_mut().held.closed();
                    {
                        let mut state = self.account.state.borrow_mut();
                        let open = state.guild;
                        state.subscriptions.closed(open);
                        state.resume = resume;
                    }
                    if Event::is_fatal_close(code) {
                        // The account stays on the list, to log in again.
                        let (id, migrating) = {
                            let state = self.account.state.borrow();
                            (state.known_id(), state.migrating)
                        };
                        self.leave_account();
                        let this = self.clone();
                        glib::spawn_future_local(async move {
                            if let Some(id) = id {
                                this.roster.borrow_mut().validated(id, Answer::NoToken);
                                this.refresh_account_views();
                                let _ = session::clear_token(id).await;
                            }
                            if migrating {
                                let _ = session::clear_legacy_token().await;
                            }
                        });
                        self.show_signed_out(Some(&format!(
                            "Discord rejected the token (close code {code}). Log in again."
                        )));
                    } else if self.account.state.borrow().token.is_some() {
                        let why = if reason.is_empty() { format!("code {code}") } else { reason };
                        self.connection_lost(&why);
                    }
                    return;
                }
            }
        }
    }

    pub(super) fn on_ready(self: &Rc<Self>, ready: Ready) {
        // A new session: what we hold for each channel may have missed events.
        self.account.timelines.borrow_mut().held.opened();
        self.forum_new_session();
        self.account.state.borrow_mut().reactors.clear();
        let Ready {
            user,
            account,
            nitro,
            guilds,
            members,
            private_channels,
            relationships,
            read_states,
            notification_settings,
            collapsed,
            settings,
            settings_proto,
            media_takedown_regulated,
            required_action,
            experiments,
            session_id,
            gates,
            unavailable_guilds,
            voice_states,
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
        let (store, was_ready, offline_save) = {
            let mut state = self.account.state.borrow_mut();
            let was_ready = state.ready_seen;
            state.ready_seen = true;
            state.reconnect_attempts = 0;
            state.guilds = guilds;
            // What a session loaded of threads goes with it.
            state.archives.clear();
            state.fetched_threads.clear();
            // A new session: what we asked for before is forgotten, as in the client.
            state.members = members;
            state.private_channels = private_channels;
            state.relationships = relationships;
            state.notes.clear();
            state.typing.clear();
            state.read = read_states;
            state.notifications = notification_settings;
            state.collapsed = collapsed;
            self.images.motion().set_media(settings.media);
            state.settings = settings;
            let offline_save = settings_proto.and_then(|proto| state.sync.connected(proto));
            state.media_takedown_regulated = media_takedown_regulated;
            state.required_action = required_action;
            state.account = Some(account);
            state.nitro = nitro;
            state.experiments = experiments;
            state.session_id = session_id;
            state.gates = gates;
            state.unavailable_guilds = unavailable_guilds;
            state.voice = voice_states;
            // A new session: every index is out of date, and no command is waiting on this one.
            state.commands.session_started();
            state.running.clear();
            let store = (!state.persisted || state.migrating).then(|| state.token.clone());
            let migrated = std::mem::take(&mut state.migrating);
            state.persisted = true;
            state.users.insert(user.id.0, user.display_name().to_string());
            state.me = Some(user.clone());
            ((store.flatten(), migrated), was_ready, offline_save)
        };
        self.remember_account(&user, store.0, store.1);

        self.settings_replaced();
        self.status_ready();
        if let Some(schedule) = offline_save {
            self.save_settings_after(schedule);
        }
        // What was used and not saved: a closed app's, and a lost connection's edit.
        self.restore_usage();
        let offline_frecency = self.account.state.borrow_mut().frecency_sync.reconnected();
        if let Some(schedule) = offline_frecency {
            self.save_frecency_after(schedule);
        }
        self.usage_connected();
        let me = self.account.state.borrow().me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default();
        self.me_label.set_label(&me);
        // READY may bring other Nitro, which changes the length limit.
        self.update_length_indicator();
        self.refresh_typing();
        // Not out from under a login that adds another account.
        if self.login_origin.get() != Origin::Beside || !self.on_login_page() {
            self.stack.set_visible_child_name("main");
        }
        self.apply_required_action();
        // A new session subscribes to nothing: say again what we look at.
        self.subscribe_session(true);

        // Stay on a Home page across reconnects.
        let page = self.account.state.borrow().page.filter(|_| was_ready);
        match page {
            Some(Page::Friends) => return self.show_friends(),
            Some(Page::MessageRequests) => return self.show_requests(),
            None => {}
        }

        // Keep the open guild (or Home) across reconnects; on first connect
        // open the first guild, or Home if there are none.
        let open = {
            let state = self.account.state.borrow();
            match state.guild {
                Some(id) if state.guilds.iter().any(|g| g.id == id) => Some(id),
                None if was_ready => None,
                _ => state.first_guild(),
            }
        };
        // After a reconnect that could not resume, stay in the open channel.
        let keep = {
            let state = self.account.state.borrow();
            state.channel.filter(|&c| {
                was_ready && state.can_view(c) && state.find_channel(c).is_some_and(|(g, _)| g.map(|g| g.id) == open)
            })
        };
        match (keep, open) {
            (Some(channel), _) => {
                self.account.state.borrow_mut().guild = open;
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
            let mut state = self.account.state.borrow_mut();
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
        if open {
            self.refresh_member_rows();
        }
    }

    pub(super) fn on_private_channel_delete(self: &Rc<Self>, id: Snowflake) {
        let (home, was_open, was_request, requests_left) = {
            let mut state = self.account.state.borrow_mut();
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
            self.update_notice_bar();
        }
        if home {
            self.rebuild_sidebar();
        }
        if self.account.state.borrow().page == Some(Page::MessageRequests) {
            self.rebuild_requests();
        }
        self.refresh_badges();
    }

    /// After a relationship changed: badges, the friends page, and, if
    /// someone was blocked or unblocked, the messages on screen.
    fn relationships_changed(self: &Rc<Self>, blocks_changed: bool) {
        self.refresh_badges();
        self.queue_friends_refresh();
        self.refresh_profile_views();
        if blocks_changed {
            self.refresh_typing();
            self.refresh_rows();
        }
    }

    pub(super) fn on_guild(self: &Rc<Self>, guild: Guild) {
        let id = guild.id;
        let is_open = {
            let mut state = self.account.state.borrow_mut();
            match state.guilds.iter_mut().find(|g| g.id == id) {
                Some(existing) => *existing = guild,
                None => state.guilds.push(guild),
            }
            state.guild == Some(id)
        };
        self.rebuild_rail();
        if is_open {
            self.rebuild_sidebar();
            // As the client does when the open server arrives (`GUILD_CREATE`).
            self.subscribe_open();
        }
        self.permissions_changed(Some(id));
    }

    /// A server channel changed: its name, topic, place, slowmode, or who
    /// may do what there.
    pub(super) fn on_guild_channel(self: &Rc<Self>, guild_id: Snowflake, channel: Channel) {
        let guild_open = {
            let mut state = self.account.state.borrow_mut();
            let state = &mut *state;
            let Some(guild) = state.guilds.iter_mut().find(|g| g.id == guild_id) else { return };
            let Some(existing) = guild.channels.iter_mut().find(|c| c.id == channel.id) else { return };
            state.slowmode.update(&channel, now_ms());
            *existing = channel;
            state.guild == Some(guild_id)
        };
        if guild_open {
            self.rebuild_sidebar();
        }
        self.permissions_changed(Some(guild_id));
    }

    /// Threads in a server changed, as `change` makes them: created,
    /// renamed, archived, locked, joined or left. Whether we may post in one,
    /// or still see it, may have changed with them.
    pub(super) fn on_threads(self: &Rc<Self>, guild_id: Snowflake, change: impl FnOnce(&mut Guild)) {
        let open = {
            let mut state = self.account.state.borrow_mut();
            let Some(guild) = state.guilds.iter_mut().find(|g| g.id == guild_id) else { return };
            change(guild);
            state.guild == Some(guild_id)
        };
        if open {
            self.rebuild_sidebar();
            self.refresh_thread_boxes();
            self.sync_threads_button();
        }
        self.refresh_threads_view();
        self.permissions_changed(Some(guild_id));
        self.thread_arrivals();
    }

    /// A thread was deleted. If it was open, its parent opens instead.
    fn on_thread_delete(self: &Rc<Self>, guild_id: Snowflake, id: Snowflake) {
        let (parent, gone) = {
            let mut state = self.account.state.borrow_mut();
            let Some(guild) = state.guilds.iter_mut().find(|g| g.id == guild_id) else { return };
            let gone = guild.thread(id).cloned();
            let parent = gone.as_ref().and_then(|t| t.parent_id);
            guild.threads.retain(|t| t.id != id);
            (parent.filter(|_| state.channel == Some(id)), gone)
        };
        if let Some(gone) = gone {
            self.forum_thread_deleted(&gone);
        }
        match parent {
            Some(parent) if self.account.state.borrow().can_view(parent) => self.select_channel(parent),
            _ => self.on_threads(guild_id, |_| {}),
        }
    }

    /// Our member in a server changed: a timeout began or was lifted, our
    /// roles changed, or screening let us in.
    pub(super) fn on_member(self: &Rc<Self>, guild_id: Snowflake, member: Member) {
        if let Some(guild) = self.account.state.borrow_mut().guilds.iter_mut().find(|g| g.id == guild_id) {
            guild.set_member(member);
        }
        self.permissions_changed(Some(guild_id));
    }

    /// Ask the gateway for the members [`discord::members::Members`] lacks.
    pub(super) fn send_member_requests(&self) {
        let state = &mut *self.account.state.borrow_mut();
        let requests = state.members.flush();
        if let Some(gateway) = &state.gateway {
            for (guild, users) in requests {
                gateway.request_members(guild, &users);
            }
        }
    }

    /// What our permissions in `guild` (or, with `None`, any server) come
    /// from changed: its roles or owner, our member, a channel's overwrites,
    /// our 2FA. Channels we lost sight of go, and with them the open one;
    /// the message box and Reply follow what we may still do.
    pub(super) fn permissions_changed(self: &Rc<Self>, guild: Option<Snowflake>) {
        self.refresh_badges();
        let (open, viewable, replyable, editable) = {
            let state = self.account.state.borrow();
            let open = state.guild.filter(|&open| guild.is_none_or(|id| id == open));
            let channel = state.channel.filter(|_| state.page.is_none());
            let reply = self.reply.borrow().as_ref().map(|r| r.channel);
            let editable = self.editing.borrow().as_ref().is_none_or(|e| state.can_edit(&e.message));
            (open, channel.is_some_and(|c| state.can_view(c)), reply.is_none_or(|c| state.can_reply(c)), editable)
        };
        let Some(open) = open else { return };
        if !replyable {
            self.set_reply(None);
        }
        if !editable {
            self.end_edit();
        }
        if !viewable {
            // Elsewhere, or a first channel we can now see.
            return self.select_guild(open);
        }
        self.update_notice_bar();
        self.refresh_rows();
        self.sync_members();
        self.refresh_member_rows();
    }
}
