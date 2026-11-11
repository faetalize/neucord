//! Everything the UI knows about the session, and the derived views of it
//! the rail and sidebar draw from: unread markers, mention counts, order.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use discord::channel_icon;
use discord::cooldowns::Slowmode;
use discord::experiments::Experiments;
use discord::gifs::Tiles;
use discord::limits::{Limits, Nitro};
use discord::member_list::{self, MemberLists};
use discord::members::{Members, RoleColour};
use discord::notice::Notice;
use discord::notifications::{NotificationSettings, Place};
use discord::permissions;
use discord::post_creation;
use discord::read_state::ReadStates;
use discord::relationships::Relationships;
use discord::settings::{RailEntry, Settings};
use discord::stickers::Packs;
use discord::subscriptions::Subscriptions;
use discord::thread_creation;
use discord::threads;
use discord::typing::Typing;
use discord::verification;
use discord::verification::Account;
use discord::voice_states::{VoiceState, VoiceStates};
use discord::{Channel, ChannelKind, Gateway, Guild, Message, Rest, Resume, Snowflake, User};
use gtk::glib;

use super::friends::Tab;

/// What Home shows in place of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Friends,
    MessageRequests,
}

/// Whose list the member list pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// The server's members, from the list `list` of `channel` (see
    /// [`member_list::list_id`]).
    Server { guild: Snowflake, channel: Snowflake, list: String },
    /// A group DM's, which its channel holds.
    Group(Snowflake),
    /// The profile of the person a DM is with, which has no list.
    Dm(Snowflake),
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
    /// Discord's standard sticker packs, once the picker has fetched them.
    pub(super) sticker_packs: Packs,
    /// The GIF picker's front page, once it has been fetched.
    pub(super) gif_tiles: Tiles,
    /// DMs and group DMs, message requests included.
    pub(super) private_channels: Vec<Channel>,
    pub(super) relationships: Relationships,
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
    /// This session's id, which interactions name.
    pub(super) session_id: String,
    /// Where each interaction of ours that Discord hasn't answered belongs:
    /// its channel and message, by nonce.
    pub(super) interactions: HashMap<Snowflake, (Snowflake, Snowflake)>,
    /// The apps and commands fetched for slash commands.
    pub(super) commands: discord::commands::CommandIndex,
    /// Commands we ran that their app hasn't answered, by nonce.
    pub(super) running: HashMap<Snowflake, super::slash::Running>,
    /// Display names by user id, accumulated from every user we see plus lazy
    /// `GET /users/{id}` lookups, so mentions resolve even for people no longer
    /// in the server.
    pub(super) users: HashMap<u64, String>,
    /// User ids we have already tried to fetch, to avoid repeat requests.
    pub(super) fetched_users: std::collections::HashSet<u64>,
    /// Who reacted with what, as far as we know.
    pub(super) reactors: discord::reactions::Reactors,
    /// The profiles popouts asked for, and our notes on people.
    pub(super) profiles: discord::profile::Profiles,
    pub(super) notes: discord::notes::Notes,
    pub(super) read: ReadStates,
    pub(super) notifications: NotificationSettings,
    /// The account's client settings: server folders (which also order the
    /// rail), how media shows, Developer Mode, compact display.
    pub(super) settings: Settings,
    /// The message menu's last groups come in the order the client uses
    /// where the media takedown regulation is TIDA.
    pub(super) media_takedown_regulated: bool,
    /// Acks waiting to be sent: channel → newest message read.
    pub(super) pending_acks: HashMap<Snowflake, Snowflake>,
    /// How long until we may post again in each channel.
    pub(super) slowmode: Slowmode,
    /// Who is typing, and when we last said that we are.
    pub(super) typing: Typing,
    /// Other people's roles in each server.
    pub(super) members: Members,
    /// What the gateway is subscribed to for each server.
    pub(super) subscriptions: Subscriptions,
    /// The rows of the member lists we asked for.
    pub(super) member_lists: MemberLists,
    /// What the thread browser asked of each channel's archived threads.
    pub(super) archives: threads::Archives,
    /// Threads we asked Discord for by id, once each a session, found or not.
    pub(super) fetched_threads: HashSet<Snowflake>,
    /// The user closed the member list pane beside the chat, which is open
    /// until they do.
    pub(super) members_closed: bool,
    /// Who is in each server's voice channels.
    pub(super) voice: VoiceStates,
    /// What each message search holds, by where it looks.
    pub(super) searches: HashMap<discord::search::Context, super::search::Saved>,
    /// Channels opened this session, newest first, for the quick switcher.
    pub(super) quick_history: Vec<Snowflake>,
}

impl State {
    /// What a voice channel's participants are called: their nickname in the
    /// server, else their display name, which the update or an earlier
    /// message told us.
    pub(super) fn voice_name(&self, state: &VoiceState) -> String {
        let name = state.nick.as_deref().or_else(|| state.user.as_ref().map(|u| u.display_name()));
        name.map(str::to_owned)
            .or_else(|| self.users.get(&state.user_id.0).cloned())
            .unwrap_or_else(|| "...".to_owned())
    }

    pub(super) fn rail_layout(&self) -> Vec<RailEntry> {
        let ids: Vec<Snowflake> = self.guilds.iter().map(|g| g.id).collect();
        discord::settings::rail_layout(&self.settings.guild_folders, &ids)
    }

    /// The servers as the client's emoji and sticker pickers list them: the
    /// channel's own (`own`) first, then the rail's order.
    pub(super) fn guilds_in_picker_order(&self, own: Option<Snowflake>) -> Vec<&Guild> {
        let rail = self.rail_layout().into_iter().flat_map(|entry| match entry {
            RailEntry::Guild(id) => vec![id],
            RailEntry::Folder { guilds, .. } => guilds,
        });
        let order: Vec<Snowflake> = own.into_iter().chain(rail.filter(|id| Some(*id) != own)).collect();
        order.iter().filter_map(|id| self.guilds.iter().find(|g| g.id == *id)).collect()
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

    /// Where commands would run if `channel` is open, for what they may be.
    pub(super) fn command_place(&self, channel: Snowflake) -> Option<discord::commands::Place> {
        let (guild, channel) = self.find_channel(channel)?;
        Some(discord::commands::Place::of(guild, channel, self.account.as_ref()?, now_unix()))
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

    /// What stands where the message box would in a channel, so nothing we
    /// may post to has one (#51). Slowmode isn't one: the box stays, and the
    /// wait shows above it.
    pub(super) fn notice(&self, id: Snowflake) -> Option<Notice> {
        let (guild, channel) = self.find_channel(id)?;
        let guild = guild.zip(self.account.as_ref());
        Notice::of(
            channel.is_system_dm(),
            channel.is_request().then_some(channel.spam),
            guild.and_then(|(g, a)| g.timeout_left(a, now_unix())),
            guild.and_then(|(g, a)| verification::gate(g, a, now_unix())),
            guild.is_some() && !self.can_send(id),
        )
    }

    /// Whether the message box is withheld from the channel: its page of
    /// posts stands in for a forum's, and a notice for the rest.
    pub(super) fn box_blocked(&self, id: Snowflake) -> bool {
        self.find_channel(id).is_some_and(|(_, c)| c.kind.is_forum_like()) || self.notice(id).is_some()
    }

    /// Whether files may be attached in the channel (see [`permissions::can_attach`]).
    pub(super) fn can_attach(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_attach(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether the post form shows in the forum or media channel (see
    /// [`post_creation::can_post`]).
    pub(super) fn can_post(&self, forum: Snowflake) -> bool {
        match (self.find_channel(forum), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => post_creation::can_post(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether the post form takes files (see [`post_creation::can_attach`]).
    pub(super) fn can_attach_to_post(&self, forum: Snowflake) -> bool {
        match (self.find_channel(forum), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => post_creation::can_attach(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// The tags the post form offers in the forum (see [`post_creation::tag_choices`]).
    pub(super) fn post_tags(&self, forum: Snowflake) -> Vec<discord::model::Tag> {
        match (self.find_channel(forum), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => {
                post_creation::tag_choices(guild, channel, me, now_unix()).into_iter().cloned().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Whether Create Poll is offered in the channel (see [`permissions::can_post_polls`]).
    pub(super) fn can_post_polls(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_post_polls(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Whether Record Voice Message is offered in the channel: our experiment
    /// and permissions (see [`discord::voice::enabled`] and
    /// [`permissions::can_record_voice_message`]).
    pub(super) fn can_record_voice(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account, self.me()) {
            (Some((guild, channel)), Some(me), Some(id)) => {
                discord::voice::enabled(&self.experiments, id)
                    && permissions::can_record_voice_message(guild, channel, me, now_unix())
            }
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

    /// Whether the attach menu offers Create Thread in the channel (see
    /// [`thread_creation::offered_in_attach_menu`]).
    pub(super) fn offers_thread_in_attach_menu(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => {
                thread_creation::offered_in_attach_menu(guild, channel, me, now_unix())
            }
            _ => false,
        }
    }

    /// Whether a thread may be started from `message` (see
    /// [`thread_creation::can_start`]).
    pub(super) fn can_start_thread_from(&self, message: &Message) -> bool {
        match (self.find_channel(message.channel_id), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => {
                thread_creation::can_start(guild, channel, Some(message), me, now_unix())
            }
            _ => false,
        }
    }

    /// Whether a new thread in the channel may be private (see
    /// [`thread_creation::Privacy`]).
    pub(super) fn thread_privacy(&self, id: Snowflake) -> thread_creation::Privacy {
        match (self.find_channel(id), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => thread_creation::Privacy::of(guild, channel, me, now_unix()),
            _ => thread_creation::Privacy::Public,
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

    /// What the client offers of reactions in the channel (see
    /// [`permissions::reacting`]).
    pub(super) fn reacting(&self, id: Snowflake) -> permissions::Reacting {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::reacting(guild, channel, me, now_unix()),
            _ => permissions::Reacting { create: false, update: false },
        }
    }

    /// Whether we may remove other people's reactions in the channel (see
    /// [`permissions::can_remove_reactions`]).
    pub(super) fn can_remove_reactions(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((guild, channel)), Some(me)) => permissions::can_remove_reactions(guild, channel, me, now_unix()),
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

    /// What `user` is called in `channel` (`getName`, 562153): their
    /// nickname in a server, our name for a friend in a DM, else the name
    /// they go by.
    pub(super) fn name_in(&self, channel: Snowflake, user: &User) -> String {
        self.nick_in(channel, user.id).unwrap_or_else(|| user.display_name().to_owned())
    }

    /// The name given to someone in `channel`, if one was.
    pub(super) fn nick_in(&self, channel: Snowflake, user: Snowflake) -> Option<String> {
        match self.find_channel(channel)? {
            (Some(guild), _) => self.members.nick(guild.id, user).map(str::to_owned),
            (None, _) => self.relationships.get(user).and_then(|r| r.nickname.clone()),
        }
    }

    /// The picture `user` set for the server `channel` is in, with the
    /// server: what shows instead of their own.
    pub(super) fn avatar_in(&self, channel: Snowflake, user: Snowflake) -> Option<(Snowflake, &str)> {
        let guild = self.find_channel(channel)?.0?.id;
        Some((guild, self.members.avatar(guild, user)?))
    }

    /// The colour of `user`'s name in `channel`: their highest coloured role,
    /// in a server.
    pub(super) fn name_colour(&self, channel: Snowflake, user: Snowflake) -> Option<RoleColour> {
        let guild = self.find_channel(channel)?.0?;
        let roles =
            if self.me() == Some(user) { Some(&guild.my_roles[..]) } else { self.members.roles(guild.id, user) };
        guild.member_colour(roles?)
    }

    /// The role a profile request names for `user` in `guild` (see
    /// [`discord::profile::connections_role`]); none for someone whose roles
    /// we don't know.
    pub(super) fn connections_role(
        &self,
        guild: Snowflake,
        user: Snowflake,
        channel: Option<Snowflake>,
    ) -> Option<Snowflake> {
        let guild = self.guilds.iter().find(|g| g.id == guild)?;
        let roles =
            if self.me() == Some(user) { Some(&guild.my_roles[..]) } else { self.members.roles(guild.id, user) };
        let channel = channel.and_then(|c| guild.find(c));
        discord::profile::connections_role(&guild.roles, guild.id, roles?, channel)
    }

    /// Whose member list the open channel has, as the client chooses
    /// (`renderSidebar`, 289287): a server's text, announcement, forum and
    /// media channels, and an announcement thread's parent, which show the
    /// server's members; and a group DM. Other threads show those in them,
    /// which Neucord doesn't list yet.
    pub(super) fn member_target(&self) -> Option<Target> {
        let id = self.channel.filter(|_| self.page.is_none())?;
        let (guild, channel) = self.find_channel(id)?;
        let Some(guild) = guild else {
            return match channel.kind {
                discord::ChannelKind::GroupDm => Some(Target::Group(id)),
                // A system account's DM has no profile to show.
                discord::ChannelKind::Dm => {
                    (!channel.is_system_dm() && !channel.recipients.is_empty()).then_some(Target::Dm(id))
                }
                _ => None,
            };
        };
        let channel = match channel.kind {
            discord::ChannelKind::AnnouncementThread => guild.channel(channel.parent_id?)?,
            kind if kind.is_thread() => return None,
            _ => channel,
        };
        let shows = channel.kind.is_text() || channel.kind.is_forum_like();
        shows.then(|| Target::Server {
            guild: guild.id,
            channel: channel.id,
            list: member_list::list_id(guild, channel),
        })
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

    /// Whether we may read `id`'s history, which the client's thread browser
    /// asks before it loads archived threads (`READ_MESSAGE_HISTORY`).
    pub(super) fn can_read_history(&self, id: Snowflake) -> bool {
        match (self.find_channel(id), &self.account) {
            (Some((Some(guild), channel)), Some(me)) => permissions::in_channel(guild, channel, me, now_unix())
                .contains(permissions::Permissions::READ_MESSAGE_HISTORY),
            (Some((None, _)), _) => true,
            _ => false,
        }
    }

    /// Whether we are in the thread: `None` for what isn't one we know.
    pub(super) fn thread_joined(&self, id: Snowflake) -> Option<bool> {
        let (_, thread) = self.find_channel(id)?;
        thread.kind.is_thread().then_some(thread.member.is_some())
    }

    /// A message came to `channel`: a thread has one more, but its starter
    /// message, and a forum post's first message (which has the post's id),
    /// aren't counted (`MESSAGE_CREATE` in 456874). Whether it counted.
    pub(super) fn count_thread_message(&mut self, channel: Snowflake, message: Snowflake, kind: u8) -> bool {
        const THREAD_STARTER: u8 = 21;
        let thread = self.guilds.iter_mut().find_map(|g| g.threads.iter_mut().find(|t| t.id == channel));
        match thread {
            Some(thread) if kind != THREAD_STARTER && message.0 != channel.0 => {
                thread.message_count = Some((thread.message_count.unwrap_or(0) + 1).min(100_000));
                true
            }
            _ => false,
        }
    }

    /// A thread we muted.
    pub(super) fn thread_muted(thread: &Channel) -> bool {
        thread.member.as_ref().is_some_and(|m| m.is_muted(now_unix()))
    }

    /// The server's channels we can view whose kind `kind` accepts, in no order.
    fn visible<'a>(&'a self, guild: &'a Guild, kind: fn(ChannelKind) -> bool) -> impl Iterator<Item = &'a Channel> {
        let now = now_unix();
        guild
            .channels
            .iter()
            .filter(move |c| kind(c.kind) && self.account.as_ref().is_some_and(|me| guild.can_view(c, me, now)))
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
        for channel in self.visible(guild, ChannelKind::is_text) {
            mentions += self.read.mentions(channel.id);
            if !unread && !guild_muted && self.is_unread(channel) {
                let place =
                    Place { guild: Some(guild.id), guild_default: 0, channel: channel.id, parent: channel.parent_id };
                unread = !self.notifications.channel_muted(&place, now);
            }
        }
        // A call's chat adds its mentions but never a dot of its own
        // (`isGuildVocal() && 0 === t` in 458294).
        mentions += self.visible(guild, ChannelKind::is_vocal).map(|c| self.read.mentions(c.id)).sum::<u32>();
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

    /// The rail's signature, and the count on the app icon and in the title
    /// (see [`discord::badge`]), from the one pass over every server. A
    /// server's dot counts, but not a DM's, as in the client (`hasAnyUnread`
    /// in 458294 holds servers only).
    pub(super) fn rail_signature(&self) -> (Vec<u64>, i64) {
        let home = self.home_badge();
        let mut sig = vec![self.guild.map_or(0, |g| g.0), home as u64];
        let (mut mentions, mut any_unread) = (home, false);
        for guild in &self.guilds {
            let (unread, guild_mentions) = self.guild_badge(guild);
            sig.extend([guild.id.0, unread as u64, guild_mentions as u64]);
            mentions += guild_mentions;
            any_unread |= unread;
        }
        // The client's "disable unread badge" setting is #86's.
        (sig, discord::badge::count(mentions, any_unread, false))
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
                        if c.kind == ChannelKind::Voice {
                            for v in self.voice.in_channel(guild.id, c.id, |v| self.voice_name(v)) {
                                let mut hash = std::collections::hash_map::DefaultHasher::new();
                                (v.user_id, v.muted(), v.deafened(), v.self_video, v.self_stream, self.voice_name(v))
                                    .hash(&mut hash);
                                sig.push(hash.finish());
                            }
                        }
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
    fn people_are_called_what_they_chose_where_they_are() {
        use discord::members::Known;
        let mut state = open_server(None, json!({}));
        state.private_channels = vec![
            serde_json::from_value(json!({"id": "30", "type": 1, "recipients": [{"id": "6", "username": "six"}]}))
                .unwrap(),
        ];
        let user = |id: u64| -> User {
            serde_json::from_value(json!({"id": id.to_string(), "username": "u", "global_name": "Global"})).unwrap()
        };
        let (server, dm) = (Snowflake(9), Snowflake(30));
        assert_eq!(state.name_in(server, &user(6)), "Global", "no nickname known");
        let known = Known { nick: Some("Nick".into()), avatar: Some("av".into()), ..Known::default() };
        state.members.add(Snowflake(1), vec![(Snowflake(6), known)], &[]);
        assert_eq!(state.name_in(server, &user(6)), "Nick");
        assert_eq!(state.avatar_in(server, Snowflake(6)), Some((Snowflake(1), "av")));
        assert_eq!(state.name_in(server, &user(7)), "Global", "someone else's");
        // A DM has no server nicknames, but our name for a friend counts.
        assert_eq!(state.name_in(dm, &user(6)), "Global");
        assert_eq!(state.avatar_in(dm, Snowflake(6)), None);
        let friend = json!({"id": "6", "type": 1, "nickname": "Pal", "user": {"id": "6", "username": "u"}});
        state
            .relationships
            .add(discord::relationships::Relationship::from_value(&friend, &Default::default()).unwrap());
        assert_eq!(state.name_in(dm, &user(6)), "Pal");
        assert_eq!(state.name_in(server, &user(6)), "Nick", "a server's choice isn't ours to rename");
    }

    #[test]
    fn which_channels_have_a_member_list() {
        let mut state = open_server(None, json!({}));
        let guild: Snowflake = Snowflake(1);
        let with = |state: &mut State, channel: &str, kind: serde_json::Value| {
            let channel: Channel =
                serde_json::from_value(json!({"id": channel, "type": kind, "parent_id": "9"})).unwrap();
            let id = channel.id;
            state.guilds[0].channels.push(channel);
            state.channel = Some(id);
        };
        state.channel = Some(Snowflake(9));
        let shown = Target::Server { guild, channel: Snowflake(9), list: "everyone".into() };
        // `@everyone` here has no View Channel, so the list is named by the hash.
        assert!(matches!(state.member_target(), Some(Target::Server { channel: Snowflake(9), .. })));
        state.guilds[0].roles.push(serde_json::from_value(json!({"id": "1", "permissions": "1024"})).unwrap());
        assert_eq!(state.member_target(), Some(shown.clone()));

        state.page = Some(Page::Friends);
        assert_eq!(state.member_target(), None, "Home's pages have none");
        state.page = None;

        with(&mut state, "20", json!(5));
        assert!(matches!(state.member_target(), Some(Target::Server { channel: Snowflake(20), .. })), "announcements");
        with(&mut state, "21", json!(15));
        assert!(matches!(state.member_target(), Some(Target::Server { channel: Snowflake(21), .. })), "forums");
        with(&mut state, "22", json!(2));
        assert_eq!(state.member_target(), None, "voice");
        with(&mut state, "23", json!(11));
        assert_eq!(state.member_target(), None, "a public thread has its own list");
        with(&mut state, "24", json!(10));
        assert_eq!(state.member_target(), Some(shown), "an announcement thread shows its parent's");

        state.guild = None;
        state.private_channels = vec![
            serde_json::from_value(json!({"id": "30", "type": 1})).unwrap(),
            serde_json::from_value(json!({"id": "31", "type": 3})).unwrap(),
            serde_json::from_value(json!({"id": "32", "type": 1, "recipients": [{"id": "5", "username": "u"}]}))
                .unwrap(),
            serde_json::from_value(
                json!({"id": "33", "type": 1, "recipients": [{"id": "6", "username": "u", "system": true}]}),
            )
            .unwrap(),
        ];
        state.channel = Some(Snowflake(30));
        assert_eq!(state.member_target(), None, "a DM with no one has no profile");
        state.channel = Some(Snowflake(32));
        assert_eq!(state.member_target(), Some(Target::Dm(Snowflake(32))), "a DM shows a profile, not a list");
        state.channel = Some(Snowflake(33));
        assert_eq!(state.member_target(), None, "Discord's own account has none");
        state.channel = Some(Snowflake(31));
        assert_eq!(state.member_target(), Some(Target::Group(Snowflake(31))));
    }

    #[test]
    fn threads_count_their_messages_and_say_whether_we_are_in() {
        let mut state = open_server(None, json!({}));
        let thread = |id: &str, member: serde_json::Value| -> Channel {
            serde_json::from_value(
                json!({"id": id, "type": 11, "parent_id": "9", "message_count": 3, "member": member}),
            )
            .unwrap()
        };
        state.guilds[0].threads = vec![thread("20", json!({"flags": 0})), thread("21", serde_json::Value::Null)];
        let count = |state: &State, id: u64| state.guilds[0].thread(Snowflake(id)).unwrap().message_count;
        assert_eq!(state.thread_joined(Snowflake(20)), Some(true));
        assert_eq!(state.thread_joined(Snowflake(21)), Some(false));
        assert_eq!(state.thread_joined(Snowflake(9)), None, "a channel isn't a thread");
        assert!(state.count_thread_message(Snowflake(20), Snowflake(90), 0));
        assert_eq!(count(&state, 20), Some(4));
        assert!(!state.count_thread_message(Snowflake(20), Snowflake(91), 21), "the starter message isn't counted");
        assert!(!state.count_thread_message(Snowflake(20), Snowflake(20), 0), "a forum post's first message isn't");
        assert!(!state.count_thread_message(Snowflake(9), Snowflake(92), 0), "a channel has no count");
        assert_eq!(count(&state, 20), Some(4));
        state.guilds[0].threads[1].message_count = Some(100_000);
        state.count_thread_message(Snowflake(21), Snowflake(93), 0);
        assert_eq!(count(&state, 21), Some(100_000), "the count stops at a hundred thousand");
    }

    #[test]
    fn archived_threads_wait_for_history_access() {
        let mut state = open_server(None, json!({}));
        let everyone =
            |permissions: &str| serde_json::from_value(json!({"id": "1", "permissions": permissions})).unwrap();
        state.guilds[0].set_role(everyone("1024"));
        assert!(!state.can_read_history(Snowflake(9)), "View Channel alone");
        state.guilds[0].set_role(everyone("66560"));
        assert!(state.can_read_history(Snowflake(9)));
        assert!(!state.can_read_history(Snowflake(77)), "not a channel we know");
    }

    #[test]
    fn the_message_box_is_blocked_where_the_notice_bar_would_replace_it() {
        let mut state = open_server(None, json!({}));
        let id = Snowflake(9);
        let everyone =
            |permissions: &str| serde_json::from_value(json!({"id": "1", "permissions": permissions})).unwrap();
        state.guilds[0].set_role(everyone("1024"));
        assert_eq!(state.notice(id), Some(Notice::Denied));
        assert!(state.box_blocked(id));
        state.guilds[0].set_role(everyone("3072"));
        assert!(!state.box_blocked(id), "with Send Messages");
        assert!(!state.box_blocked(Snowflake(77)), "a channel we don't know");

        state.guilds[0].channels[0].kind = ChannelKind::Forum;
        assert!(state.box_blocked(id), "a forum has its page of posts in its place");

        let dm = |extra: serde_json::Value| -> Channel {
            let mut dm = json!({"id": "50", "type": 1, "recipients": [{"id": "6", "username": "x"}]});
            dm.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(dm).unwrap()
        };
        state.private_channels = vec![dm(json!({}))];
        assert!(!state.box_blocked(Snowflake(50)), "an ordinary DM");
        state.private_channels = vec![dm(json!({"is_message_request": true}))];
        assert_eq!(state.notice(Snowflake(50)), Some(Notice::Request { spam: false }));
        state.private_channels = vec![dm(json!({"is_spam": true}))];
        assert_eq!(state.notice(Snowflake(50)), Some(Notice::Request { spam: true }));
        state.private_channels =
            vec![dm(json!({"recipients": [{"id": "643945264868098049", "username": "Discord", "system": true}]}))];
        assert_eq!(state.notice(Snowflake(50)), Some(Notice::System));
    }

    #[test]
    fn a_calls_chat_counts_mentions_for_the_server_but_never_an_unread_dot() {
        let mut state = open_server(None, json!({"type": 2, "last_message_id": "200"}));
        state.guilds[0].set_role(serde_json::from_value(json!({"id": "1", "permissions": "1049600"})).unwrap());
        state.read = ReadStates::from_ready(&json!([{"id": "9", "last_message_id": "100", "mention_count": 0}]));
        assert!(state.is_unread(&state.guilds[0].channels[0]), "the chat has something new");
        assert_eq!(state.guild_badge(&state.guilds[0]), (false, 0), "which the server doesn't show");
        state.read = ReadStates::from_ready(&json!([{"id": "9", "last_message_id": "100", "mention_count": 2}]));
        assert_eq!(state.guild_badge(&state.guilds[0]), (false, 2), "a mention does count");
        // The same chat in a text channel lights the dot.
        state.guilds[0].channels[0].kind = ChannelKind::Text;
        assert_eq!(state.guild_badge(&state.guilds[0]), (true, 2));
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
