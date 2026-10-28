//! Everything the UI knows about the session, and the derived views of it
//! the rail and sidebar draw from: unread markers, mention counts, order.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use discord::cooldowns::Slowmode;
use discord::members::{Members, RoleColour};
use discord::notifications::{NotificationSettings, Place};
use discord::permissions;
use discord::read_state::ReadStates;
use discord::relationships::Relationships;
use discord::settings::{GuildFolder, MediaSettings, RailEntry};
use discord::verification::Account;
use discord::{Channel, Gateway, Guild, Rest, Resume, Snowflake, User};
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

    /// The guild a channel belongs to and the channel itself; `None` guild for DMs.
    pub(super) fn find_channel(&self, id: Snowflake) -> Option<(Option<&Guild>, &Channel)> {
        if let Some(dm) = self.private_channels.iter().find(|c| c.id == id) {
            return Some((None, dm));
        }
        self.guilds.iter().find_map(|g| g.channels.iter().find(|c| c.id == id).map(|c| (Some(g), c)))
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

    /// Whether we may post in the channel (see [`permissions::can_send`]).
    pub(super) fn can_send(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_send(guild, channel, me, now_unix()),
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

    /// The colour of `user`'s name in `channel`: their highest coloured role,
    /// in a server.
    pub(super) fn name_colour(&self, channel: Snowflake, user: Snowflake) -> Option<RoleColour> {
        let guild = self.find_channel(channel)?.0?;
        let roles =
            if self.me() == Some(user) { Some(&guild.my_roles[..]) } else { self.members.roles(guild.id, user) };
        guild.member_colour(roles?)
    }

    /// The server's text channels we can view, in sidebar order.
    pub(super) fn sidebar<'a>(&self, guild: &'a Guild) -> Vec<(Option<&'a str>, Vec<&'a Channel>)> {
        self.account.as_ref().map(|me| guild.sidebar(me, now_unix())).unwrap_or_default()
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
            Some(guild) => self
                .visible_text(guild)
                .map(|c| (Place { guild: Some(guild.id), guild_default: 0, channel: c.id, parent: c.parent_id }, c))
                .collect(),
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
            .chain(self.guilds.iter_mut().flat_map(|g| g.channels.iter_mut()))
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
