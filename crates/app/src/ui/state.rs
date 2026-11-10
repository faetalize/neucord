//! Everything the UI knows about the session, and the derived views of it
//! the rail and sidebar draw from: unread markers, mention counts, order.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use discord::channel_icon;
use discord::cooldowns::Slowmode;
use discord::experiments::Experiments;
use discord::limits::{Limits, Nitro};
use discord::members::{Members, RoleColour};
use discord::notifications::{NotificationSettings, Place};
use discord::permissions;
use discord::read_state::ReadStates;
use discord::relationships::Relationships;
use discord::settings::{GuildFolder, MediaSettings, RailEntry};
use discord::threads;
use discord::verification::Account;
use discord::{Channel, Gateway, Guild, Message, Rest, Resume, Snowflake, User};
use gtk::glib;

use super::friends::Tab;

/// What Home shows in place of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Friends,
    MessageRequests,
}

/// Where the unread messages of a channel started when it was opened, for
/// the "New" divider and the offer to jump there.
#[derive(Clone, Copy, Debug)]
pub(super) struct Unread {
    pub(super) channel: Snowflake,
    /// The last message read then.
    pub(super) acked: Snowflake,
    /// The first unread message has been in view.
    pub(super) seen: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) token: Option<String>,
    /// The token is already in the keyring.
    pub(super) persisted: bool,
    /// READY arrived at least once for this token.
    pub(super) ready_seen: bool,
    pub(super) reconnect_attempts: u32,
    /// Set while the last session can be resumed; kept across failed
    /// attempts, replaced by each close.
    pub(super) resume: Option<Resume>,
    pub(super) rest: Option<Rest>,
    pub(super) gateway: Option<Gateway>,
    pub(super) me: Option<User>,
    /// What servers' verification asks of us.
    pub(super) account: Option<Account>,
    pub(super) nitro: Nitro,
    /// What Discord rolls out to the account and its servers.
    pub(super) experiments: Experiments,
    /// A step Discord requires before the account can be used.
    pub(super) required_action: Option<String>,
    pub(super) guilds: Vec<Guild>,
    /// DMs and group DMs, message requests included.
    pub(super) private_channels: Vec<Channel>,
    pub(super) relationships: Relationships,
    /// The user's server folders, which also order the rail.
    pub(super) guild_folders: Vec<GuildFolder>,
    /// Folders the user expanded in the rail (by folder id).
    pub(super) open_folders: std::collections::HashSet<u64>,
    /// The open guild; `None` is Home, the DM list.
    pub(super) guild: Option<Snowflake>,
    pub(super) channel: Option<Snowflake>,
    /// Set while Home shows one of its pages; then no channel is open.
    pub(super) page: Option<Page>,
    pub(super) friends_tab: Tab,
    /// Runs of blocked users' messages the user chose to see, by the run's
    /// first message.
    pub(super) revealed_blocked: HashSet<Snowflake>,
    /// What was unread in the open channel when it opened.
    pub(super) unread: Option<Unread>,
    /// Spoilers revealed in each message's text, by number.
    pub(super) revealed_spoilers: HashMap<Snowflake, HashSet<usize>>,
    /// Display names by user id, accumulated from every user we see plus lazy
    /// `GET /users/{id}` lookups, so mentions resolve even for people no longer
    /// in the server.
    pub(super) users: HashMap<u64, String>,
    /// User ids we have already tried to fetch, to avoid repeat requests.
    pub(super) fetched_users: std::collections::HashSet<u64>,
    pub(super) read: ReadStates,
    pub(super) notifications: NotificationSettings,
    /// How embeds and media show.
    pub(super) media: MediaSettings,
    /// Menus offer to copy ids.
    pub(super) developer_mode: bool,
    /// The message menu's last groups come in the order the client uses
    /// where the media takedown regulation is TIDA.
    pub(super) media_takedown_regulated: bool,
    /// Compact message display, which sizes history pages.
    pub(super) message_display_compact: bool,
    /// Acks waiting to be sent: channel → newest message read.
    pub(super) pending_acks: HashMap<Snowflake, Snowflake>,
    /// How long until we may post again in each channel.
    pub(super) slowmode: Slowmode,
    /// Other people's roles in each server.
    pub(super) members: Members,
}

impl State {
    pub(super) fn rail_layout(&self) -> Vec<RailEntry> {
        let ids: Vec<Snowflake> = self.guilds.iter().map(|g| g.id).collect();
        discord::settings::rail_layout(&self.guild_folders, &ids)
    }

    /// The first server in rail order, opened on first connect.
    pub(super) fn first_guild(&self) -> Option<Snowflake> {
        self.rail_layout().into_iter().find_map(|entry| match entry {
            RailEntry::Guild(id) => Some(id),
            RailEntry::Folder { guilds, .. } => guilds.first().copied(),
        })
    }

    /// The DM list: private channels that are not message requests.
    pub(super) fn dms(&self) -> impl Iterator<Item = &Channel> {
        self.private_channels.iter().filter(|c| !c.is_request())
    }

    /// Message requests and spam, newest first.
    pub(super) fn requests(&self) -> Vec<&Channel> {
        let mut requests: Vec<&Channel> = self.private_channels.iter().filter(|c| c.is_request()).collect();
        requests.sort_by_key(|c| std::cmp::Reverse(c.activity()));
        requests
    }

    /// Message requests not filed as spam: what the requests badge counts.
    pub(super) fn waiting_requests(&self) -> u32 {
        self.private_channels.iter().filter(|c| c.message_request && !c.spam).count() as u32
    }

    /// The open channel, if it is a message request.
    pub(super) fn open_request(&self) -> Option<&Channel> {
        let id = self.channel?;
        self.private_channels.iter().find(|c| c.id == id && c.is_request())
    }

    pub(super) fn me(&self) -> Option<Snowflake> {
        self.me.as_ref().map(|u| u.id)
    }

    /// The guild a channel or thread belongs to and the channel itself;
    /// `None` guild for DMs.
    pub(super) fn find_channel(&self, id: Snowflake) -> Option<(Option<&Guild>, &Channel)> {
        if let Some(dm) = self.private_channels.iter().find(|c| c.id == id) {
            return Some((None, dm));
        }
        self.guilds.iter().find_map(|g| g.find(id).map(|c| (Some(g), c)))
    }

    /// Whether the channel shows at all: DMs always, server channels with
    /// View Channel. Neucord opens nothing else, since Discord refuses us
    /// there.
    pub(super) fn can_view(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((None, _)), _) => true,
            (Some((Some(guild), channel)), Some(me)) => guild.can_view(channel, me, now_unix()),
            _ => false,
        }
    }

    /// What Nitro, boosts and experiments allow us; before READY, what they
    /// allow an account without them.
    pub(super) fn limits(&self) -> Limits<'_> {
        Limits {
            nitro: &self.nitro,
            experiments: &self.experiments,
            me: self.me.as_ref().map_or(Snowflake(0), |u| u.id),
        }
    }

    /// Whether we may post in the channel (see [`permissions::can_send`]).
    pub(super) fn can_send(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_send(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether files may be attached in the channel (see [`permissions::can_attach`]).
    pub(super) fn can_attach(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_attach(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether Reply is offered in the channel (see [`permissions::can_reply`]).
    pub(super) fn can_reply(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_reply(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether we may edit `message` (see [`permissions::can_edit`]).
    pub(super) fn can_edit(&self, message: &Message) -> bool {
        match (self.find_channel(message.channel_id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_edit(guild, channel, message, me, now_unix()),
            _ => false,
        }
    }

    /// Whether we may delete `message` (see [`permissions::can_delete`]).
    pub(super) fn can_delete(&self, message: &Message) -> bool {
        match (self.find_channel(message.channel_id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_delete(guild, channel, message, me, now_unix()),
            _ => false,
        }
    }

    /// Whether an edit or delete in the channel reopens it first (see
    /// [`permissions::unarchives_first`]).
    pub(super) fn unarchives_first(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::unarchives_first(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// The colour of `user`'s name in `channel`: their highest coloured role,
    /// in a server.
    pub(super) fn name_colour(&self, channel: Snowflake, user: Snowflake) -> Option<RoleColour> {
        let guild = self.find_channel(channel)?.0?;
        let roles =
            if self.me() == Some(user) { Some(&guild.my_roles[..]) } else { self.members.roles(guild.id, user) };
        guild.member_colour(roles?)
    }

    /// The server's channels we can view, in sidebar order.
    pub(super) fn sidebar<'a>(&self, guild: &'a Guild) -> Vec<(Option<&'a str>, Vec<&'a Channel>)> {
        self.account.as_ref().map(|me| guild.sidebar(me, now_unix())).unwrap_or_default()
    }

    /// The threads the sidebar lists under `parent` (see [`threads::listed`]).
    pub(super) fn sidebar_threads<'a>(&self, guild: &'a Guild, parent: &Channel) -> Vec<&'a Channel> {
        let Some(me) = &self.account else { return Vec::new() };
        let place = Place {
            guild: Some(guild.id),
            guild_default: guild.default_message_notifications,
            channel: parent.id,
            parent: parent.parent_id,
        };
        let muted = self.notifications.channel_muted(&place, now_unix());
        let listed = threads::listed(guild, parent, self.channel, muted, &self.read, now_ms());
        listed.into_iter().filter(|t| guild.can_view(t, me, now_unix())).collect()
    }

    /// A thread we muted.
    pub(super) fn thread_muted(thread: &Channel) -> bool {
        thread.member.as_ref().is_some_and(|m| m.is_muted(now_unix()))
    }

    /// The server's text channels we can view, in no order.
    fn visible_text<'a>(&'a self, guild: &'a Guild) -> impl Iterator<Item = &'a Channel> {
        let now = now_unix();
        guild
            .channels
            .iter()
            .filter(move |c| c.kind.is_text() && self.account.as_ref().is_some_and(|me| guild.can_view(c, me, now)))
    }

    pub(super) fn place(&self, id: Snowflake) -> Option<Place> {
        let (guild, channel) = self.find_channel(id)?;
        Some(Place {
            guild: guild.map(|g| g.id),
            guild_default: guild.map_or(0, |g| g.default_message_notifications),
            channel: channel.id,
            parent: channel.parent_id,
        })
    }

    pub(super) fn is_unread(&self, channel: &Channel) -> bool {
        if channel.kind.is_thread() {
            let guild = self.guilds.iter().find(|g| g.thread(channel.id).is_some());
            return guild.is_some_and(|g| threads::is_unread(g, channel, &self.read));
        }
        self.read.is_unread(channel.id, channel.last_message_id)
    }

    /// Unread dot and mention count for a server's rail icon. Muted channels
    /// (and a muted server) add mentions but no dot, as in Discord.
    pub(super) fn guild_badge(&self, guild: &Guild) -> (bool, u32) {
        let now = now_unix();
        let guild_muted = self.notifications.guild_muted(guild.id, now);
        let mut unread = false;
        let mut mentions = 0;
        // Channels we can't view count for nothing, as in the client.
        for channel in self.visible_text(guild) {
            mentions += self.read.mentions(channel.id);
            if !unread && !guild_muted && self.is_unread(channel) {
                let place =
                    Place { guild: Some(guild.id), guild_default: 0, channel: channel.id, parent: channel.parent_id };
                unread = !self.notifications.channel_muted(&place, now);
            }
        }
        // Threads we've joined count too, unless muted, whatever their parent.
        for thread in threads::joined_active(guild) {
            mentions += self.read.mentions(thread.id);
            unread = unread || (!guild_muted && !Self::thread_muted(thread) && self.is_unread(thread));
        }
        (unread, mentions)
    }

    /// Everything the rail shows that can change without the guild list changing.
    /// What the rail is built from: its servers and folders, in order, with
    /// their names and icons. Markers aside, it changes rarely.
    pub(super) fn rail_shape(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for guild in &self.guilds {
            (guild.id, &guild.name, &guild.icon).hash(&mut hasher);
        }
        for entry in self.rail_layout() {
            match entry {
                RailEntry::Guild(id) => id.hash(&mut hasher),
                RailEntry::Folder { id, name, color, guilds } => {
                    (id, name, color, guilds, self.open_folders.contains(&id)).hash(&mut hasher)
                }
            }
        }
        hasher.finish()
    }

    pub(super) fn rail_signature(&self) -> Vec<u64> {
        let mut sig = vec![self.guild.map_or(0, |g| g.0), self.home_badge() as u64];
        for guild in &self.guilds {
            let (unread, mentions) = self.guild_badge(guild);
            sig.extend([guild.id.0, unread as u64, mentions as u64]);
        }
        sig
    }

    /// The same for the sidebar: the open place, selection, order and markers.
    pub(super) fn sidebar_signature(&self) -> Vec<u64> {
        let now = now_unix();
        let mut sig = vec![self.guild.map_or(0, |g| g.0), self.channel.map_or(0, |c| c.0)];
        let channels: Vec<(Place, &Channel)> = match self.guild.and_then(|id| self.guilds.iter().find(|g| g.id == id)) {
            Some(guild) => {
                let mut rows = Vec::new();
                for (_, channels) in self.sidebar(guild) {
                    for c in channels {
                        rows.push((
                            Place { guild: Some(guild.id), guild_default: 0, channel: c.id, parent: c.parent_id },
                            c,
                        ));
                        let mut icon = std::collections::hash_map::DefaultHasher::new();
                        channel_icon::of(guild, c).hash(&mut icon);
                        sig.push(icon.finish());
                        for t in self.sidebar_threads(guild, c) {
                            // A muted thread shows as muted; its place stands for that.
                            sig.extend([t.id.0, Self::thread_muted(t) as u64]);
                            rows.push((
                                Place { guild: Some(guild.id), guild_default: 0, channel: t.id, parent: None },
                                t,
                            ));
                        }
                    }
                }
                rows
            }
            None => {
                let page = self.page.map_or(0, |p| p as u64 + 1);
                let (requests, waiting) = (self.requests().len(), self.waiting_requests());
                sig.extend([page, self.relationships.incoming() as u64, requests as u64, waiting as u64]);
                sig.push(self.open_request().is_some() as u64);
                let mut dms: Vec<&Channel> = self.dms().collect();
                dms.sort_by_key(|c| std::cmp::Reverse(c.activity()));
                dms.into_iter()
                    .map(|c| (Place { guild: None, guild_default: 0, channel: c.id, parent: None }, c))
                    .collect()
            }
        };
        for (place, channel) in channels {
            sig.extend([
                channel.id.0,
                self.is_unread(channel) as u64,
                self.notifications.channel_muted(&place, now) as u64,
                self.read.mentions(channel.id) as u64,
            ]);
        }
        sig
    }

    /// Home's badge: every DM message counts as a mention, and so does a
    /// friend request.
    pub(super) fn home_badge(&self) -> u32 {
        self.dms().map(|c| self.read.mentions(c.id)).sum::<u32>() + self.relationships.incoming()
    }

    /// Record a new newest message for a channel wherever it lives.
    pub(super) fn bump_last_message(&mut self, channel: Snowflake, message: Snowflake) {
        let slot = self
            .private_channels
            .iter_mut()
            .chain(self.guilds.iter_mut().flat_map(|g| g.channels.iter_mut().chain(g.threads.iter_mut())))
            .find(|c| c.id == channel);
        if let Some(c) = slot
            && c.last_message_id.is_none_or(|last| message > last)
        {
            c.last_message_id = Some(message);
        }
    }
}

pub(super) fn now_unix() -> i64 {
    glib::real_time() / 1_000_000
}

pub(super) fn now_ms() -> i64 {
    glib::real_time() / 1_000
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// An open server with one text channel, `rules` naming its rules channel.
    fn open_server(rules: Option<u64>, channel: serde_json::Value) -> State {
        let mut channel_json = json!({"id": "9", "type": 0, "name": "general"});
        channel_json.as_object_mut().unwrap().extend(channel.as_object().unwrap().clone());
        let mut guild: Guild = serde_json::from_value(json!({
            "id": "1", "name": "G", "rules_channel_id": rules.map(|id| id.to_string()),
            "roles": [{"id": "40", "name": "Member", "permissions": "0"}],
            "channels": [channel_json],
        }))
        .unwrap();
        guild.my_roles = vec![Snowflake(40)];
        State {
            account: Some(Account { id: Snowflake(5), ..Account::default() }),
            guild: Some(guild.id),
            guilds: vec![guild],
            ..State::default()
        }
    }

    #[test]
    fn the_sidebar_is_drawn_again_when_a_channels_icon_changes() {
        let plain = open_server(None, json!({})).sidebar_signature();
        assert_ne!(plain, open_server(Some(9), json!({})).sidebar_signature(), "it became the rules channel");
        assert_ne!(plain, open_server(None, json!({"nsfw": true})).sidebar_signature(), "it was age-restricted");
        // Our role sees it, `@everyone` doesn't.
        let hidden = json!({"permission_overwrites": [
            {"id": "1", "type": 0, "deny": "1024", "allow": "0"},
            {"id": "40", "type": 0, "deny": "0", "allow": "1024"},
        ]});
        assert_ne!(plain, open_server(None, hidden).sidebar_signature(), "it was hidden from @everyone");
        assert_eq!(plain, open_server(None, json!({})).sidebar_signature(), "nothing changed");
    }
}
