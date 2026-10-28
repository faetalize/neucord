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
//! Not copied: lurking and viewing a server as a role, which Neucord can't
//! do, a public stage's wider allowance for lurkers, and threads, which take
//! their parent's permissions with rules of their own (#23).

use std::ops::{BitAnd, BitOr};

use serde::{Deserialize, Deserializer};

use crate::verification::Account;
use crate::{Channel, Guild};

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

/// Ours in `guild` as a whole, at `now` (seconds since 1970).
pub fn in_guild(guild: &Guild, me: &Account, now: i64) -> Permissions {
    compute(guild, &[], me, now)
}

/// Ours in `channel`, one of `guild`'s, at `now`.
pub fn in_channel(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> Permissions {
    if channel.flags & IS_SCHEDULED_FOR_DELETION != 0 {
        return Permissions::NONE;
    }
    compute(guild, &channel.permission_overwrites, me, now)
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
    let everyone = guild.role(guild.id).map_or(Permissions::DEFAULT_EVERYONE, |r| r.permissions);
    let base = guild.my_roles.iter().filter_map(|&id| guild.role(id)).fold(everyone, |p, r| p | r.permissions);
    if base.contains(Permissions::ADMINISTRATOR) {
        return Permissions::ALL;
    }
    // Overwrites are keyed by id: `@everyone`'s is the server's own.
    let find = |id| overwrites.iter().rfind(|o| o.id == id);
    let mut permissions = base;
    if let Some(o) = find(guild.id) {
        permissions = permissions.overwritten(o.deny, o.allow);
    }
    let (deny, allow) = guild
        .my_roles
        .iter()
        .filter_map(|&id| find(id))
        .fold((Permissions::NONE, Permissions::NONE), |(deny, allow), o| (deny | o.deny, allow | o.allow));
    permissions = permissions.overwritten(deny, allow);
    if let Some(o) = find(me.id) {
        permissions = permissions.overwritten(o.deny, o.allow);
    }
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

/// Whether we may post in `channel`: anywhere but the system DM outside
/// servers, and with View Channel and Send Messages in one. The client asks
/// only for the latter, but never opens a channel we can't view.
pub fn can_send(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    match guild {
        None => !channel.is_system_dm(),
        Some(guild) => {
            in_channel(guild, channel, me, now).contains(Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES)
        }
    }
}

/// Whether the client offers Reply in `channel` (517997), with
/// [`crate::Message::is_replyable`] for the message itself: not in the
/// system DM, and in a server only with Send Messages and Read Message
/// History.
pub fn can_reply(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    let history = Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
    can_send(guild, channel, me, now) && guild.is_none_or(|g| in_channel(g, channel, me, now).contains(history))
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
        Account { id: ME, verified: true, email: true, phone: false, mfa: false }
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
    fn parse_permissions() {
        let parse = |v| serde_json::from_value::<Permissions>(v).unwrap();
        assert_eq!(parse(json!("4503599627370496")), Permissions::BYPASS_SLOWMODE);
        assert_eq!(parse(json!(1024)), VIEW);
        assert_eq!(parse(json!(null)), Permissions::NONE);
        assert!(serde_json::from_value::<Permissions>(json!("lots")).is_err());
    }
}
