//! What we may do in a server and its channels, worked out as the official
//! client does (module 488926, anchor `lurkerPermissionsMask`), so Neucord
//! offers nothing the client would hold back.
//!
//! The owner has every permission. Anyone else starts from `@everyone`'s
//! permissions and adds their roles'; Administrator then gives every
//! permission. Otherwise the channel's overwrites apply: `@everyone`'s, our
//! roles' (all their denies, then all their allows), our own. Unless that
//! left Administrator, automod quarantine cuts us down to reading and
//! changing our nickname, and a timeout to reading. These apply even to an
//! administrator: a member held by screening only reads, a guest keeps what
//! a guest may, and a server that requires 2FA for moderation takes the
//! moderation permissions from an account without it, owner included.
//!
//! A thread takes its parent's permissions, with posting decided by Send
//! Messages in Threads instead of Send Messages ([`in_thread`]).
//!
//! Not copied: lurking and viewing a server as a role, which Neucord can't
//! do, and a public stage's wider allowance for lurkers.

use std::ops::{BitAnd, BitOr};

use serde::{Deserialize, Deserializer};

use crate::verification::{self, Account};
use crate::{Channel, ChannelKind, Guild, Message, Snowflake, User};

/// A set of Discord's permission bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Permissions(pub u64);

impl Permissions {
    pub const NONE: Self = Self(0);
    pub const CREATE_INSTANT_INVITE: Self = Self(1 << 0);
    pub const KICK_MEMBERS: Self = Self(1 << 1);
    pub const BAN_MEMBERS: Self = Self(1 << 2);
    pub const ADMINISTRATOR: Self = Self(1 << 3);
    pub const MANAGE_CHANNELS: Self = Self(1 << 4);
    pub const MANAGE_GUILD: Self = Self(1 << 5);
    pub const ADD_REACTIONS: Self = Self(1 << 6);
    pub const VIEW_AUDIT_LOG: Self = Self(1 << 7);
    pub const PRIORITY_SPEAKER: Self = Self(1 << 8);
    pub const STREAM: Self = Self(1 << 9);
    pub const VIEW_CHANNEL: Self = Self(1 << 10);
    pub const SEND_MESSAGES: Self = Self(1 << 11);
    pub const SEND_TTS_MESSAGES: Self = Self(1 << 12);
    pub const MANAGE_MESSAGES: Self = Self(1 << 13);
    pub const EMBED_LINKS: Self = Self(1 << 14);
    pub const ATTACH_FILES: Self = Self(1 << 15);
    pub const READ_MESSAGE_HISTORY: Self = Self(1 << 16);
    pub const MENTION_EVERYONE: Self = Self(1 << 17);
    pub const USE_EXTERNAL_EMOJIS: Self = Self(1 << 18);
    pub const VIEW_GUILD_ANALYTICS: Self = Self(1 << 19);
    pub const CONNECT: Self = Self(1 << 20);
    pub const SPEAK: Self = Self(1 << 21);
    pub const MUTE_MEMBERS: Self = Self(1 << 22);
    pub const DEAFEN_MEMBERS: Self = Self(1 << 23);
    pub const MOVE_MEMBERS: Self = Self(1 << 24);
    pub const USE_VAD: Self = Self(1 << 25);
    pub const CHANGE_NICKNAME: Self = Self(1 << 26);
    pub const MANAGE_NICKNAMES: Self = Self(1 << 27);
    pub const MANAGE_ROLES: Self = Self(1 << 28);
    pub const MANAGE_WEBHOOKS: Self = Self(1 << 29);
    pub const MANAGE_GUILD_EXPRESSIONS: Self = Self(1 << 30);
    pub const USE_APPLICATION_COMMANDS: Self = Self(1 << 31);
    pub const REQUEST_TO_SPEAK: Self = Self(1 << 32);
    pub const MANAGE_EVENTS: Self = Self(1 << 33);
    pub const MANAGE_THREADS: Self = Self(1 << 34);
    pub const CREATE_PUBLIC_THREADS: Self = Self(1 << 35);
    pub const CREATE_PRIVATE_THREADS: Self = Self(1 << 36);
    pub const USE_EXTERNAL_STICKERS: Self = Self(1 << 37);
    pub const SEND_MESSAGES_IN_THREADS: Self = Self(1 << 38);
    pub const USE_EMBEDDED_ACTIVITIES: Self = Self(1 << 39);
    pub const MODERATE_MEMBERS: Self = Self(1 << 40);
    pub const VIEW_CREATOR_MONETIZATION_ANALYTICS: Self = Self(1 << 41);
    pub const USE_SOUNDBOARD: Self = Self(1 << 42);
    pub const CREATE_GUILD_EXPRESSIONS: Self = Self(1 << 43);
    pub const CREATE_EVENTS: Self = Self(1 << 44);
    pub const USE_EXTERNAL_SOUNDS: Self = Self(1 << 45);
    pub const SEND_VOICE_MESSAGES: Self = Self(1 << 46);
    pub const SET_VOICE_CHANNEL_STATUS: Self = Self(1 << 48);
    pub const SEND_POLLS: Self = Self(1 << 49);
    pub const USE_EXTERNAL_APPS: Self = Self(1 << 50);
    pub const PIN_MESSAGES: Self = Self(1 << 51);
    pub const BYPASS_SLOWMODE: Self = Self(1 << 52);
    pub const MANAGE_OFFICIAL_MESSAGES: Self = Self(1 << 53);

    /// Every permission the client knows of, which is what it gives the
    /// owner and administrators: bit 47 is unused.
    pub const ALL: Self = Self::of(&[
        Self::CREATE_INSTANT_INVITE,
        Self::KICK_MEMBERS,
        Self::BAN_MEMBERS,
        Self::ADMINISTRATOR,
        Self::MANAGE_CHANNELS,
        Self::MANAGE_GUILD,
        Self::ADD_REACTIONS,
        Self::VIEW_AUDIT_LOG,
        Self::PRIORITY_SPEAKER,
        Self::STREAM,
        Self::VIEW_CHANNEL,
        Self::SEND_MESSAGES,
        Self::SEND_TTS_MESSAGES,
        Self::MANAGE_MESSAGES,
        Self::EMBED_LINKS,
        Self::ATTACH_FILES,
        Self::READ_MESSAGE_HISTORY,
        Self::MENTION_EVERYONE,
        Self::USE_EXTERNAL_EMOJIS,
        Self::VIEW_GUILD_ANALYTICS,
        Self::CONNECT,
        Self::SPEAK,
        Self::MUTE_MEMBERS,
        Self::DEAFEN_MEMBERS,
        Self::MOVE_MEMBERS,
        Self::USE_VAD,
        Self::CHANGE_NICKNAME,
        Self::MANAGE_NICKNAMES,
        Self::MANAGE_ROLES,
        Self::MANAGE_WEBHOOKS,
        Self::MANAGE_GUILD_EXPRESSIONS,
        Self::USE_APPLICATION_COMMANDS,
        Self::REQUEST_TO_SPEAK,
        Self::MANAGE_EVENTS,
        Self::MANAGE_THREADS,
        Self::CREATE_PUBLIC_THREADS,
        Self::CREATE_PRIVATE_THREADS,
        Self::USE_EXTERNAL_STICKERS,
        Self::SEND_MESSAGES_IN_THREADS,
        Self::USE_EMBEDDED_ACTIVITIES,
        Self::MODERATE_MEMBERS,
        Self::VIEW_CREATOR_MONETIZATION_ANALYTICS,
        Self::USE_SOUNDBOARD,
        Self::CREATE_GUILD_EXPRESSIONS,
        Self::CREATE_EVENTS,
        Self::USE_EXTERNAL_SOUNDS,
        Self::SEND_VOICE_MESSAGES,
        Self::SET_VOICE_CHANNEL_STATUS,
        Self::SEND_POLLS,
        Self::USE_EXTERNAL_APPS,
        Self::PIN_MESSAGES,
        Self::BYPASS_SLOWMODE,
        Self::MANAGE_OFFICIAL_MESSAGES,
    ]);

    /// `@everyone`'s permissions in a server whose role we don't have.
    const DEFAULT_EVERYONE: Self = Self::of(&[
        Self::CREATE_INSTANT_INVITE,
        Self::CHANGE_NICKNAME,
        Self::VIEW_CHANNEL,
        Self::SEND_MESSAGES,
        Self::EMBED_LINKS,
        Self::ATTACH_FILES,
        Self::READ_MESSAGE_HISTORY,
        Self::MENTION_EVERYONE,
        Self::USE_EXTERNAL_EMOJIS,
        Self::USE_EXTERNAL_STICKERS,
        Self::ADD_REACTIONS,
        Self::CREATE_PUBLIC_THREADS,
        Self::CREATE_PRIVATE_THREADS,
        Self::SEND_MESSAGES_IN_THREADS,
        Self::SEND_POLLS,
        Self::CONNECT,
        Self::SPEAK,
        Self::USE_VAD,
        Self::STREAM,
        Self::USE_EMBEDDED_ACTIVITIES,
        Self::USE_SOUNDBOARD,
        Self::REQUEST_TO_SPEAK,
        Self::USE_APPLICATION_COMMANDS,
        Self::CREATE_GUILD_EXPRESSIONS,
        Self::CREATE_EVENTS,
        Self::USE_EXTERNAL_APPS,
    ]);

    /// What a timeout leaves, and a member held by screening.
    const READING: Self = Self::of(&[Self::VIEW_CHANNEL, Self::READ_MESSAGE_HISTORY]);

    /// What automod quarantine leaves.
    const QUARANTINED: Self = Self::of(&[Self::VIEW_CHANNEL, Self::READ_MESSAGE_HISTORY, Self::CHANGE_NICKNAME]);

    /// The most a guest member may do.
    const GUEST: Self = Self::of(&[
        Self::VIEW_CHANNEL,
        Self::SEND_MESSAGES,
        Self::CONNECT,
        Self::SPEAK,
        Self::STREAM,
        Self::USE_EMBEDDED_ACTIVITIES,
        Self::USE_EXTERNAL_APPS,
        Self::USE_EXTERNAL_EMOJIS,
        Self::USE_EXTERNAL_SOUNDS,
        Self::USE_EXTERNAL_STICKERS,
        Self::USE_SOUNDBOARD,
        Self::USE_VAD,
        Self::SEND_MESSAGES_IN_THREADS,
        Self::EMBED_LINKS,
        Self::ATTACH_FILES,
        Self::ADD_REACTIONS,
    ]);

    /// What a server that requires 2FA for moderation takes from an account
    /// without it.
    const ELEVATED: Self = Self::of(&[
        Self::KICK_MEMBERS,
        Self::BAN_MEMBERS,
        Self::ADMINISTRATOR,
        Self::MANAGE_CHANNELS,
        Self::MANAGE_GUILD,
        Self::MANAGE_ROLES,
        Self::MANAGE_MESSAGES,
        Self::MANAGE_THREADS,
        Self::MODERATE_MEMBERS,
    ]);

    const fn of(list: &[Self]) -> Self {
        let mut bits = 0;
        let mut i = 0;
        while i < list.len() {
            bits |= list[i].0;
            i += 1;
        }
        Self(bits)
    }

    /// Whether every permission in `other` is here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// A deny, then an allow, as each overwrite applies.
    const fn overwritten(self, deny: Self, allow: Self) -> Self {
        Self(self.without(deny).0 | allow.0)
    }
}

impl BitOr for Permissions {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitAnd for Permissions {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Discord sends permissions as decimal strings, since they outgrew what a
/// JavaScript number holds exactly.
impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<'a> {
            Text(std::borrow::Cow<'a, str>),
            Number(u64),
            Null(()),
        }
        match Raw::deserialize(d)? {
            Raw::Text(text) => text.parse().map(Self).map_err(serde::de::Error::custom),
            Raw::Number(bits) => Ok(Self(bits)),
            Raw::Null(()) => Ok(Self::NONE),
        }
    }
}

/// Member flags the computation reads.
const IS_GUEST: u64 = 1 << 4;
const AUTOMOD_QUARANTINED: u64 = (1 << 7) | (1 << 8) | (1 << 10);

/// Channel flag: the channel is about to be deleted, and nobody may do
/// anything there.
const IS_SCHEDULED_FOR_DELETION: u64 = 1 << 9;
/// Channel flag: where members' reports reach a server's moderators.
pub(crate) const IS_MODERATOR_REPORT_CHANNEL: u64 = 1 << 19;

/// Ours in `guild` as a whole, at `now` (seconds since 1970).
pub fn in_guild(guild: &Guild, me: &Account, now: i64) -> Permissions {
    compute(guild, &[], me, now)
}

/// Ours in `channel`, one of `guild`'s or a thread in one, at `now`.
pub fn in_channel(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> Permissions {
    if channel.flags & IS_SCHEDULED_FOR_DELETION != 0 {
        return Permissions::NONE;
    }
    if channel.kind.is_thread() {
        // No parent, or one being deleted: nothing, whatever the thread.
        let parent = channel.parent_id.and_then(|id| guild.channel(id));
        return match parent {
            Some(parent) if parent.flags & IS_SCHEDULED_FOR_DELETION == 0 => {
                in_thread(guild, channel, in_channel(guild, parent, me, now))
            }
            _ => Permissions::NONE,
        };
    }
    compute(guild, &channel.permission_overwrites, me, now)
}

/// Ours in `thread`, given ours in its parent (the client's thread rule, `F`
/// in 488926). A media thread only takes reading. A private thread we
/// haven't joined gives nothing without Manage Threads, unless we're a
/// guest. Otherwise the parent's permissions apply, with Send Messages
/// granted by Send Messages in Threads, and taken away again in a locked
/// thread without Manage Threads.
fn in_thread(guild: &Guild, thread: &Channel, parent: Permissions) -> Permissions {
    if thread.kind == ChannelKind::MediaThread {
        return Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
    }
    let joined = thread.member.is_some();
    let guest = guild.member_flags & IS_GUEST != 0;
    if thread.kind == ChannelKind::PrivateThread && !joined && !guest && !parent.contains(Permissions::MANAGE_THREADS) {
        return Permissions::NONE;
    }
    let locked_out = thread.is_locked() && !parent.contains(Permissions::MANAGE_THREADS);
    if parent.contains(Permissions::SEND_MESSAGES_IN_THREADS) && !locked_out {
        parent | Permissions::SEND_MESSAGES
    } else {
        parent.without(Permissions::SEND_MESSAGES)
    }
}

fn compute(guild: &Guild, overwrites: &[crate::model::Overwrite], me: &Account, now: i64) -> Permissions {
    let permissions = if guild.owner_id == Some(me.id) {
        Permissions::ALL
    } else {
        let mut permissions = member(guild, overwrites, me, now);
        if guild.pending {
            permissions = permissions & Permissions::READING;
        }
        if guild.member_flags & IS_GUEST != 0 {
            permissions = permissions & Permissions::GUEST;
        }
        permissions
    };
    if guild.requires_mfa && !me.mfa { permissions.without(Permissions::ELEVATED) } else { permissions }
}

/// From our roles and the overwrites; the owner never gets here.
fn member(guild: &Guild, overwrites: &[crate::model::Overwrite], me: &Account, now: i64) -> Permissions {
    let mut permissions = from_roles(guild, overwrites, me.id, &guild.my_roles);
    if !permissions.contains(Permissions::ADMINISTRATOR) {
        if guild.member_flags & AUTOMOD_QUARANTINED != 0 {
            permissions = permissions & Permissions::QUARANTINED;
        }
        if guild.timed_out_until.is_some_and(|until| until > now) {
            permissions = permissions & Permissions::READING;
        }
    }
    permissions
}

/// What `user`'s roles and the overwrites give: `@everyone`'s, then their
/// roles', then the overwrites of `@everyone`, of their roles together and
/// of them, unless a role makes them an administrator.
fn from_roles(
    guild: &Guild,
    overwrites: &[crate::model::Overwrite],
    user: Snowflake,
    roles: &[Snowflake],
) -> Permissions {
    let everyone = guild.role(guild.id).map_or(Permissions::DEFAULT_EVERYONE, |r| r.permissions);
    let base = roles.iter().filter_map(|&id| guild.role(id)).fold(everyone, |p, r| p | r.permissions);
    if base.contains(Permissions::ADMINISTRATOR) {
        return Permissions::ALL;
    }
    // Overwrites are keyed by id: `@everyone`'s is the server's own.
    let find = |id| overwrites.iter().rfind(|o| o.id == id);
    let mut permissions = base;
    if let Some(o) = find(guild.id) {
        permissions = permissions.overwritten(o.deny, o.allow);
    }
    let (deny, allow) = (roles.iter())
        .filter_map(|&id| find(id))
        .fold((Permissions::NONE, Permissions::NONE), |(deny, allow), o| (deny | o.deny, allow | o.allow));
    permissions = permissions.overwritten(deny, allow);
    if let Some(o) = find(user) {
        permissions = permissions.overwritten(o.deny, o.allow);
    }
    permissions
}

/// Whether another member, `user` with `roles`, can see `channel` (a
/// thread's parent stands for it): the message box's `@` list offers only
/// them (`queryChannelUsers` in 997625, asking 488926's `$3` for View
/// Channel). The owner always can.
pub fn member_can_view(guild: &Guild, channel: &Channel, user: Snowflake, roles: &[Snowflake]) -> bool {
    let channel = match channel.parent_id.filter(|_| channel.kind.is_thread()) {
        Some(parent) => match guild.channel(parent) {
            Some(parent) => parent,
            None => return false,
        },
        None => channel,
    };
    guild.owner_id == Some(user)
        || from_roles(guild, &channel.permission_overwrites, user, roles).contains(Permissions::VIEW_CHANNEL)
}

/// What it takes to see `channel` at all (the client's `accessPermissions`):
/// View Channel, and Connect too for voice and stage channels.
pub fn access(channel: &Channel) -> Permissions {
    match channel.kind {
        ChannelKind::Voice | ChannelKind::Stage => Permissions::VIEW_CHANNEL | Permissions::CONNECT,
        _ => Permissions::VIEW_CHANNEL,
    }
}

/// Whether `channel`, one of `guild`'s, shows at all: what the sidebar
/// lists, unread dots count and Neucord opens.
pub fn can_view(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> bool {
    in_channel(guild, channel, me, now).contains(access(channel))
}

/// Whether `@everyone` can't see `channel`, so only some roles or members do:
/// the client's test for the lock on a channel's icon (module 148719, with the
/// icon choice in 713654). It looks at `@everyone` alone, never at us. A voice
/// or stage channel also counts when `@everyone` can't connect.
pub fn hidden_from_everyone(guild: &Guild, channel: &Channel) -> bool {
    let kind = channel.kind;
    let lockable = kind.is_forum_like()
        || matches!(
            kind,
            ChannelKind::Text | ChannelKind::Announcement | ChannelKind::App | ChannelKind::Voice | ChannelKind::Stage
        );
    if !lockable {
        return false;
    }
    // Overwrites are keyed by id, so a repeat of `@everyone`'s replaces the first.
    let overwrite = channel.permission_overwrites.iter().rfind(|o| o.id == guild.id);
    if matches!(kind, ChannelKind::Voice | ChannelKind::Stage) {
        let everyone = guild.role(guild.id).map_or(Permissions::DEFAULT_EVERYONE, |r| r.permissions);
        let everyone = overwrite.map_or(everyone, |o| everyone.overwritten(o.deny, o.allow));
        if !everyone.contains(Permissions::CONNECT) {
            return true;
        }
    }
    overwrite.is_some_and(|o| o.deny.contains(Permissions::VIEW_CHANNEL))
}

/// Whether we may post in `channel`: anywhere but the system DM outside
/// servers, and in one where we can see it with Send Messages. The client
/// asks only for the latter, but never opens a channel we can't see. Never
/// a forum or media channel: Send Messages there starts posts, which are
/// threads, and the client shows them no message box (420018).
pub fn can_send(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    match guild {
        None => !channel.is_system_dm(),
        Some(_) if channel.kind.is_forum_like() => false,
        Some(guild) => in_channel(guild, channel, me, now).contains(access(channel) | Permissions::SEND_MESSAGES),
    }
}

/// Whether a quarantine or a timeout keeps us from voting in `guild`, as
/// `canSubmitVote` in 951727 reads them: the quarantine flags of 229527 and
/// `communicationDisabledUntil` (316031), whatever our permissions are, so
/// administrators included.
pub fn vote_held_back(guild: Option<&Guild>, now: i64) -> bool {
    guild.is_some_and(|g| {
        g.member_flags & AUTOMOD_QUARANTINED != 0 || g.timed_out_until.is_some_and(|until| until > now)
    })
}

/// Whether the client turns a message's buttons and menus off for us in
/// `channel` (`useComponentState` in 911408): the server's verification
/// stops us chatting (screening aside), we are still pending, a locked thread
/// we can't manage or an archived one we can't reopen, or a timeout, which
/// holds back administrators too.
pub fn components_blocked(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    let Some(guild) = guild else { return false };
    let locked = channel.is_locked() && !in_channel(guild, channel, me, now).contains(Permissions::MANAGE_THREADS);
    verification::gate(guild, me, now).is_some_and(|gate| gate != verification::Gate::Screening)
        || guild.pending
        || locked
        || (channel.is_archived() && !can_unarchive(guild, channel, me, now))
        || guild.timed_out_until.is_some_and(|until| until > now)
}

/// Whether the client offers attaching files in `channel` (the attach menu
/// in 316576, whose `canAttachFiles` is any private channel, else Attach
/// Files with Send Messages), where [`can_send`] also holds.
pub fn can_attach(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    can_send(guild, channel, me, now)
        && guild.is_none_or(|g| in_channel(g, channel, me, now).contains(Permissions::ATTACH_FILES))
}

/// Whether the client offers Record Voice Message in `channel` (87207): in
/// any private channel, else with Send Voice Messages; and where [`can_send`]
/// also holds, which the message box it belongs to needs.
pub fn can_record_voice_message(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    can_send(guild, channel, me, now)
        && !channel.kind.is_forum_like()
        && guild.is_none_or(|g| in_channel(g, channel, me, now).contains(Permissions::SEND_VOICE_MESSAGES))
}

/// Whether the client offers Create Poll in `channel` (`I7` in 969632): a
/// channel type that takes polls, and in a server Send Polls with Send
/// Messages, which [`can_send`] checks and which the box it belongs to needs.
pub fn can_post_polls(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    channel.kind.takes_polls()
        && can_send(guild, channel, me, now)
        && guild.is_none_or(|g| in_channel(g, channel, me, now).contains(Permissions::SEND_POLLS))
}

/// Whether the client offers Reply in `channel` (517997), with
/// [`crate::Message::is_replyable`] for the message itself: not in the
/// system DM, in a server only with Send Messages and Read Message History
/// and while we aren't timed out, and in an archived thread only if we
/// [`can_unarchive`] it. The timeout counts on its own because it only takes
/// Send Messages from those without Administrator, and the client asks
/// about the timeout whatever our permissions.
pub fn can_reply(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    can_send(guild, channel, me, now)
        && guild.is_none_or(|g| {
            in_channel(g, channel, me, now).contains(access(channel) | Permissions::READ_MESSAGE_HISTORY)
                && g.timed_out_until.is_none_or(|until| until <= now)
                && (!channel.is_archived() || can_unarchive(g, channel, me, now))
        })
}

/// Whether we may act on `target`, a member of `guild`, with `permission`
/// (`canManageUser` in 576705): never the owner; `permission` in the server
/// as a whole; and we own the server, or our highest role is above theirs,
/// which it always is when they have none or we don't know theirs
/// (`target_roles` none: `HJ` in 488926 finds no member). Highest and above
/// go by [`Guild::highest_role`] (`wO` in 488926, `zA` in 7864).
pub fn can_manage(
    guild: &Guild,
    me: &Account,
    target: Snowflake,
    target_roles: Option<&[Snowflake]>,
    permission: Permissions,
    now: i64,
) -> bool {
    if guild.owner_id == Some(target) || !in_guild(guild, me, now).contains(permission) {
        return false;
    }
    if guild.owner_id == Some(me.id) {
        return true;
    }
    let highest = |roles: &[Snowflake]| guild.highest_role(roles.iter().filter_map(|&id| guild.role(id)));
    let Some(ours) = highest(&guild.my_roles) else { return false };
    match target_roles.and_then(highest) {
        None => true,
        Some(theirs) => guild.highest_role([ours, theirs]).is_some_and(|h| h.id == ours.id && ours.id != theirs.id),
    }
}

/// Whether /timeout may time `target` out (`b` in 838111): someone we know
/// (`target` none: not in the client's `UserStore`), not Discord's own or a
/// webhook's account, not the owner, without Administrator from
/// `@everyone` and their roles (`$3` in 488926; the 2FA cut there is only
/// ever ours), and [`can_manage`] with Moderate Members.
pub fn can_time_out(
    guild: &Guild,
    me: &Account,
    target: Option<&User>,
    target_roles: Option<&[Snowflake]>,
    now: i64,
) -> bool {
    let Some(target) = target else { return false };
    let everyone = guild.role(guild.id).map_or(Permissions::DEFAULT_EVERYONE, |r| r.permissions);
    let theirs = target_roles
        .unwrap_or_default()
        .iter()
        .filter_map(|&id| guild.role(id))
        .fold(everyone, |p, r| p | r.permissions);
    !target.non_user_bot()
        && guild.owner_id != Some(target.id)
        && !theirs.contains(Permissions::ADMINISTRATOR)
        && can_manage(guild, me, target.id, target_roles, Permissions::MODERATE_MEMBERS, now)
}

/// Whether we may reopen an archived `thread` (`lK` in 406704): a locked one
/// with Manage Threads, any other with Send Messages in Threads and Send
/// Messages there, and Send Messages in its parent. Never a media thread.
pub fn can_unarchive(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> bool {
    if !thread.kind.is_thread() || thread.kind == ChannelKind::MediaThread {
        return false;
    }
    let here = in_channel(guild, thread, me, now);
    if thread.is_locked() {
        return here.contains(Permissions::MANAGE_THREADS);
    }
    let parent = thread.parent_id.and_then(|id| guild.channel(id));
    here.contains(Permissions::SEND_MESSAGES_IN_THREADS | Permissions::SEND_MESSAGES)
        && parent.is_some_and(|p| in_channel(guild, p, me, now).contains(Permissions::SEND_MESSAGES))
}

/// Whether `me` may edit `message` in `channel`, as the client offers it:
/// [`Message::is_editable_by`], where messages [`can_change`], and not in a
/// locked thread without Manage Threads (`s5` in 406704).
pub fn can_edit(guild: Option<&Guild>, channel: &Channel, message: &Message, me: &Account, now: i64) -> bool {
    let locked_out = |g| channel.is_locked() && !in_channel(g, channel, me, now).contains(Permissions::MANAGE_THREADS);
    message.is_editable_by(me.id) && can_change(guild, channel, me, now) && !guild.is_some_and(locked_out)
}

/// Whether `me` may delete `message` in `channel` (483768):
/// [`Message::is_deletable_by`], with Manage Messages there, where messages
/// [`can_change`]. In a moderator report channel, the report itself and
/// system messages stay.
pub fn can_delete(guild: Option<&Guild>, channel: &Channel, message: &Message, me: &Account, now: i64) -> bool {
    let manage = guild.is_some_and(|g| in_channel(g, channel, me, now).contains(Permissions::MANAGE_MESSAGES));
    // The report is the post's first message, which shares the post's id.
    let report =
        channel.flags & IS_MODERATOR_REPORT_CHANNEL != 0 && (message.id == channel.id || !message.is_user_message());
    message.is_deletable_by(me.id, manage) && can_change(guild, channel, me, now) && !report
}

/// Whether Pin Message and Unpin Message show on `message` (`A` in 492841):
/// not on a system message or in the system DM, where we may pin and read
/// the history (a DM needs neither), in a channel messages can still be
/// changed in ([`can_change`]), and not in a voice, stage or media-thread
/// channel. A message that failed to send has none of it in Neucord, which
/// keeps no such message in the list.
pub fn can_pin(guild: Option<&Guild>, channel: &Channel, message: &Message, me: &Account, now: i64) -> bool {
    let allowed = |g| {
        let permissions = in_channel(g, channel, me, now);
        permissions.contains(Permissions::PIN_MESSAGES) && permissions.contains(Permissions::READ_MESSAGE_HISTORY)
    };
    let may = channel.kind.is_private() || guild.is_some_and(allowed);
    message.is_user_message()
        && !channel.is_system_dm()
        && may
        && can_change(guild, channel, me, now)
        && !matches!(channel.kind, ChannelKind::Voice | ChannelKind::Stage | ChannelKind::MediaThread)
}

/// What the client offers of reactions in a channel (`A` in 707985, with
/// what `cw` in 190854 feeds it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reacting {
    /// Add one with an emoji nobody has used on the message: the add button,
    /// and Add Reaction in the message menu.
    pub create: bool,
    /// Click the reactions already there to add or take back ours. This
    /// needs no Add Reactions.
    pub update: bool,
}

/// Whether `me` may react in `channel`, one of `guild`'s or a DM, at `now`.
/// Everything wants a channel messages can still be changed in
/// ([`can_change`]) that isn't a media thread, and a server that isn't
/// holding us back with verification. A new reaction wants Add Reactions
/// too, but a DM wants nothing; neither is for the system DM. Clicking one
/// there is also off while timed out or quarantined by automod.
pub fn reacting(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> Reacting {
    let system = channel.is_system_dm();
    let open = can_change(guild, channel, me, now) && channel.kind != ChannelKind::MediaThread;
    let chat = guild.is_none_or(|g| verification::gate(g, me, now).is_none());
    let add = guild.is_none_or(|g| chat && in_channel(g, channel, me, now).contains(Permissions::ADD_REACTIONS));
    let held_back = guild.is_some_and(|g| {
        g.timed_out_until.is_some_and(|until| until > now) || g.member_flags & AUTOMOD_QUARANTINED != 0
    });
    Reacting { create: open && chat && add && !system, update: open && chat && !system && !held_back }
}

/// Whether `me` may remove other people's reactions in `channel`: Manage
/// Messages there, where messages [`can_change`] (761304's menu entries and
/// the list of who reacted in 112753).
pub fn can_remove_reactions(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    let manage = guild.is_some_and(|g| in_channel(g, channel, me, now).contains(Permissions::MANAGE_MESSAGES));
    manage && can_change(guild, channel, me, now)
}

/// Whether messages in `channel` can be edited or deleted at all (`Id` in
/// 406704): anywhere but an archived thread, unless it's unlocked and we
/// [`can_unarchive`] it, as an edit or delete there reopens it first
/// ([`unarchives_first`]).
fn can_change(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    match guild {
        Some(g) if channel.is_archived() => !channel.is_locked() && can_unarchive(g, channel, me, now),
        _ => true,
    }
}

/// Whether an edit or delete in `channel` reopens it first
/// (`unarchiveThreadIfNecessary` in 7909): an archived thread, unless
/// it's locked and we lack Manage Threads. Sending doesn't.
pub fn unarchives_first(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    let manage = |g| in_channel(g, channel, me, now).contains(Permissions::MANAGE_THREADS);
    channel.is_archived() && (!channel.is_locked() || guild.is_some_and(manage))
}

/// The forum or media channel `post` is a post of: a public thread under one
/// (`isForumPost`, 95701).
fn forum_of<'a>(guild: &'a Guild, post: &Channel) -> Option<&'a Channel> {
    let parent = post.parent_id.and_then(|id| guild.channel(id))?;
    (post.kind == ChannelKind::PublicThread && parent.kind.is_forum_like()).then_some(parent)
}

/// Whether the posts of `forum` we can't see in our lists, the archived ones,
/// may be asked for and searched (`O3` in 435470, and the load in 424798):
/// reading the forum's history. Whether a channel of moderator reports lists
/// them is [`Channel::lists_archived`]'s.
pub fn can_read_post_history(guild: &Guild, forum: &Channel, me: &Account, now: i64) -> bool {
    in_channel(guild, forum, me, now).contains(Permissions::READ_MESSAGE_HISTORY)
}

/// Whether Pin Post and Unpin Post show on `post` (`A` in 41402, with `H_`
/// in 406704): Manage Threads, in a forum or media channel's post.
pub fn can_pin_post(guild: &Guild, post: &Channel, me: &Account, now: i64) -> bool {
    forum_of(guild, post).is_some() && in_channel(guild, post, me, now).contains(Permissions::MANAGE_THREADS)
}

/// A moderator report: a thread or post whose own flags, or its forum's,
/// say so (`V` in 715757, `isModeratorReportChannel` in 95701).
pub(crate) fn is_report(guild: &Guild, thread: &Channel) -> bool {
    let forum = thread.parent_id.and_then(|id| guild.channel(id));
    (thread.flags | forum.map_or(0, |f| f.flags)) & IS_MODERATOR_REPORT_CHANNEL != 0
}

/// Whether Edit Thread, or Edit Post, shows on `thread` (200273): someone with
/// Manage Threads in its parent, or its owner while it isn't locked (`NI` in
/// 406704), as long as it isn't archived, or may be reopened ([`can_change`],
/// `Id`), and not locked for someone without Manage Threads in it (`s5`).
/// Never on a moderator report.
pub fn can_edit_thread(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> bool {
    if !thread.kind.is_thread() || is_report(guild, thread) {
        return false;
    }
    let Some(parent) = thread.parent_id.and_then(|id| guild.channel(id)) else { return false };
    let manages_parent = in_channel(guild, parent, me, now).contains(Permissions::MANAGE_THREADS);
    let owns_open = !thread.is_locked() && thread.owner_id == Some(me.id);
    (manages_parent || owns_open)
        && can_change(Some(guild), thread, me, now)
        && !(thread.is_locked() && !thread_moderator(guild, thread, me, now))
}

/// Manage Threads on `thread` itself (`H_` in 406704): the slowmode of a
/// thread is only theirs to set.
pub fn thread_moderator(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> bool {
    in_channel(guild, thread, me, now).contains(Permissions::MANAGE_THREADS)
}

/// What a thread's menu offers as its close item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Close,
    Open,
}

/// Close or Open on `thread` (182061): to reopen an archived one, what
/// [`can_unarchive`] says; to close an open one, Manage Threads in its
/// parent, or being its owner while it isn't locked (`NI` in 406704).
/// Never on a moderator report.
pub fn thread_close(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> Option<Close> {
    if !thread.kind.is_thread() || is_report(guild, thread) {
        return None;
    }
    if thread.is_archived() {
        return can_unarchive(guild, thread, me, now).then_some(Close::Open);
    }
    let parent = thread.parent_id.and_then(|id| guild.channel(id))?;
    let manage = in_channel(guild, parent, me, now).contains(Permissions::MANAGE_THREADS);
    (manage || (!thread.is_locked() && thread.owner_id == Some(me.id))).then_some(Close::Close)
}

/// What a thread's menu offers as its lock item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Lock,
    Unlock,
}

/// Lock or Unlock on `thread` (707792): Manage Threads on it, and not on a
/// moderator report.
pub fn thread_lock(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> Option<Lock> {
    if !thread.kind.is_thread() || is_report(guild, thread) {
        return None;
    }
    in_channel(guild, thread, me, now)
        .contains(Permissions::MANAGE_THREADS)
        .then(|| if thread.is_locked() { Lock::Unlock } else { Lock::Lock })
}

/// What Delete on a thread's menu deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delete {
    /// A thread.
    Thread,
    /// A forum post, whole.
    Post,
    /// Only the post's own message, as its author may once others replied.
    Message,
}

/// Delete on `thread` (307623): in a plain thread with Manage Threads; in a
/// forum post with Manage Threads, or as its author while nobody has
/// replied (the whole post), or once they have, only its first message.
/// Neucord can't tell that message is gone, so it is taken to be there.
/// Needs View Channel, and never on a moderator report.
pub fn thread_delete(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> Option<Delete> {
    if !thread.kind.is_thread() || is_report(guild, thread) || !guild.can_view(thread, me, now) {
        return None;
    }
    let manage = in_channel(guild, thread, me, now).contains(Permissions::MANAGE_THREADS);
    if forum_of(guild, thread).is_none() {
        return manage.then_some(Delete::Thread);
    }
    let owner = thread.owner_id == Some(me.id);
    let replied = thread.message_count.unwrap_or(0) > 0;
    if manage || (owner && !replied) { Some(Delete::Post) } else { (owner && replied).then_some(Delete::Message) }
}

/// Whether Edit Tags shows on `post` (565846, with `NI` in 406704): for
/// someone with Manage Threads, or the post's author unless it's locked, in
/// a forum that has tags, as long as the post is open or can be reopened
/// ([`can_change`]). Moderator reports have none.
pub fn can_edit_post_tags(guild: &Guild, post: &Channel, me: &Account, now: i64) -> bool {
    let Some(forum) = forum_of(guild, post) else { return false };
    let manage = in_channel(guild, forum, me, now).contains(Permissions::MANAGE_THREADS);
    (manage || (!post.is_locked() && post.owner_id == Some(me.id)))
        && !forum.available_tags.is_empty()
        && forum.flags & IS_MODERATOR_REPORT_CHANNEL == 0
        && can_change(Some(guild), post, me, now)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Snowflake;

    const GUILD: u64 = 1;
    const ME: Snowflake = Snowflake(5);
    const MOD: u64 = 40;
    const MEMBER: u64 = 41;

    const VIEW: Permissions = Permissions::VIEW_CHANNEL;
    const SEND: Permissions = Permissions::SEND_MESSAGES;
    const HISTORY: Permissions = Permissions::READ_MESSAGE_HISTORY;

    fn me() -> Account {
        Account {
            id: ME,
            verified: true,
            email: true,
            phone: false,
            mfa: false,
            nsfw_allowed: false,
            nsfw_set: false,
            age_status: None,
        }
    }

    /// `@everyone` may view, send and read; Mod adds Manage Messages and
    /// Bypass Slowmode.
    fn guild() -> Guild {
        let everyone = (VIEW | SEND | HISTORY).0.to_string();
        let moderator = (Permissions::MANAGE_MESSAGES | Permissions::BYPASS_SLOWMODE).0.to_string();
        let mut guild: Guild = serde_json::from_value(json!({
            "id": GUILD.to_string(), "name": "G", "owner_id": "2",
            "roles": [
                {"id": GUILD.to_string(), "name": "@everyone", "permissions": everyone},
                {"id": MOD.to_string(), "name": "Mod", "permissions": moderator},
                {"id": MEMBER.to_string(), "name": "Member", "permissions": "0"},
            ],
        }))
        .unwrap();
        guild.my_roles = vec![Snowflake(MEMBER)];
        guild
    }

    /// A channel with `(id, deny, allow)` overwrites.
    fn channel(overwrites: &[(u64, Permissions, Permissions)]) -> Channel {
        let overwrites: Vec<_> = overwrites
            .iter()
            .map(|(id, deny, allow)| {
                json!({"id": id.to_string(), "type": 0, "deny": deny.0.to_string(), "allow": allow.0.to_string()})
            })
            .collect();
        serde_json::from_value(json!({"id": "9", "type": 0, "permission_overwrites": overwrites})).unwrap()
    }

    fn can(guild: &Guild, channel: &Channel, permissions: Permissions) -> bool {
        in_channel(guild, channel, &me(), 0).contains(permissions)
    }

    #[test]
    fn whom_the_at_list_offers_can_see_the_channel() {
        let guild = guild();
        let (them, theirs) = (Snowflake(7), [Snowflake(MOD)]);
        let staff = channel(&[(GUILD, VIEW, Permissions::NONE), (MOD, Permissions::NONE, VIEW)]);
        assert!(member_can_view(&guild, &staff, them, &theirs), "by a role's overwrite");
        assert!(!member_can_view(&guild, &staff, them, &[]), "@everyone can't");
        assert!(member_can_view(&guild, &staff, Snowflake(2), &[]), "the owner can");
        let hidden = channel(&[(7, VIEW, Permissions::NONE)]);
        assert!(!member_can_view(&guild, &hidden, them, &theirs), "their own overwrite comes last");
        assert!(member_can_view(&guild, &channel(&[]), them, &[]));
    }

    #[test]
    fn roles_add_up() {
        let mut guild = guild();
        let open = channel(&[]);
        assert!(can(&guild, &open, VIEW | SEND | HISTORY), "from @everyone");
        assert!(!can(&guild, &open, Permissions::BYPASS_SLOWMODE));
        guild.my_roles.push(Snowflake(MOD));
        assert!(can(&guild, &open, Permissions::BYPASS_SLOWMODE | Permissions::MANAGE_MESSAGES));
        guild.my_roles = vec![Snowflake(99)];
        assert!(can(&guild, &open, VIEW), "a role we don't know adds nothing");

        guild.roles.retain(|r| r.id != Snowflake(GUILD));
        assert_eq!(in_guild(&guild, &me(), 0), Permissions::DEFAULT_EVERYONE, "no @everyone role: the default");
    }

    #[test]
    fn overwrites_in_order() {
        let guild = guild();
        // Staff-only announcements: @everyone may not send.
        let announcements = channel(&[(GUILD, SEND, Permissions::NONE)]);
        assert!(can(&guild, &announcements, VIEW) && !can(&guild, &announcements, SEND));
        // A role's allow beats @everyone's deny.
        let staff = channel(&[(GUILD, SEND, Permissions::NONE), (MEMBER, Permissions::NONE, SEND)]);
        assert!(can(&guild, &staff, SEND));
        // Among our roles, an allow beats a deny.
        let mut both = guild.clone();
        both.my_roles.push(Snowflake(MOD));
        let split = channel(&[(MEMBER, SEND, Permissions::NONE), (MOD, Permissions::NONE, SEND)]);
        assert!(can(&both, &split, SEND));
        // Our own overwrite comes last.
        let muted = channel(&[(MEMBER, Permissions::NONE, SEND), (ME.0, SEND, Permissions::NONE)]);
        assert!(!can(&guild, &muted, SEND));
        let hidden = channel(&[(GUILD, VIEW, Permissions::NONE)]);
        assert!(!can(&guild, &hidden, VIEW));
        assert!(can(&guild, &channel(&[(77, VIEW, Permissions::NONE)]), VIEW), "someone else's");
    }

    #[test]
    fn owner_and_administrators() {
        let hidden = channel(&[(GUILD, VIEW | SEND, Permissions::NONE), (ME.0, VIEW | SEND, Permissions::NONE)]);
        let mut owned = guild();
        owned.owner_id = Some(ME);
        assert_eq!(in_channel(&owned, &hidden, &me(), 0), Permissions::ALL, "overwrites don't bind the owner");

        let mut admin = guild();
        admin.roles[2].permissions = Permissions::ADMINISTRATOR;
        assert_eq!(in_channel(&admin, &hidden, &me(), 0), Permissions::ALL);
        assert!(!Permissions::ALL.contains(Permissions(1 << 47)), "only the bits the client knows");
    }

    #[test]
    fn timeouts_and_quarantine_leave_reading() {
        let mut guild = guild();
        guild.timed_out_until = Some(100);
        let open = channel(&[]);
        assert_eq!(in_channel(&guild, &open, &me(), 99), VIEW | HISTORY);
        assert_eq!(in_channel(&guild, &open, &me(), 100), VIEW | SEND | HISTORY, "over");
        guild.roles[2].permissions = Permissions::ADMINISTRATOR;
        assert_eq!(in_channel(&guild, &open, &me(), 99), Permissions::ALL, "administrators are exempt");

        let mut quarantined = self::guild();
        quarantined.member_flags = 1 << 8;
        assert_eq!(in_guild(&quarantined, &me(), 0), VIEW | HISTORY);
        quarantined.roles[0].permissions = VIEW | Permissions::CHANGE_NICKNAME | SEND;
        assert_eq!(in_guild(&quarantined, &me(), 0), VIEW | Permissions::CHANGE_NICKNAME);
    }

    #[test]
    fn a_timeout_or_quarantine_holds_back_a_vote_for_everyone() {
        let mut guild = guild();
        assert!(!vote_held_back(Some(&guild), 0) && !vote_held_back(None, 0), "a DM has neither");
        guild.timed_out_until = Some(100);
        guild.roles[2].permissions = Permissions::ADMINISTRATOR;
        assert!(vote_held_back(Some(&guild), 99), "administrators included");
        assert!(!vote_held_back(Some(&guild), 100));
        guild.timed_out_until = None;
        guild.member_flags = 1 << 10;
        assert!(vote_held_back(Some(&guild), 0));
    }

    #[test]
    fn pending_and_guests_bind_administrators_too() {
        let mut guild = guild();
        guild.roles[2].permissions = Permissions::ADMINISTRATOR;
        guild.pending = true;
        assert_eq!(in_guild(&guild, &me(), 0), VIEW | HISTORY);
        guild.pending = false;
        guild.member_flags = IS_GUEST;
        assert_eq!(in_guild(&guild, &me(), 0), Permissions::GUEST);
        guild.owner_id = Some(ME);
        guild.pending = true;
        assert_eq!(in_guild(&guild, &me(), 0), Permissions::ALL, "but not the owner");
    }

    #[test]
    fn two_factor_for_moderators() {
        let mut guild = guild();
        guild.owner_id = Some(ME);
        guild.requires_mfa = true;
        let without = in_guild(&guild, &me(), 0);
        assert!(!without.contains(Permissions::ADMINISTRATOR) && !without.contains(Permissions::MANAGE_MESSAGES));
        assert!(without.contains(Permissions::BYPASS_SLOWMODE | SEND), "only moderation goes");
        assert_eq!(in_guild(&guild, &Account { mfa: true, ..me() }, 0), Permissions::ALL);
    }

    #[test]
    fn channels_being_deleted() {
        let mut doomed = channel(&[]);
        doomed.flags = IS_SCHEDULED_FOR_DELETION;
        let mut owned = guild();
        owned.owner_id = Some(ME);
        assert_eq!(in_channel(&owned, &doomed, &me(), 0), Permissions::NONE);
    }

    #[test]
    fn a_timeout_stops_replies_even_for_administrators() {
        let mut guild = guild();
        guild.timed_out_until = Some(100);
        guild.roles[2].permissions = Permissions::ADMINISTRATOR;
        let open = channel(&[]);
        assert!(can_send(Some(&guild), &open, &me(), 99), "permissions don't stop them");
        assert!(!can_reply(Some(&guild), &open, &me(), 99));
        assert!(can_reply(Some(&guild), &open, &me(), 100), "over");

        guild.roles[2].permissions = Permissions::NONE;
        guild.owner_id = Some(ME);
        assert!(!can_reply(Some(&guild), &open, &me(), 99), "nor does owning the server");
    }

    #[test]
    fn sending_and_replying() {
        let guild = guild();
        let me = me();
        assert!(can_send(Some(&guild), &channel(&[]), &me, 0) && can_reply(Some(&guild), &channel(&[]), &me, 0));
        let announcements = channel(&[(GUILD, SEND, Permissions::NONE)]);
        assert!(!can_send(Some(&guild), &announcements, &me, 0) && !can_reply(Some(&guild), &announcements, &me, 0));
        let no_history = channel(&[(GUILD, HISTORY, Permissions::NONE)]);
        assert!(can_send(Some(&guild), &no_history, &me, 0), "a message, yes");
        assert!(!can_reply(Some(&guild), &no_history, &me, 0), "a reply, no");
        let unseen = channel(&[(GUILD, VIEW, Permissions::NONE)]);
        assert!(!can_send(Some(&guild), &unseen, &me, 0));

        let dm: Channel =
            serde_json::from_value(json!({"id": "3", "type": 1, "recipients": [{"id": "6", "username": "a"}]}))
                .unwrap();
        assert!(can_send(None, &dm, &me, 0) && can_reply(None, &dm, &me, 0));
        let system: Channel = serde_json::from_value(
            json!({"id": "4", "type": 1, "recipients": [{"id": "7", "username": "discord", "system": true}]}),
        )
        .unwrap();
        assert!(!can_send(None, &system, &me, 0) && !can_reply(None, &system, &me, 0));
    }

    #[test]
    fn forum_and_media_channels_take_no_message() {
        let me = me();
        for kind in [15, 16] {
            let mut forum = channel(&[]);
            forum.kind = serde_json::from_value(json!(kind)).unwrap();
            let mut guild = guild();
            guild.owner_id = Some(ME);
            assert!(can_view(&guild, &forum, &me, 0) && can(&guild, &forum, SEND), "type {kind}: posts, yes");
            assert!(!can_send(Some(&guild), &forum, &me, 0) && !can_reply(Some(&guild), &forum, &me, 0));
        }
    }

    #[test]
    fn voice_and_stage_chats_need_connect() {
        let guild = guild();
        let me = me();
        for kind in [2, 13] {
            let mut voice = channel(&[]);
            voice.kind = serde_json::from_value(json!(kind)).unwrap();
            assert!(!can_view(&guild, &voice, &me, 0), "type {kind}: no Connect from @everyone");
            assert!(!can_send(Some(&guild), &voice, &me, 0) && !can_reply(Some(&guild), &voice, &me, 0));
            voice.permission_overwrites =
                channel(&[(GUILD, Permissions::NONE, Permissions::CONNECT)]).permission_overwrites;
            assert!(can_view(&guild, &voice, &me, 0) && can_send(Some(&guild), &voice, &me, 0));
            assert!(can_reply(Some(&guild), &voice, &me, 0));
        }
        let text = channel(&[(GUILD, Permissions::CONNECT, Permissions::NONE)]);
        assert!(can_view(&guild, &text, &me, 0), "a text channel needs no Connect");
    }

    const IN_THREADS: Permissions = Permissions::SEND_MESSAGES_IN_THREADS;
    const MANAGE_THREADS: Permissions = Permissions::MANAGE_THREADS;

    /// `guild()` with `parent` as channel 9, and a thread of `kind` in it.
    fn with_thread(parent: Channel, kind: u8, extra: serde_json::Value) -> (Guild, Channel) {
        let mut guild = guild();
        guild.channels = vec![parent];
        let mut thread = json!({"id": "20", "type": kind, "parent_id": "9",
                                "thread_metadata": {"archived": false, "locked": false}});
        thread.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        (guild, serde_json::from_value(thread).unwrap())
    }

    fn joined() -> serde_json::Value {
        json!({"member": {"flags": 0, "join_timestamp": "2026-10-28T00:00:00+00:00"}})
    }

    #[test]
    fn attaching_takes_attach_files_with_send_messages_in_servers() {
        let me = me();
        let mut guild = guild();
        let open = channel(&[]);
        assert!(can_send(Some(&guild), &open, &me, 0) && !can_attach(Some(&guild), &open, &me, 0));
        guild.my_roles.push(Snowflake(MOD));
        let moderator = guild.roles.iter_mut().find(|r| r.id == Snowflake(MOD)).unwrap();
        moderator.permissions = moderator.permissions | Permissions::ATTACH_FILES;
        assert!(can_attach(Some(&guild), &open, &me, 0));
        let muted = channel(&[(GUILD, SEND, Permissions::NONE)]);
        assert!(!can_attach(Some(&guild), &muted, &me, 0), "Attach Files alone isn't enough");

        let dm: Channel = serde_json::from_value(json!({"id": "9", "type": 1})).unwrap();
        assert!(can_attach(None, &dm, &me, 0), "a private channel needs no permission");
        let system: Channel = serde_json::from_value(
            json!({"id": "9", "type": 1, "recipients": [{"id": "2", "username": "d", "system": true}]}),
        )
        .unwrap();
        assert!(system.is_system_dm() && !can_attach(None, &system, &me, 0));
    }

    #[test]
    fn recording_a_voice_message_takes_send_voice_messages_in_servers() {
        let me = me();
        let mut guild = guild();
        let open = channel(&[]);
        assert!(can_send(Some(&guild), &open, &me, 0) && !can_record_voice_message(Some(&guild), &open, &me, 0));
        guild.my_roles.push(Snowflake(MOD));
        let moderator = guild.roles.iter_mut().find(|r| r.id == Snowflake(MOD)).unwrap();
        moderator.permissions = moderator.permissions | Permissions::SEND_VOICE_MESSAGES;
        assert!(can_record_voice_message(Some(&guild), &open, &me, 0));
        let muted = channel(&[(GUILD, SEND, Permissions::NONE)]);
        assert!(!can_record_voice_message(Some(&guild), &muted, &me, 0), "Send Voice Messages alone isn't enough");
        let forum: Channel = serde_json::from_value(json!({"id": "9", "type": 15})).unwrap();
        assert!(!can_record_voice_message(Some(&guild), &forum, &me, 0), "never in a forum");

        let dm: Channel = serde_json::from_value(json!({"id": "9", "type": 1})).unwrap();
        assert!(can_record_voice_message(None, &dm, &me, 0), "a private channel needs no permission");
        let system: Channel = serde_json::from_value(
            json!({"id": "9", "type": 1, "recipients": [{"id": "2", "username": "d", "system": true}]}),
        )
        .unwrap();
        assert!(!can_record_voice_message(None, &system, &me, 0));
    }

    #[test]
    fn polls_take_send_polls_in_servers() {
        let (mut guild, me) = (guild(), me());
        let open = channel(&[]);
        assert!(can_send(Some(&guild), &open, &me, 0) && !can_post_polls(Some(&guild), &open, &me, 0));
        guild.my_roles.push(Snowflake(MOD));
        let moderator = guild.roles.iter_mut().find(|r| r.id == Snowflake(MOD)).unwrap();
        moderator.permissions = moderator.permissions | Permissions::SEND_POLLS;
        assert!(can_post_polls(Some(&guild), &open, &me, 0));
        let muted = channel(&[(GUILD, SEND, Permissions::NONE)]);
        assert!(!can_post_polls(Some(&guild), &muted, &me, 0), "Send Polls alone isn't enough");

        let mut forum = channel(&[]);
        forum.kind = serde_json::from_value(json!(15)).unwrap();
        assert!(!can_post_polls(Some(&guild), &forum, &me, 0));
        let dm: Channel = serde_json::from_value(json!({"id": "9", "type": 1})).unwrap();
        assert!(can_post_polls(None, &dm, &me, 0), "a private channel needs no permission");
        let system: Channel = serde_json::from_value(
            json!({"id": "9", "type": 1, "recipients": [{"id": "2", "username": "d", "system": true}]}),
        )
        .unwrap();
        assert!(!can_post_polls(None, &system, &me, 0));
    }

    #[test]
    fn polls_go_in_the_clients_channel_types() {
        let types: Vec<u8> =
            (0..=21).filter(|&t| serde_json::from_value::<ChannelKind>(json!(t)).unwrap().takes_polls()).collect();
        assert_eq!(types, [0, 1, 2, 3, 5, 10, 11, 12, 13, 17, 18, 19, 21]);
    }

    #[test]
    fn threads_post_with_send_messages_in_threads() {
        let me = me();
        let (guild, thread) = with_thread(channel(&[]), 11, json!({}));
        assert!(can_view(&guild, &thread, &me, 0), "seen as its parent is");
        assert!(!can_send(Some(&guild), &thread, &me, 0), "Send Messages alone isn't enough");

        let threads_only = channel(&[(GUILD, SEND, IN_THREADS)]);
        let (guild, thread) = with_thread(threads_only, 11, json!({}));
        assert!(!can_send(Some(&guild), guild.channel(Snowflake(9)).unwrap(), &me, 0));
        assert!(can_send(Some(&guild), &thread, &me, 0), "and Send Messages isn't needed");
        assert!(can_reply(Some(&guild), &thread, &me, 0));

        let hidden = channel(&[(GUILD, VIEW, IN_THREADS)]);
        let (guild, thread) = with_thread(hidden, 11, json!({}));
        assert!(!can_view(&guild, &thread, &me, 0) && !can_send(Some(&guild), &thread, &me, 0));
    }

    /// A forum (channel 9, which has `tags` tags) with a post of ours or of
    /// someone else's.
    fn forum_post(mine: bool, tags: usize, extra: serde_json::Value) -> (Guild, Channel) {
        let mut forum = channel(&[(GUILD, Permissions::NONE, IN_THREADS)]);
        forum.kind = serde_json::from_value(json!(15)).unwrap();
        forum.available_tags = (0..tags)
            .map(|i| serde_json::from_value(json!({"id": (100 + i).to_string(), "name": "t"})).unwrap())
            .collect();
        let owner = if mine { ME.0 } else { 3 };
        let mut extra = extra;
        extra.as_object_mut().unwrap().insert("owner_id".into(), json!(owner.to_string()));
        with_thread(forum, 11, extra)
    }

    #[test]
    fn pinning_and_editing_tags_follow_manage_threads_and_who_made_the_post() {
        let me = me();
        let (mut guild, post) = forum_post(false, 2, json!({}));
        assert!(!can_pin_post(&guild, &post, &me, 0) && !can_edit_post_tags(&guild, &post, &me, 0), "someone else's");
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_pin_post(&guild, &post, &me, 0) && can_edit_post_tags(&guild, &post, &me, 0));

        let (guild, mine) = forum_post(true, 2, json!({}));
        assert!(!can_pin_post(&guild, &mine, &me, 0), "pinning is for moderators");
        assert!(can_edit_post_tags(&guild, &mine, &me, 0), "our own post's tags");
        let (guild, none) = forum_post(true, 0, json!({}));
        assert!(!can_edit_post_tags(&guild, &none, &me, 0), "a forum with no tags");
        let locked = json!({"thread_metadata": {"archived": false, "locked": true}});
        let (guild, locked) = forum_post(true, 2, locked);
        assert!(!can_edit_post_tags(&guild, &locked, &me, 0), "locked");
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, archived) = forum_post(true, 2, archived);
        assert!(can_edit_post_tags(&guild, &archived, &me, 0), "reopened first, which we may");
        let (mut guild, archived) =
            forum_post(true, 2, json!({"thread_metadata": {"archived": true, "locked": false}}));
        guild.channels[0].permission_overwrites[0].allow = Permissions::NONE;
        assert!(!can_edit_post_tags(&guild, &archived, &me, 0), "but not where we may not post");
    }

    #[test]
    fn a_thread_of_a_text_channel_is_no_post() {
        let me = me();
        let (mut guild, thread) = with_thread(channel(&[]), 11, json!({"owner_id": "5"}));
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(!can_pin_post(&guild, &thread, &me, 0) && !can_edit_post_tags(&guild, &thread, &me, 0));
    }

    #[test]
    fn archived_posts_are_asked_for_with_history_in_moderator_reports_too() {
        let me = me();
        let (mut guild, _) = forum_post(false, 0, json!({}));
        assert!(can_read_post_history(&guild, &guild.channels[0].clone(), &me, 0));
        guild.channels[0].permission_overwrites[0].deny = HISTORY;
        assert!(!can_read_post_history(&guild, &guild.channels[0].clone(), &me, 0), "no history");
        guild.channels[0].permission_overwrites[0].deny = Permissions::NONE;
        guild.channels[0].flags = 1 << 19;
        assert!(can_read_post_history(&guild, &guild.channels[0].clone(), &me, 0), "a channel of reports");
        guild.channels[0].permission_overwrites[0].deny = HISTORY;
        assert!(!can_read_post_history(&guild, &guild.channels[0].clone(), &me, 0), "reports without history");
    }

    #[test]
    fn locked_threads_take_manage_threads() {
        let me = me();
        let locked = json!({"thread_metadata": {"archived": false, "locked": true}});
        let (mut guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, locked);
        assert!(can_view(&guild, &thread, &me, 0) && !can_send(Some(&guild), &thread, &me, 0));
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_send(Some(&guild), &thread, &me, 0));
    }

    #[test]
    fn buttons_and_menus_go_off_where_we_cant_chat() {
        let me = me();
        let blocked = |g: &Guild, c: &Channel, now| components_blocked(Some(g), c, &me, now);
        let open = channel(&[]);
        let mut guild = guild();
        assert!(!blocked(&guild, &open, 0) && !components_blocked(None, &open, &me, 0), "a DM has no rules");

        guild.timed_out_until = Some(100);
        guild.roles[2].permissions = Permissions::ADMINISTRATOR;
        assert!(blocked(&guild, &open, 99), "a timeout, administrators too");
        assert!(!blocked(&guild, &open, 100));
        guild.timed_out_until = None;

        guild.pending = true;
        assert!(blocked(&guild, &open, 0), "still pending");
        guild.pending = false;

        // No role to vouch for us, and a level that wants a phone.
        guild.my_roles.clear();
        guild.verification.level = crate::verification::VerificationLevel::VeryHigh;
        assert!(blocked(&guild, &open, 0), "verification");
        guild.verification.screening = true;
        guild.verification.level = crate::verification::VerificationLevel::None;
        assert!(!blocked(&guild, &open, 0), "screening alone isn't part of it");

        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[]), 11, archived);
        assert!(blocked(&guild, &thread, 0), "an archived thread we can't reopen");
        let locked = json!({"thread_metadata": {"archived": false, "locked": true}});
        let (mut guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, locked);
        assert!(blocked(&guild, &thread, 0), "a locked thread");
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(!blocked(&guild, &thread, 0), "unless we manage threads");
    }

    #[test]
    fn private_threads_we_havent_joined_give_nothing() {
        let me = me();
        let parent = channel(&[(GUILD, Permissions::NONE, IN_THREADS)]);
        let (mut guild, private) = with_thread(parent.clone(), 12, json!({}));
        assert_eq!(in_channel(&guild, &private, &me, 0), Permissions::NONE);
        let (_, member) = with_thread(parent.clone(), 12, joined());
        assert!(can_send(Some(&guild), &member, &me, 0));
        let (_, public) = with_thread(parent, 11, json!({}));
        assert!(can_send(Some(&guild), &public, &me, 0), "a public one needs no joining");

        guild.member_flags = IS_GUEST;
        assert!(can_view(&guild, &private, &me, 0), "guests are let in");
        guild.member_flags = 0;
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_send(Some(&guild), &private, &me, 0), "and moderators");
    }

    fn manager(guild: &mut Guild) {
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
    }

    fn forum() -> Channel {
        serde_json::from_value(json!({"id": "9", "type": 15})).unwrap()
    }

    #[test]
    fn closing_and_opening_a_thread() {
        let me = me();
        let owned = json!({"owner_id": "5"});
        let (guild, thread) = with_thread(channel(&[]), 11, owned.clone());
        assert_eq!(thread_close(&guild, &thread, &me, 0), Some(Close::Close), "its owner may close it");
        let (guild, stranger) = with_thread(channel(&[]), 11, json!({"owner_id": "6"}));
        assert_eq!(thread_close(&guild, &stranger, &me, 0), None, "someone else's");
        let (mut guild, stranger) = with_thread(channel(&[]), 11, json!({"owner_id": "6"}));
        manager(&mut guild);
        assert_eq!(thread_close(&guild, &stranger, &me, 0), Some(Close::Close), "a manager may close any");

        let locked = json!({"owner_id": "5", "thread_metadata": {"archived": false, "locked": true}});
        let (guild, thread) = with_thread(channel(&[]), 11, locked);
        assert_eq!(thread_close(&guild, &thread, &me, 0), None, "its owner can't once it is locked");

        let archived = json!({"owner_id": "5", "thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, archived);
        assert_eq!(thread_close(&guild, &thread, &me, 0), Some(Close::Open), "we may post in it, so reopen it");
        let archived = json!({"owner_id": "5", "thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, SEND, IN_THREADS)]), 11, archived);
        assert_eq!(thread_close(&guild, &thread, &me, 0), None, "we may not");

        let report = json!({"owner_id": "5", "flags": IS_MODERATOR_REPORT_CHANNEL});
        let (guild, thread) = with_thread(channel(&[]), 11, report);
        assert_eq!(thread_close(&guild, &thread, &me, 0), None, "never a moderator report");
    }

    #[test]
    fn editing_a_thread() {
        let me = me();
        let (guild, mine) = with_thread(channel(&[]), 11, json!({"owner_id": "5"}));
        assert!(can_edit_thread(&guild, &mine, &me, 0), "its owner");
        let (guild, other) = with_thread(channel(&[]), 11, json!({"owner_id": "6"}));
        assert!(!can_edit_thread(&guild, &other, &me, 0), "someone else's");
        let (mut guild, other) = with_thread(channel(&[]), 11, json!({"owner_id": "6"}));
        manager(&mut guild);
        assert!(can_edit_thread(&guild, &other, &me, 0), "a manager, whoever owns it");

        let locked = json!({"owner_id": "5", "thread_metadata": {"archived": false, "locked": true}});
        let (guild, thread) = with_thread(channel(&[]), 11, locked.clone());
        assert!(!can_edit_thread(&guild, &thread, &me, 0), "its owner once it is locked");
        let (mut guild, thread) = with_thread(channel(&[]), 11, locked);
        manager(&mut guild);
        assert!(can_edit_thread(&guild, &thread, &me, 0), "a manager, though locked");

        let archived = json!({"owner_id": "5", "thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, archived.clone());
        assert!(can_edit_thread(&guild, &thread, &me, 0), "archived, but we may reopen it");
        let (guild, thread) = with_thread(channel(&[(GUILD, SEND, IN_THREADS)]), 11, archived);
        assert!(!can_edit_thread(&guild, &thread, &me, 0), "archived, and we may not");

        let report = json!({"owner_id": "5", "flags": IS_MODERATOR_REPORT_CHANNEL});
        let (guild, thread) = with_thread(channel(&[]), 11, report);
        assert!(!can_edit_thread(&guild, &thread, &me, 0), "never a moderator report");
        assert!(!thread_moderator(&guild, &thread, &me, 0));
    }

    #[test]
    fn locking_and_unlocking_a_thread() {
        let me = me();
        let (mut guild, thread) = with_thread(channel(&[]), 11, json!({"owner_id": "5"}));
        assert_eq!(thread_lock(&guild, &thread, &me, 0), None, "owning it isn't managing it");
        manager(&mut guild);
        assert_eq!(thread_lock(&guild, &thread, &me, 0), Some(Lock::Lock));
        let locked = json!({"thread_metadata": {"archived": false, "locked": true}});
        let (mut guild, thread) = with_thread(channel(&[]), 11, locked);
        manager(&mut guild);
        assert_eq!(thread_lock(&guild, &thread, &me, 0), Some(Lock::Unlock));
        let report = json!({"flags": IS_MODERATOR_REPORT_CHANNEL});
        let (mut guild, thread) = with_thread(channel(&[]), 11, report);
        manager(&mut guild);
        assert_eq!(thread_lock(&guild, &thread, &me, 0), None);
    }

    #[test]
    fn deleting_a_thread_or_a_post() {
        let me = me();
        let (mut guild, thread) = with_thread(channel(&[]), 11, json!({"owner_id": "5"}));
        assert_eq!(thread_delete(&guild, &thread, &me, 0), None, "a plain thread wants Manage Threads");
        manager(&mut guild);
        assert_eq!(thread_delete(&guild, &thread, &me, 0), Some(Delete::Thread));

        let post = |extra: serde_json::Value| with_thread(forum(), 11, extra);
        let (guild, mine) = post(json!({"owner_id": "5", "message_count": 0}));
        assert_eq!(thread_delete(&guild, &mine, &me, 0), Some(Delete::Post), "its author, unanswered");
        let (guild, answered) = post(json!({"owner_id": "5", "message_count": 3}));
        assert_eq!(thread_delete(&guild, &answered, &me, 0), Some(Delete::Message), "answered: only the message");
        let (guild, other) = post(json!({"owner_id": "6", "message_count": 0}));
        assert_eq!(thread_delete(&guild, &other, &me, 0), None, "someone else's");
        let (mut guild, other) = post(json!({"owner_id": "6", "message_count": 3}));
        manager(&mut guild);
        assert_eq!(thread_delete(&guild, &other, &me, 0), Some(Delete::Post), "a manager deletes it whole");
        let (mut guild, report) = post(json!({"owner_id": "6", "flags": IS_MODERATOR_REPORT_CHANNEL}));
        manager(&mut guild);
        assert_eq!(thread_delete(&guild, &report, &me, 0), None, "never a moderator report");
        let (mut guild, hidden) = with_thread(channel(&[(GUILD, VIEW, Permissions::NONE)]), 11, json!({}));
        manager(&mut guild);
        assert_eq!(thread_delete(&guild, &hidden, &me, 0), None, "one we can't see");
    }

    #[test]
    fn media_threads_only_read() {
        let me = me();
        let (mut guild, thread) = with_thread(channel(&[]), 19, joined());
        guild.owner_id = Some(ME);
        assert_eq!(in_channel(&guild, &thread, &me, 0), VIEW | HISTORY, "even for the owner");
        assert!(!can_send(Some(&guild), &thread, &me, 0) && !can_unarchive(&guild, &thread, &me, 0));
    }

    #[test]
    fn threads_without_a_parent_give_nothing() {
        let me = me();
        let (mut guild, thread) = with_thread(channel(&[]), 19, json!({}));
        guild.channels[0].flags = IS_SCHEDULED_FOR_DELETION;
        assert_eq!(in_channel(&guild, &thread, &me, 0), Permissions::NONE, "its parent is being deleted");
        guild.channels.clear();
        assert_eq!(in_channel(&guild, &thread, &me, 0), Permissions::NONE, "its parent is gone");
    }

    #[test]
    fn archived_threads() {
        let me = me();
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, archived);
        assert!(can_send(Some(&guild), &thread, &me, 0), "posting reopens it");
        assert!(can_unarchive(&guild, &thread, &me, 0) && can_reply(Some(&guild), &thread, &me, 0));

        // Send Messages in Threads only: the box stays, Reply goes.
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, SEND, IN_THREADS)]), 11, archived);
        assert!(can_send(Some(&guild), &thread, &me, 0));
        assert!(!can_unarchive(&guild, &thread, &me, 0) && !can_reply(Some(&guild), &thread, &me, 0));

        let locked = json!({"thread_metadata": {"archived": true, "locked": true}});
        let (mut guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, locked);
        assert!(!can_send(Some(&guild), &thread, &me, 0) && !can_reply(Some(&guild), &thread, &me, 0));
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_unarchive(&guild, &thread, &me, 0) && can_reply(Some(&guild), &thread, &me, 0));
        assert!(!can_unarchive(&guild, guild.channel(Snowflake(9)).unwrap(), &me, 0), "not a thread");
    }

    const REACT: Permissions = Permissions::ADD_REACTIONS;

    fn reacting_in(guild: &Guild, channel: &Channel) -> (bool, bool) {
        let r = reacting(Some(guild), channel, &me(), 99);
        (r.create, r.update)
    }

    #[test]
    fn a_new_reaction_takes_add_reactions_and_an_old_one_doesnt() {
        let mut guild = guild();
        let open = channel(&[]);
        assert_eq!(reacting_in(&guild, &open), (false, true));
        guild.roles[2].permissions = REACT;
        assert_eq!(reacting_in(&guild, &open), (true, true));
        let denied = channel(&[(MEMBER, REACT, Permissions::NONE)]);
        assert_eq!(reacting_in(&guild, &denied), (false, true));
    }

    #[test]
    fn dms_need_no_permission_but_not_the_system_dm() {
        let dm = |user: serde_json::Value| -> Channel {
            serde_json::from_value(json!({"id": "9", "type": 1, "recipients": [user]})).unwrap()
        };
        let friend = reacting(None, &dm(json!({"id": "2", "username": "u"})), &me(), 0);
        assert_eq!(friend, Reacting { create: true, update: true });
        let system = reacting(None, &dm(json!({"id": "2", "username": "u", "system": true})), &me(), 0);
        assert_eq!(system, Reacting { create: false, update: false });
    }

    #[test]
    fn a_timeout_or_quarantine_stops_clicking_reactions() {
        let mut guild = guild();
        guild.roles[2].permissions = REACT;
        let open = channel(&[]);
        guild.timed_out_until = Some(100);
        assert_eq!(reacting_in(&guild, &open), (false, false));
        guild.timed_out_until = Some(99);
        assert_eq!(reacting_in(&guild, &open), (true, true), "over");
        guild.timed_out_until = None;
        guild.member_flags = 1 << 7;
        assert_eq!(reacting_in(&guild, &open), (false, false));
    }

    #[test]
    fn verification_holds_reactions_back_with_everything_else() {
        let mut guild = guild();
        guild.roles[0].permissions = VIEW | SEND | HISTORY | REACT;
        guild.my_roles.clear();
        let open = channel(&[]);
        assert_eq!(reacting_in(&guild, &open), (true, true));
        guild.verification.level = crate::verification::VerificationLevel::Low;
        let unclaimed = Account { email: false, ..me() };
        assert_eq!(reacting(Some(&guild), &open, &unclaimed, 99), Reacting { create: false, update: false });
        assert_eq!(reacting(Some(&guild), &open, &me(), 99), Reacting { create: true, update: true });
    }

    #[test]
    fn archived_threads_react_only_where_they_reopen() {
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let parent = channel(&[(GUILD, Permissions::NONE, IN_THREADS | REACT)]);
        let (guild, thread) = with_thread(parent.clone(), 11, archived.clone());
        assert_eq!(reacting_in(&guild, &thread), (true, true), "reacting reopens it");
        let (guild, thread) = with_thread(channel(&[(GUILD, SEND, IN_THREADS | REACT)]), 11, archived);
        assert_eq!(reacting_in(&guild, &thread), (false, false), "Send Messages in the parent alone can't");
        let (guild, media) = with_thread(parent, 19, joined());
        assert_eq!(reacting_in(&guild, &media), (false, false), "a media thread only reads");
    }

    #[test]
    fn removing_others_reactions_takes_manage_messages() {
        let mut guild = guild();
        let open = channel(&[]);
        assert!(!can_remove_reactions(Some(&guild), &open, &me(), 0));
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_remove_reactions(Some(&guild), &open, &me(), 0));
        assert!(!can_remove_reactions(None, &open, &me(), 0), "a DM has no moderators");
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (mut guild, thread) = with_thread(channel(&[(GUILD, Permissions::NONE, IN_THREADS)]), 11, archived);
        guild.roles[1].permissions = guild.roles[1].permissions | IN_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_remove_reactions(Some(&guild), &thread, &me(), 0), "reopened first");
        guild.roles[1].permissions = Permissions::MANAGE_MESSAGES;
        let (_, locked) = with_thread(channel(&[]), 11, json!({"thread_metadata": {"archived": true, "locked": true}}));
        assert!(!can_remove_reactions(Some(&guild), &locked, &me(), 0), "a locked archive stays shut");
    }

    /// A message by `author` in channel `channel`, of `kind`.
    fn message(id: u64, channel: u64, author: Snowflake, kind: u8) -> Message {
        serde_json::from_value(json!({"id": id.to_string(), "channel_id": channel.to_string(), "type": kind,
                                      "author": {"id": author.to_string(), "username": "u"}, "timestamp": ""}))
        .unwrap()
    }

    #[test]
    fn deleting_others_takes_manage_messages() {
        let (me, mut guild, open) = (me(), guild(), channel(&[]));
        let (ours, theirs) = (message(30, 9, ME, 0), message(31, 9, Snowflake(2), 0));
        assert!(can_delete(Some(&guild), &open, &ours, &me, 0) && can_edit(Some(&guild), &open, &ours, &me, 0));
        assert!(!can_delete(Some(&guild), &open, &theirs, &me, 0) && !can_edit(Some(&guild), &open, &theirs, &me, 0));
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_delete(Some(&guild), &open, &theirs, &me, 0), "a moderator's");
        assert!(!can_edit(Some(&guild), &open, &theirs, &me, 0), "but never edited");
        let mut report = channel(&[]);
        report.flags = IS_MODERATOR_REPORT_CHANNEL;
        report.id = Snowflake(31);
        assert!(!can_delete(Some(&guild), &report, &theirs, &me, 0), "the report itself");
        assert!(!can_delete(Some(&guild), &report, &message(32, 31, Snowflake(2), 7), &me, 0), "a system message");
        assert!(can_delete(Some(&guild), &report, &message(33, 31, Snowflake(2), 0), &me, 0), "a reply to it");
        // DMs have no Manage Messages.
        let dm: Channel = serde_json::from_value(json!({"id": "9", "type": 1})).unwrap();
        assert!(can_delete(None, &dm, &ours, &me, 0) && !can_delete(None, &dm, &theirs, &me, 0));
    }

    #[test]
    fn pinning_takes_pin_messages_and_a_message_someone_wrote() {
        let (me, mut guild, open) = (me(), guild(), channel(&[]));
        let theirs = message(31, 9, Snowflake(2), 0);
        assert!(!can_pin(Some(&guild), &open, &theirs, &me, 0), "not without Pin Messages");
        let pinner = (Permissions::PIN_MESSAGES | HISTORY).0.to_string();
        guild.set_role(serde_json::from_value(json!({"id": "42", "name": "Pinner", "permissions": pinner})).unwrap());
        guild.my_roles.push(Snowflake(42));
        assert!(can_pin(Some(&guild), &open, &theirs, &me, 0));
        assert!(!can_pin(Some(&guild), &open, &message(32, 9, Snowflake(2), 7), &me, 0), "a system message");
        let mut voice = channel(&[]);
        voice.kind = ChannelKind::Voice;
        assert!(!can_pin(Some(&guild), &voice, &theirs, &me, 0), "a voice channel's chat");
        // A DM needs neither permission, but not the system's.
        let dm: Channel = serde_json::from_value(json!({"id": "9", "type": 1})).unwrap();
        assert!(can_pin(None, &dm, &theirs, &me, 0));
        let system: Channel = serde_json::from_value(
            json!({"id": "9", "type": 1, "recipients": [{"id": "643945264868098049", "username": "Discord", "system": true}]}),
        )
        .unwrap();
        assert!(!can_pin(None, &system, &theirs, &me, 0));
    }

    #[test]
    fn editing_and_deleting_in_threads() {
        let me = me();
        let parent = channel(&[(GUILD, Permissions::NONE, IN_THREADS)]);
        let ours = message(30, 20, ME, 0);
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(parent.clone(), 11, archived);
        assert!(can_edit(Some(&guild), &thread, &ours, &me, 0) && can_delete(Some(&guild), &thread, &ours, &me, 0));
        assert!(unarchives_first(Some(&guild), &thread, &me, 0), "reopened first");
        let (guild, thread) = with_thread(parent.clone(), 11, json!({}));
        assert!(!unarchives_first(Some(&guild), &thread, &me, 0), "already open");

        // Archived where we couldn't reopen it.
        let archived = json!({"thread_metadata": {"archived": true, "locked": false}});
        let (guild, thread) = with_thread(channel(&[(GUILD, SEND, IN_THREADS)]), 11, archived);
        assert!(!can_edit(Some(&guild), &thread, &ours, &me, 0) && !can_delete(Some(&guild), &thread, &ours, &me, 0));

        // Locked and open: deleting ours still works, editing takes Manage Threads.
        let locked = json!({"thread_metadata": {"archived": false, "locked": true}});
        let (mut guild, thread) = with_thread(parent.clone(), 11, locked);
        assert!(!can_edit(Some(&guild), &thread, &ours, &me, 0) && can_delete(Some(&guild), &thread, &ours, &me, 0));
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(can_edit(Some(&guild), &thread, &ours, &me, 0));

        // Locked and archived: nothing, even for moderators, who could reopen it.
        let locked = json!({"thread_metadata": {"archived": true, "locked": true}});
        let (mut guild, thread) = with_thread(parent, 11, locked);
        guild.roles[1].permissions = MANAGE_THREADS;
        guild.my_roles.push(Snowflake(MOD));
        assert!(!can_edit(Some(&guild), &thread, &ours, &me, 0) && !can_delete(Some(&guild), &thread, &ours, &me, 0));
        assert!(unarchives_first(Some(&guild), &thread, &me, 0), "a moderator's other changes reopen it");
    }

    #[test]
    fn parse_permissions() {
        let parse = |v| serde_json::from_value::<Permissions>(v).unwrap();
        assert_eq!(parse(json!("4503599627370496")), Permissions::BYPASS_SLOWMODE);
        assert_eq!(parse(json!(1024)), VIEW);
        assert_eq!(parse(json!(null)), Permissions::NONE);
        assert!(serde_json::from_value::<Permissions>(json!("lots")).is_err());
    }

    /// A server where `@everyone` has `everyone`; Mod (position 3), Member
    /// (2) and Helper (2, a larger id) give nothing more.
    fn ranked(everyone: Permissions, mine: &[u64]) -> Guild {
        let mut guild: Guild = serde_json::from_value(json!({
            "id": GUILD.to_string(), "name": "G", "owner_id": "2",
            "roles": [
                {"id": GUILD.to_string(), "name": "@everyone", "permissions": everyone.0.to_string()},
                {"id": "40", "name": "Mod", "permissions": "0", "position": 3},
                {"id": "41", "name": "Member", "permissions": "0", "position": 2},
                {"id": "42", "name": "Helper", "permissions": "0", "position": 2},
                {"id": "43", "name": "Admin", "permissions": Permissions::ADMINISTRATOR.0.to_string(), "position": 1},
            ],
        }))
        .unwrap();
        guild.my_roles = mine.iter().map(|&id| Snowflake(id)).collect();
        guild
    }

    #[test]
    fn managing_needs_a_higher_role() {
        let kick = Permissions::KICK_MEMBERS;
        let target = Snowflake(6);
        let can = |guild: &Guild, roles: Option<&[u64]>| {
            let roles: Option<Vec<Snowflake>> = roles.map(|r| r.iter().map(|&id| Snowflake(id)).collect());
            can_manage(guild, &me(), target, roles.as_deref(), kick, 0)
        };
        let guild = ranked(kick, &[40]);
        assert!(can(&guild, Some(&[41])), "above");
        assert!(!can(&guild, Some(&[40])), "the same role");
        assert!(can(&guild, Some(&[])), "they have none");
        assert!(can(&guild, None), "their roles unknown");
        assert!(!can(&ranked(kick, &[41]), Some(&[40])), "below");
        // The same position: the older role is above.
        assert!(can(&ranked(kick, &[41]), Some(&[42])));
        assert!(!can(&ranked(kick, &[42]), Some(&[41])));
        assert!(!can(&ranked(kick, &[]), None), "we have no role");
        assert!(!can(&ranked(Permissions::NONE, &[40]), Some(&[41])), "the permission missing");
        assert!(!can_manage(&guild, &me(), Snowflake(2), None, kick, 0), "never the owner");
        let mut owned = ranked(Permissions::NONE, &[]);
        owned.owner_id = Some(ME);
        assert!(can(&owned, Some(&[40])), "we own it");
    }

    #[test]
    fn timeouts_spare_admins_owners_and_webhooks() {
        let guild = ranked(Permissions::MODERATE_MEMBERS, &[40]);
        let user = |json: serde_json::Value| serde_json::from_value::<User>(json).unwrap();
        let pat = user(json!({"id": "6", "username": "pat"}));
        let roles = [Snowflake(41)];
        let out = |target: Option<&User>, roles: Option<&[Snowflake]>| can_time_out(&guild, &me(), target, roles, 0);
        assert!(out(Some(&pat), Some(&roles)));
        assert!(!out(None, Some(&roles)), "someone nobody knows");
        assert!(!out(Some(&pat), Some(&[Snowflake(43)])), "an administrator, whatever their rank");
        let hook = user(json!({"id": "6", "username": "hook", "bot": true, "discriminator": "0000"}));
        assert!(!out(Some(&hook), Some(&roles)));
        let owner = user(json!({"id": "2", "username": "own"}));
        assert!(!out(Some(&owner), None));
        assert!(!can_time_out(&ranked(Permissions::NONE, &[40]), &me(), Some(&pat), Some(&roles), 0));
        // Administrator on `@everyone` makes everyone one.
        let everyone_admin = ranked(Permissions::MODERATE_MEMBERS | Permissions::ADMINISTRATOR, &[40]);
        assert!(!can_time_out(&everyone_admin, &me(), Some(&pat), Some(&roles), 0));
    }
}
