//! The subset of Discord's data model Neucord uses so far. Unknown fields are
//! ignored, so Discord adding fields never breaks parsing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::embed::Embed;
use crate::emoji::GuildEmoji;
use crate::permissions::{self, Permissions};
use crate::stickers::Listed;
use crate::verification::{Account, Rules};

/// Discord IDs are 64-bit integers sent as JSON strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Snowflake(pub u64);

impl Snowflake {
    const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

    /// When it was made, in milliseconds since 1970.
    pub fn unix_ms(self) -> i64 {
        ((self.0 >> 22) + Self::DISCORD_EPOCH_MS) as i64
    }

    /// The first snowflake of the millisecond `unix_ms` (`fromTimestamp` in
    /// 811091): 0 for anything before Discord's epoch.
    pub fn from_unix_ms(unix_ms: i64) -> Self {
        Snowflake(((unix_ms - Self::DISCORD_EPOCH_MS as i64).max(0) as u64) << 22)
    }

    /// A snowflake for "now", used as a message nonce.
    pub fn now() -> Self {
        let unix_ms = (glib::real_time() / 1000) as u64;
        Snowflake((unix_ms - Self::DISCORD_EPOCH_MS) << 22)
    }
}

impl std::fmt::Display for Snowflake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Snowflake {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map(Snowflake).map_err(serde::de::Error::custom)
    }
}

fn null_is_false<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

fn null_is_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A custom emoji's id, which the client reads as none when it is 0 (`et` in
/// 95701): Discord sends null, and an id as text.
fn emoji_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Snowflake>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(u64),
    }
    let id = match Option::<Id>::deserialize(d)? {
        Some(Id::Text(text)) => text.parse().ok(),
        Some(Id::Number(number)) => Some(number),
        None => None,
    };
    Ok(id.filter(|&id| id != 0).map(Snowflake))
}

/// An ISO 8601 time, in milliseconds since 1970.
fn iso_ms<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let text = Option::<String>::deserialize(d)?;
    let time = text.and_then(|t| glib::DateTime::from_iso8601(&t, Some(&glib::TimeZone::utc())).ok());
    Ok(time.map(|t| t.to_unix() * 1000 + i64::from(t.microsecond()) / 1000))
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
    /// Discord's own account, which sends official notifications.
    #[serde(default, deserialize_with = "null_is_false")]
    pub system: bool,
    /// `0000` for a webhook, which shows as a bot; people and bots have none
    /// or one that counts.
    #[serde(default)]
    pub discriminator: Option<String>,
}

impl User {
    /// What Discord shows: the display name if set, else the username.
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }

    /// An account that is no one's: Discord's own, or a webhook's name. The
    /// client (`isNonUserBot`, 619019) offers them no profile.
    pub fn non_user_bot(&self) -> bool {
        self.system || (self.bot && self.discriminator.as_deref() == Some("0000"))
    }

    /// `username#1234`, or the username alone for an account with no
    /// discriminator that counts (`0`, a webhook's `0000`): `getUserTag` in
    /// 427262, in its default full mode without the `@`.
    pub fn tag(&self) -> String {
        if self.username.is_empty() {
            return "???".into();
        }
        match self.discriminator.as_deref() {
            Some(d) if d != "0" && d != "0000" => format!("{}#{d}", self.username),
            _ => self.username.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Text,
    /// A one-to-one direct message.
    Dm,
    Voice,
    /// A group DM, with up to ten people.
    GroupDm,
    Category,
    Announcement,
    /// A thread under an announcement channel.
    AnnouncementThread,
    /// A thread anyone who sees its parent can open, and a forum or media
    /// channel's post.
    PublicThread,
    /// A thread only its members and moderators see.
    PrivateThread,
    Stage,
    Forum,
    /// A forum whose posts show as a grid of media.
    Media,
    /// Its own kind of thread (19), which only ever takes reading.
    MediaThread,
    Other(u8),
}

impl ChannelKind {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Text,
            1 => Self::Dm,
            2 => Self::Voice,
            3 => Self::GroupDm,
            4 => Self::Category,
            5 => Self::Announcement,
            10 => Self::AnnouncementThread,
            11 => Self::PublicThread,
            12 => Self::PrivateThread,
            13 => Self::Stage,
            15 => Self::Forum,
            16 => Self::Media,
            19 => Self::MediaThread,
            other => Self::Other(other),
        }
    }

    /// The number Discord gives the kind.
    pub fn raw(self) -> u8 {
        match self {
            Self::Text => 0,
            Self::Dm => 1,
            Self::Voice => 2,
            Self::GroupDm => 3,
            Self::Category => 4,
            Self::Announcement => 5,
            Self::AnnouncementThread => 10,
            Self::PublicThread => 11,
            Self::PrivateThread => 12,
            Self::Stage => 13,
            Self::Forum => 15,
            Self::Media => 16,
            Self::MediaThread => 19,
            Self::Other(raw) => raw,
        }
    }

    /// Channels that hold a plain message history.
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::Announcement | Self::Dm | Self::GroupDm)
    }

    /// Voice and stage channels (`isGuildVocal` in 95701). Each also has a text
    /// chat, which Neucord shows without joining the call.
    pub fn is_vocal(self) -> bool {
        matches!(self, Self::Voice | Self::Stage)
    }

    /// DMs and group DMs: channels outside any guild.
    pub fn is_private(self) -> bool {
        matches!(self, Self::Dm | Self::GroupDm)
    }

    /// The client's thread types (`Le` in 95701), which take their parent's
    /// permissions.
    pub fn is_thread(self) -> bool {
        matches!(self, Self::AnnouncementThread | Self::PublicThread | Self::PrivateThread | Self::MediaThread)
    }

    /// Channels made of posts, which never take a message: posting starts a
    /// thread.
    pub fn is_forum_like(self) -> bool {
        matches!(self, Self::Forum | Self::Media)
    }

    /// The channel types polls may be posted in (`POLLS` in 478437): not
    /// categories, forums or media channels.
    pub fn takes_polls(self) -> bool {
        matches!(
            self,
            Self::Text
                | Self::Dm
                | Self::Voice
                | Self::GroupDm
                | Self::Announcement
                | Self::AnnouncementThread
                | Self::PublicThread
                | Self::PrivateThread
                | Self::Stage
                | Self::MediaThread
                | Self::Other(17 | 18 | 21)
        )
    }
}

impl<'de> Deserialize<'de> for ChannelKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u8::deserialize(d).map(Self::from_raw)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Channel {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: ChannelKind,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i32>,
    #[serde(default)]
    pub parent_id: Option<Snowflake>,
    /// A server channel's topic, in message markup.
    #[serde(default)]
    pub topic: Option<String>,
    /// The other people in a DM or group DM (never includes us).
    #[serde(default)]
    pub recipients: Vec<User>,
    /// Group DM icon hash.
    #[serde(default)]
    pub icon: Option<String>,
    /// Who made a group DM, whom its member list marks.
    #[serde(default)]
    pub owner_id: Option<Snowflake>,
    /// Names the member list the channel shows, where Discord says (see
    /// [`crate::member_list::list_id`]).
    #[serde(default)]
    pub member_list_id: Option<String>,
    /// Newest message, used to sort DMs by recent activity. May point at a
    /// deleted message; it only ever needs to be roughly right.
    #[serde(default)]
    pub last_message_id: Option<Snowflake>,
    /// A DM from someone we share no friendship with, waiting to be accepted.
    #[serde(default, rename = "is_message_request", deserialize_with = "null_is_false")]
    pub message_request: bool,
    /// A message request Discord filed as spam.
    #[serde(default, rename = "is_spam", deserialize_with = "null_is_false")]
    pub spam: bool,
    /// Slowmode: seconds each member must wait between messages; 0 when off.
    #[serde(default, deserialize_with = "null_is_default")]
    pub rate_limit_per_user: u32,
    /// A server channel's overwrites of its roles' permissions.
    #[serde(default, deserialize_with = "null_is_default")]
    pub permission_overwrites: Vec<Overwrite>,
    #[serde(default, deserialize_with = "null_is_default")]
    pub flags: u64,
    /// Age-restricted.
    #[serde(default, deserialize_with = "null_is_false")]
    pub nsfw: bool,
    /// A thread's state.
    #[serde(default)]
    pub thread_metadata: Option<ThreadMetadata>,
    /// Minutes without activity before a thread started here archives
    /// itself, when its starter doesn't pick one.
    #[serde(default)]
    pub default_auto_archive_duration: Option<u32>,
    /// The app that manages a group DM (`isManaged`), which /leave's
    /// confirmation names.
    #[serde(default)]
    pub application_id: Option<Snowflake>,
    /// Ours, in a thread we've joined.
    #[serde(default)]
    pub member: Option<ThreadMember>,
    /// When something other than a message last happened in a thread, in
    /// milliseconds since 1970.
    #[serde(default, rename = "last_non_message_activity_timestamp", deserialize_with = "iso_ms")]
    pub last_activity: Option<i64>,
    /// How many messages a thread holds, as of when Discord sent it.
    #[serde(default)]
    pub message_count: Option<u32>,
    /// The tags a forum or media channel's posts may carry.
    #[serde(default, deserialize_with = "null_is_default")]
    pub available_tags: Vec<Tag>,
    /// The tags a post carries.
    #[serde(default, deserialize_with = "null_is_default")]
    pub applied_tags: Vec<Snowflake>,
    /// A forum or media channel's defaults for how its posts are shown, as
    /// Discord sends them: see [`crate::forum`].
    #[serde(default)]
    pub default_sort_order: Option<u8>,
    #[serde(default)]
    pub default_forum_layout: Option<u8>,
    #[serde(default)]
    pub default_tag_setting: Option<String>,
    /// The reaction a post's card offers.
    #[serde(default)]
    pub default_reaction_emoji: Option<ReactionEmoji>,
}

/// A label posts in a forum or media channel may carry.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Tag {
    pub id: Snowflake,
    #[serde(default)]
    pub name: String,
    /// A custom emoji, or none.
    #[serde(default, deserialize_with = "emoji_id")]
    pub emoji_id: Option<Snowflake>,
    /// A Unicode emoji, or a custom one's name.
    #[serde(default)]
    pub emoji_name: Option<String>,
    /// Only members with Manage Threads may apply it.
    #[serde(default, deserialize_with = "null_is_false")]
    pub moderated: bool,
}

/// A forum's default reaction: a custom emoji or a Unicode one.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReactionEmoji {
    #[serde(default, deserialize_with = "emoji_id")]
    pub emoji_id: Option<Snowflake>,
    #[serde(default)]
    pub emoji_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ThreadMetadata {
    #[serde(default, deserialize_with = "null_is_false")]
    pub archived: bool,
    /// Only members with Manage Threads may post, or reopen it.
    #[serde(default, deserialize_with = "null_is_false")]
    pub locked: bool,
    /// Minutes without activity before it archives itself.
    #[serde(default, deserialize_with = "null_is_default")]
    pub auto_archive_duration: u32,
    /// When it was last archived or reopened, in milliseconds since 1970.
    #[serde(default, deserialize_with = "iso_ms")]
    pub archive_timestamp: Option<i64>,
}

/// Our membership of a thread.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ThreadMember {
    /// Which messages notify (see [`crate::notifications`]).
    #[serde(default, deserialize_with = "null_is_default")]
    pub flags: u64,
    #[serde(default, deserialize_with = "null_is_false")]
    pub muted: bool,
    #[serde(default)]
    pub mute_config: Option<crate::notifications::MuteConfig>,
    /// In milliseconds since 1970.
    #[serde(default, deserialize_with = "iso_ms")]
    pub join_timestamp: Option<i64>,
}

impl ThreadMember {
    /// Muted, and the mute hasn't run out.
    pub fn is_muted(&self, now_unix: i64) -> bool {
        crate::notifications::mute_active(self.muted, self.mute_config.as_ref(), now_unix)
    }
}

/// Permissions a channel denies and allows a role or a member, which the
/// official client tells apart by id alone.
#[derive(Clone, Debug, Deserialize)]
pub struct Overwrite {
    pub id: Snowflake,
    #[serde(default)]
    pub allow: Permissions,
    #[serde(default)]
    pub deny: Permissions,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Role {
    pub id: Snowflake,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub permissions: Permissions,
    /// Given by an integration (a bot, boosting), never by hand.
    #[serde(default, deserialize_with = "null_is_false")]
    pub managed: bool,
    /// Higher is above; `@everyone` is always at the bottom.
    #[serde(default, deserialize_with = "null_is_default")]
    pub position: i32,
    /// `0xRRGGBB`, or 0 for none.
    #[serde(default, deserialize_with = "null_is_default")]
    pub color: u32,
    /// A gradient, in servers that have them (see [`crate::members`]).
    #[serde(default)]
    pub colors: Option<RoleColors>,
    /// Given for linking an account of another service, which marks the role
    /// by its tags holding `guild_connections`, with no value.
    #[serde(default, rename = "tags", deserialize_with = "has_guild_connections")]
    pub linked: bool,
}

fn has_guild_connections<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let tags = Option::<serde_json::Map<String, Value>>::deserialize(d)?;
    Ok(tags.is_some_and(|tags| tags.contains_key("guild_connections")))
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RoleColors {
    #[serde(default, deserialize_with = "null_is_default")]
    pub primary_color: u32,
    #[serde(default)]
    pub secondary_color: Option<u32>,
    #[serde(default)]
    pub tertiary_color: Option<u32>,
}

impl Channel {
    /// A server channel Discord hides from an account that may not see it
    /// (see [`crate::obfuscation`]).
    pub fn is_obfuscated(&self) -> bool {
        self.flags & crate::obfuscation::FLAG != 0
    }

    /// The name the client shows for the channel (`m1` in 47167): a hidden
    /// channel is "No Access" whatever it is called.
    pub fn shown_name(&self) -> Option<&str> {
        if self.is_obfuscated() { Some(crate::obfuscation::NO_ACCESS) } else { self.name.as_deref() }
    }

    /// What the sidebar and header call this channel: its name if it has one,
    /// else (DMs, unnamed group DMs) the recipients' names.
    pub fn title(&self) -> String {
        if let Some(name) = self.shown_name().filter(|n| !n.is_empty()) {
            return name.to_owned();
        }
        let names: Vec<&str> = self.recipients.iter().map(User::display_name).collect();
        if names.is_empty() { "Unnamed".to_owned() } else { names.join(", ") }
    }

    /// The channel's link in Discord's web app, as Copy Link copies it
    /// (`af` in 147036, with the path from `CHANNEL` in 901123). `guild` is
    /// its server, `None` for a DM.
    pub fn link(&self, guild: Option<Snowflake>) -> String {
        let guild = guild.map_or("@me".to_owned(), |g| g.to_string());
        format!("https://discord.com/channels/{guild}/{}", self.id)
    }

    /// A message request or spam: kept out of the DM list until accepted.
    pub fn is_request(&self) -> bool {
        self.message_request || self.spam
    }

    /// The DM with Discord's own account. It takes no messages: a message
    /// there is one no official client can send.
    pub fn is_system_dm(&self) -> bool {
        self.kind == ChannelKind::Dm && self.recipients.first().is_some_and(|u| u.system)
    }

    /// Sort key for the DM list: most recent activity first.
    pub fn activity(&self) -> Snowflake {
        self.last_message_id.unwrap_or(self.id)
    }

    /// A thread that is archived: it shows in no list, and reopens when
    /// someone posts in it.
    pub fn is_archived(&self) -> bool {
        self.kind.is_thread() && self.thread_metadata.as_ref().is_some_and(|m| m.archived)
    }

    /// A thread only moderators may post in.
    pub fn is_locked(&self) -> bool {
        self.kind.is_thread() && self.thread_metadata.as_ref().is_some_and(|m| m.locked)
    }

    /// A thread that isn't archived.
    pub fn is_active_thread(&self) -> bool {
        self.kind.is_thread() && !self.is_archived()
    }

    /// A forum post pinned to the top of its forum.
    pub fn is_pinned(&self) -> bool {
        self.flags & Self::PINNED != 0
    }

    const PINNED: u64 = 1 << 1;
}

/// `GuildNSFWContentLevel` (652215): `EXPLICIT` and `AGE_RESTRICTED`.
const AGE_RESTRICTED_LEVELS: [u8; 2] = [1, 3];

#[derive(Clone, Debug)]
pub struct Guild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub channels: Vec<Channel>,
    /// Threads we know of: READY and GUILD_CREATE bring the active ones
    /// we've joined, `THREAD_*` events the rest.
    pub threads: Vec<Channel>,
    /// Notification level for members who never changed it: 0 all, 1 mentions.
    pub default_message_notifications: u8,
    /// The server's owner, who has every permission.
    pub owner_id: Option<Snowflake>,
    /// Every role, `@everyone`'s (the server's own id) included.
    pub roles: Vec<Role>,
    /// Moderation takes 2FA on the account (`mfa_level` 1).
    pub requires_mfa: bool,
    /// Roles may be gradients (`ENHANCED_ROLE_COLORS`).
    pub gradient_roles: bool,
    /// Our roles here. Filled in by the gateway.
    pub my_roles: Vec<Snowflake>,
    /// When our timeout here ends, in seconds since 1970. Filled in by the
    /// gateway; may lie in the past.
    pub timed_out_until: Option<i64>,
    /// What the server asks before members can talk.
    pub verification: Rules,
    /// When we joined, in seconds since 1970.
    pub joined_at: Option<i64>,
    /// Membership screening holds us until we accept the server's rules.
    pub pending: bool,
    /// Our member flags here.
    pub member_flags: u64,
    /// What the server has unlocked or opted into, such as `COMMUNITY`.
    pub features: Vec<String>,
    /// From READY or GUILD_CREATE; updates don't carry it.
    pub member_count: Option<u64>,
    pub hub_type: Option<u8>,
    /// `GuildNSFWContentLevel` (652215) as Discord rates the server, and as
    /// its owner set it: `DEFAULT` (0) when absent.
    pub nsfw_level: u8,
    pub owner_configured_content_level: u8,
    pub vanity_url_code: Option<String>,
    /// Boost level, 0 to 3.
    pub premium_tier: u8,
    /// The channel with the server's rules.
    pub rules_channel_id: Option<Snowflake>,
    /// The server's custom emoji, every one of them: whether we may use one
    /// is `emoji::Library`'s to say.
    pub emojis: Vec<GuildEmoji>,
    /// The server's stickers, every one of them: whether we may send one is
    /// `stickers::Library`'s to say.
    pub stickers: Vec<Listed>,
}

/// What Neucord uses of our own member in a server, which READY, GUILD_CREATE
/// and GUILD_MEMBER_UPDATE carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member {
    pub roles: Vec<Snowflake>,
    /// `communication_disabled_until`, rounded up to the second.
    pub timed_out_until: Option<i64>,
    /// Rounded up to the second too, so the time-based gates never lift early.
    pub joined_at: Option<i64>,
    pub pending: bool,
    pub flags: u64,
}

impl Member {
    pub fn from_value(value: &Value) -> Member {
        let id = |v: &Value| v.as_str()?.parse().ok().map(Snowflake);
        Member {
            roles: value["roles"].as_array().into_iter().flatten().filter_map(id).collect(),
            timed_out_until: unix_rounded_up(&value["communication_disabled_until"]),
            joined_at: unix_rounded_up(&value["joined_at"]),
            pending: value["pending"] == true,
            flags: value["flags"].as_u64().unwrap_or(0),
        }
    }
}

/// An ISO 8601 time, in seconds since 1970, a part second counting as whole.
pub(crate) fn unix_rounded_up(value: &Value) -> Option<i64> {
    let time = glib::DateTime::from_iso8601(value.as_str()?, Some(&glib::TimeZone::utc())).ok()?;
    Some(time.to_unix() + i64::from(time.microsecond() > 0))
}

impl Guild {
    pub fn set_member(&mut self, member: Member) {
        self.my_roles = member.roles;
        self.timed_out_until = member.timed_out_until;
        // READY's members may leave it out; the guild has it too.
        self.joined_at = member.joined_at.or(self.joined_at);
        self.pending = member.pending;
        self.member_flags = member.flags;
    }

    /// The server's settings changed (GUILD_UPDATE): take them from `update`,
    /// keeping the channels and our member.
    pub fn update(&mut self, update: Guild) {
        self.name = update.name;
        self.icon = update.icon;
        self.default_message_notifications = update.default_message_notifications;
        // A transferred server stops exempting its old owner.
        self.owner_id = update.owner_id.or(self.owner_id);
        if !update.roles.is_empty() {
            self.roles = update.roles;
        }
        self.requires_mfa = update.requires_mfa;
        self.gradient_roles = update.gradient_roles;
        self.verification = update.verification;
        self.features = update.features;
        self.member_count = update.member_count.or(self.member_count);
        self.hub_type = update.hub_type;
        self.nsfw_level = update.nsfw_level;
        self.owner_configured_content_level = update.owner_configured_content_level;
        self.vanity_url_code = update.vanity_url_code;
        self.premium_tier = update.premium_tier;
        self.rules_channel_id = update.rules_channel_id;
        // Like roles: an update without any is one that doesn't mention them.
        if !update.emojis.is_empty() {
            self.emojis = update.emojis;
        }
        if !update.stickers.is_empty() {
            self.stickers = update.stickers;
        }
    }

    /// Whether the server is age-restricted (`wh` in 260509): by
    /// `nsfw_level` for accounts in `2025-09-server-nsfw-level`, else by the
    /// level its owner set.
    pub fn age_restricted(&self, by_nsfw_level: bool) -> bool {
        let level = if by_nsfw_level { self.nsfw_level } else { self.owner_configured_content_level };
        AGE_RESTRICTED_LEVELS.contains(&level)
    }

    /// One of the server's channels, not counting threads.
    pub fn channel(&self, id: Snowflake) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn thread(&self, id: Snowflake) -> Option<&Channel> {
        self.threads.iter().find(|c| c.id == id)
    }

    /// A channel or a thread.
    pub fn find(&self, id: Snowflake) -> Option<&Channel> {
        self.channel(id).or_else(|| self.thread(id))
    }

    /// A thread was created or changed. An update leaves out our membership,
    /// and may know of no newer message than we do.
    pub fn set_thread(&mut self, mut thread: Channel) {
        match self.threads.iter_mut().find(|c| c.id == thread.id) {
            Some(existing) => {
                if thread.member.is_none() {
                    thread.member = existing.member.take();
                }
                thread.last_message_id = thread.last_message_id.max(existing.last_message_id);
                *existing = thread;
            }
            None => self.threads.push(thread),
        }
    }

    /// We joined (`Some`) or left a thread.
    pub fn set_thread_member(&mut self, thread: Snowflake, member: Option<ThreadMember>) {
        if let Some(thread) = self.threads.iter_mut().find(|c| c.id == thread) {
            thread.member = member;
        }
    }

    pub fn role(&self, id: Snowflake) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == id)
    }

    /// The highest of `roles` (`AT` in 7864): `@everyone` last, then by
    /// position, the older (smaller id) above on a tie.
    pub fn highest_role<'a>(&self, roles: impl IntoIterator<Item = &'a Role>) -> Option<&'a Role> {
        roles.into_iter().min_by_key(|r| (r.id == self.id, std::cmp::Reverse(r.position), r.id))
    }

    /// A role was created or changed.
    pub fn set_role(&mut self, role: Role) {
        match self.roles.iter_mut().find(|r| r.id == role.id) {
            Some(existing) => *existing = role,
            None => self.roles.push(role),
        }
    }

    /// Seconds left of our timeout here, if we are in one. Then the official
    /// client lets us read and nothing else, and says so where the message box
    /// would be. Administrators keep every permission and see no notice
    /// (`renderMessageBanner`); Discord doesn't let anyone time them out.
    pub fn timeout_left(&self, me: &Account, now_unix: i64) -> Option<i64> {
        let left = self.timed_out_until.map(|until| until - now_unix).filter(|&left| left > 0)?;
        let admin = permissions::in_guild(self, me, now_unix).contains(Permissions::ADMINISTRATOR);
        (!admin).then_some(left)
    }

    /// Text, announcement, forum, media, voice and stage channels we can view,
    /// in sidebar order: uncategorised first, then each category by position,
    /// channels by position within it, with voice and stage after the rest
    /// (`getShownChannelIds` in 791513 adds 10 000 to their position).
    /// Returns `(category, channels)` groups.
    pub fn sidebar(&self, me: &Account, now_unix: i64) -> Vec<(Option<&Channel>, Vec<&Channel>)> {
        let by_position =
            |a: &&Channel, b: &&Channel| (a.position.unwrap_or(0), a.id).cmp(&(b.position.unwrap_or(0), b.id));
        let place = |c: &&Channel| i64::from(c.position.unwrap_or(0)) + if c.kind.is_vocal() { 10_000 } else { 0 };
        let channels_in = |parent: Option<Snowflake>| {
            let mut v: Vec<&Channel> = self
                .channels
                .iter()
                .filter(|c| {
                    (c.kind.is_text() || c.kind.is_forum_like() || c.kind.is_vocal())
                        && c.parent_id == parent
                        && self.can_view(c, me, now_unix)
                })
                .collect();
            v.sort_by_key(|c| (place(c), c.id));
            v
        };

        let mut groups = vec![(None, channels_in(None))];
        let mut categories: Vec<&Channel> = self.channels.iter().filter(|c| c.kind == ChannelKind::Category).collect();
        categories.sort_by(by_position);
        for category in categories {
            groups.push((Some(category), channels_in(Some(category.id))));
        }
        groups.retain(|(_, channels)| !channels.is_empty());
        groups
    }

    /// Whether `channel` shows at all (see [`permissions::can_view`]).
    pub fn can_view(&self, channel: &Channel, me: &Account, now_unix: i64) -> bool {
        permissions::can_view(self, channel, me, now_unix)
    }
}

/// Guilds arrive in two shapes depending on the gateway capabilities: name and
/// icon either at the top level or inside `properties`. Unavailable guilds have
/// neither and are rejected here; they arrive later via GUILD_CREATE.
impl<'de> Deserialize<'de> for Guild {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Guild::from_value(&Value::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl Guild {
    /// A guild from its JSON as it is. Deserializing from a `&Value` would
    /// copy all of it first, which READY, with a guild for each server, can't
    /// afford.
    pub(crate) fn from_value(value: &Value) -> Result<Guild, serde_json::Error> {
        #[derive(Deserialize)]
        struct Properties {
            name: String,
            #[serde(default)]
            icon: Option<String>,
            #[serde(default)]
            default_message_notifications: Option<u8>,
            #[serde(default)]
            owner_id: Option<Snowflake>,
            #[serde(default)]
            mfa_level: Option<u8>,
        }
        #[derive(Deserialize)]
        struct Raw {
            id: Snowflake,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            icon: Option<String>,
            #[serde(default)]
            properties: Option<Properties>,
            #[serde(default)]
            channels: Vec<Channel>,
            #[serde(default)]
            default_message_notifications: Option<u8>,
            #[serde(default)]
            owner_id: Option<Snowflake>,
            #[serde(default)]
            mfa_level: Option<u8>,
        }

        let raw = Raw::deserialize(value)?;
        let (name, icon, default, owner_id, mfa_level) = match (raw.name, raw.properties) {
            (Some(name), _) => (name, raw.icon, raw.default_message_notifications, raw.owner_id, raw.mfa_level),
            (None, Some(p)) => (p.name, p.icon, p.default_message_notifications, p.owner_id, p.mfa_level),
            (None, None) => return Err(serde::de::Error::custom("guild has no name (unavailable)")),
        };
        let props = if value["properties"].is_object() { &value["properties"] } else { value };
        let features: Vec<String> =
            props["features"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        // Roles stay at the top level in both shapes. One we can't read is
        // left out, which only ever withholds what it grants.
        let roles = value["roles"].as_array().into_iter().flatten().filter_map(|r| Role::deserialize(r).ok()).collect();
        let threads = value["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| Channel::deserialize(t).ok())
            .filter(|t| t.kind.is_thread())
            .collect();
        Ok(Guild {
            id: raw.id,
            name,
            icon,
            channels: raw.channels,
            threads,
            default_message_notifications: default.unwrap_or(0),
            owner_id,
            roles,
            requires_mfa: mfa_level == Some(1),
            gradient_roles: features.iter().any(|f| f == "ENHANCED_ROLE_COLORS"),
            my_roles: Vec::new(),
            timed_out_until: None,
            verification: Rules::from_guild(value),
            joined_at: unix_rounded_up(&value["joined_at"]),
            pending: false,
            member_flags: 0,
            features,
            member_count: value["member_count"].as_u64(),
            hub_type: props["hub_type"].as_u64().and_then(|t| u8::try_from(t).ok()),
            nsfw_level: props["nsfw_level"].as_u64().and_then(|v| u8::try_from(v).ok()).unwrap_or(0),
            owner_configured_content_level: props["owner_configured_content_level"]
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(0),
            vanity_url_code: props["vanity_url_code"].as_str().map(String::from),
            premium_tier: props["premium_tier"].as_u64().and_then(|t| u8::try_from(t).ok()).unwrap_or(0),
            rules_channel_id: props["rules_channel_id"].as_str().and_then(|id| id.parse().ok()).map(Snowflake),
            emojis: GuildEmoji::list(raw.id, &value["emojis"]).unwrap_or_default(),
            stickers: Listed::guild_list(raw.id, &value["stickers"]).unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub url: String,
    /// The same file through the media proxy, which can resize it.
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub flags: u64,
    /// In bytes.
    #[serde(default)]
    pub size: Option<u64>,
    /// A voice message's length, in seconds.
    #[serde(default)]
    pub duration_secs: Option<f64>,
    /// A voice message's loudness over time: base64, a byte a sample.
    #[serde(default)]
    pub waveform: Option<String>,
}

impl Attachment {
    const IS_THUMBNAIL: u64 = 1 << 1;
    const IS_SPOILER: u64 = 1 << 3;

    /// Thumbnails of other attachments, which the official client hides.
    pub fn is_hidden(&self) -> bool {
        self.flags & Self::IS_THUMBNAIL != 0
    }

    /// Hidden until the reader reveals it. Older uploads mark it only in the
    /// file name.
    pub fn is_spoiler(&self) -> bool {
        self.flags & Self::IS_SPOILER != 0 || self.filename.starts_with("SPOILER_")
    }

    /// An animated image, by the official client's rule.
    pub fn is_animated(&self) -> bool {
        crate::embed::looks_animated(&self.url, self.flags)
    }

    /// Whether this attachment is an image we can show inline. Videos have
    /// dimensions too, so dimensions only decide when the type is unknown.
    pub fn is_image(&self) -> bool {
        match self.content_type.as_deref() {
            Some(t) => t.starts_with("image/"),
            None => self.width.is_some() && self.height.is_some(),
        }
    }

    /// A video to play inline. The official client goes by the extension,
    /// and wants dimensions.
    pub fn is_video(&self) -> bool {
        let sized = self.width.is_some_and(|w| w > 0) && self.height.is_some_and(|h| h > 0);
        sized && self.extension_in(&["mp4", "webm", "mov", "qt"])
    }

    /// Sound to play inline.
    pub fn is_audio(&self) -> bool {
        self.extension_in(&["mp3", "m4a", "ogg", "opus", "wav", "flac"])
    }

    fn extension_in(&self, extensions: &[&str]) -> bool {
        let extension = self.filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        extension.is_some_and(|e| extensions.contains(&e.as_str()))
    }

    /// A voice message's loudness, 0 to 1, as `bars` bars: samples averaged
    /// when there are more, and silence after them when there are fewer.
    pub fn waveform_bars(&self, bars: usize) -> Vec<f32> {
        let samples = self.waveform.as_deref().map(glib::base64_decode).unwrap_or_default();
        (0..bars)
            .map(|bar| {
                if samples.len() <= bars {
                    return samples.get(bar).map_or(0.0, |&s| s as f32 / 255.0);
                }
                let range = &samples[bar * samples.len() / bars..(bar + 1) * samples.len() / bars];
                range.iter().map(|&s| s as f32).sum::<f32>() / range.len().max(1) as f32 / 255.0
            })
            .collect()
    }
}

/// How many bars a voice message's waveform has: more for longer ones, as
/// the official client draws it on a 6-pixel pitch.
pub fn voice_bars(duration: f64) -> usize {
    let width = if duration <= 0.5 {
        40.0
    } else if duration >= 45.0 {
        294.0
    } else {
        (duration - 0.5) / 44.5 * 254.0 + 40.0
    };
    ((width + 4.0) / 6.0).floor() as usize
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    pub author: User,
    #[serde(default)]
    pub content: String,
    /// ISO 8601.
    pub timestamp: String,
    /// Set once the message has been edited.
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Users mentioned in `content`, resolved by Discord. Used to turn
    /// `<@id>` into a name without a separate user cache.
    #[serde(default)]
    pub mentions: Vec<User>,
    /// `@everyone` or `@here`.
    #[serde(default)]
    pub mention_everyone: bool,
    #[serde(default)]
    pub mention_roles: Vec<Snowflake>,
    #[serde(rename = "type", default)]
    pub kind: u8,
    /// What a reply (or forward, crosspost, pin notice) points at.
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    /// The message a reply answers, as Discord resolved it.
    #[serde(default, deserialize_with = "referenced")]
    pub referenced_message: Referenced,
    #[serde(default)]
    pub sticker_items: Vec<Sticker>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub flags: u64,
    #[serde(default)]
    pub poll: Option<crate::poll::Poll>,
    /// What people reacted with, an emoji at a time.
    #[serde(default, deserialize_with = "crate::reactions::list")]
    pub reactions: Vec<crate::reactions::Reaction>,
    /// What a forward carries.
    #[serde(default)]
    pub message_snapshots: Vec<Snapshot>,
    #[serde(default)]
    pub components: Vec<crate::component::Component>,
    /// The app a message with components belongs to, if it isn't its author.
    #[serde(default)]
    pub application_id: Option<Snowflake>,
    /// What the sender called it; an app's response to an interaction
    /// carries the interaction's.
    #[serde(default, deserialize_with = "nonce")]
    pub nonce: Option<String>,
    /// Ours: what is pending on its components, and what was picked in them.
    #[serde(skip)]
    pub local: crate::interaction::Local,
    /// For a call's system message: who joined it, and when it ended.
    #[serde(default)]
    pub call: Option<Call>,
    /// The author's member, which a live message in a server carries.
    #[serde(default)]
    pub member: Option<MessageMember>,
    /// For an app's answer to a command: who ran it, and how the app was
    /// installed.
    #[serde(default)]
    pub interaction_metadata: Option<InteractionMetadata>,
    /// For an app's answer: the command it answers, as the line above it
    /// names it. Discord calls it deprecated but sends it.
    #[serde(default)]
    pub interaction: Option<MessageInteraction>,
    /// Set when the message launched an activity; only whether it's there
    /// counts.
    #[serde(default)]
    pub activity_instance: Option<serde::de::IgnoredAny>,
    /// A Nitro trial offered to a friend, which can't be edited.
    #[serde(default)]
    pub referral_trial_offer_id: Option<Snowflake>,
    /// The thread this message started, which history sends along.
    #[serde(default, deserialize_with = "started_thread")]
    pub thread: Option<StartedThread>,
}

/// A thread a message started, as the message carries it.
#[derive(Clone, Debug)]
pub struct StartedThread(pub std::sync::Arc<Channel>);

/// Two are equal when they are the same object: a message only changes
/// when Discord sends it again.
impl PartialEq for StartedThread {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

fn started_thread<'de, D: Deserializer<'de>>(d: D) -> Result<Option<StartedThread>, D::Error> {
    let thread = Option::<Channel>::deserialize(d)?;
    Ok(thread.filter(|t| t.kind.is_thread()).map(|t| StartedThread(std::sync::Arc::new(t))))
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct InteractionMetadata {
    pub user: User,
    /// Keyed by how the app was installed: `"0"` to a server, `"1"` to a user.
    #[serde(default)]
    pub authorizing_integration_owners: std::collections::BTreeMap<String, Value>,
    /// Why an answer only we can see is hidden from the others, when it
    /// isn't just the app's choice; see [`crate::interaction::ephemerality_text`].
    #[serde(default)]
    pub ephemerality_reason: Option<u64>,
    /// The interaction's type: 2 a command, 3 a component, 5 a modal's
    /// submit (127258).
    #[serde(rename = "type", default)]
    pub kind: u8,
    /// Whom a user command was run on.
    #[serde(default)]
    pub target_user: Option<User>,
    /// The command's type (`ApplicationCommandType`), for a command.
    #[serde(default)]
    pub command_type: Option<u8>,
    /// For a modal's submit: the interaction that opened the modal.
    #[serde(default)]
    pub triggering_interaction_metadata: Option<Box<InteractionMetadata>>,
}

/// The command an app's answer answers (`interaction`, read by 432005).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MessageInteraction {
    #[serde(rename = "type", default)]
    pub kind: u8,
    /// The command's name with its subcommands, space-separated.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub name_localized: Option<String>,
    /// Who ran it.
    pub user: User,
}

impl MessageInteraction {
    /// What the client shows: `name_localized ?? name`, so an empty
    /// localized name stays empty.
    pub fn display_name(&self) -> &str {
        self.name_localized.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MessageMember {
    #[serde(default)]
    pub roles: Vec<Snowflake>,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
}

impl MessageMember {
    /// What shows of them (see [`crate::members::Known`]).
    pub fn known(&self) -> crate::members::Known {
        let text = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        crate::members::Known { roles: self.roles.clone(), nick: text(&self.nick), avatar: text(&self.avatar) }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub participants: Vec<Snowflake>,
    /// ISO 8601; `None` while the call goes on.
    #[serde(default)]
    pub ended_timestamp: Option<String>,
}

impl Message {
    const CROSSPOSTED: u64 = 1 << 0;
    const SUPPRESS_EMBEDS: u64 = 1 << 2;
    const HAS_THREAD: u64 = 1 << 5;
    const EPHEMERAL: u64 = 1 << 6;
    const LOADING: u64 = 1 << 7;
    /// How long an app has to follow up a deferred answer (`p` in 964363).
    const DEFERRED_FOR_MS: i64 = 15 * 60 * 1000;
    pub const IS_VOICE_MESSAGE: u64 = 1 << 13;
    const IS_COMPONENTS_V2: u64 = 1 << 15;

    /// Built from components alone: its attachments are what they show.
    pub fn is_components_v2(&self) -> bool {
        self.flags & Self::IS_COMPONENTS_V2 != 0
    }

    /// A thread has been started from it, with the message's id: the client
    /// looks it up as `castMessageIdAsChannelId`.
    pub fn has_thread(&self) -> bool {
        self.flags & Self::HAS_THREAD != 0
    }

    /// A recorded voice message: its audio attachment plays with a waveform.
    pub fn is_voice_message(&self) -> bool {
        self.flags & Self::IS_VOICE_MESSAGE != 0
    }

    /// The audio Save File saves from a voice message, or from the one this
    /// forwards: its first attachment, as the client takes it (761304).
    pub fn voice_audio(&self) -> Option<Attachment> {
        let forwarded = self.forwarded();
        let message = forwarded.as_ref().unwrap_or(self);
        message.is_voice_message().then(|| message.attachments.first().cloned()).flatten()
    }

    /// Published from an announcement channel to the servers following it,
    /// where editing or deleting it changes every copy.
    pub fn is_crossposted(&self) -> bool {
        self.flags & Self::CROSSPOSTED != 0
    }

    /// The author (or a moderator) removed the embeds.
    pub fn embeds_suppressed(&self) -> bool {
        self.flags & Self::SUPPRESS_EMBEDS != 0
    }

    /// Discord's message type for a reply.
    const REPLY: u8 = 19;

    /// The message's markup. Headings and lists count only in messages sent
    /// or edited since Discord added them, as in the official client.
    pub fn markup(&self) -> Vec<crate::markup::Span> {
        // The first message after the change, and the time it was sent.
        const SINCE_ID: Snowflake = Snowflake(1_088_216_706_570_268_682);
        const SINCE: &str = "2023-03-22T21:44:40";
        // An edit we saw live has no time, but is recent.
        let edited = self.edited_timestamp.as_deref().is_some_and(|t| t.is_empty() || t >= SINCE);
        crate::markup::parse_with(&self.content, self.id >= SINCE_ID || edited)
    }

    /// Whether the official client offers Reply on this message: ordinary
    /// messages and replies, commands, and a few system messages people
    /// answer (boosts, joins, …), but not, say, pins or calls. Never an
    /// ephemeral one, which only we can see. The client also asks that the
    /// message was sent, but only what Discord has accepted is ever listed.
    pub fn is_replyable(&self) -> bool {
        matches!(self.kind, 0 | 7 | 19 | 20 | 23 | 24 | 25 | 32..=42 | 45 | 46 | 69) && !self.is_ephemeral()
    }

    /// Only we can see it: an app's private answer.
    pub fn is_ephemeral(&self) -> bool {
        self.flags & Self::EPHEMERAL != 0
    }

    /// An app has taken a command and is yet to answer it.
    pub fn is_loading(&self) -> bool {
        self.flags & Self::LOADING != 0
    }

    /// A deferred answer the app never followed up, fifteen minutes on
    /// (`fK` in 964363).
    pub fn is_timed_out(&self, now_ms: i64) -> bool {
        self.is_loading() && self.id.unix_ms() + Self::DEFERRED_FOR_MS < now_ms
    }

    const FORWARD: u8 = 1;
    const THREAD_CREATED: u8 = 18;
    pub const THREAD_STARTER: u8 = 21;
    const AUTO_MODERATION_ACTION: u8 = 24;
    const MEDIA_MENTION: u8 = 68;
    /// Types nobody may delete (`UNDELETABLE` in 448761): calls, changes to
    /// a group DM, a thread's starter and a few other system messages.
    const UNDELETABLE: [u8; 11] = [1, 2, 3, 4, 5, 21, 35, 56, 57, 64, 68];

    /// Whether the official client offers `me` to edit this message (697470):
    /// our own text, not a forward, poll, voice message or Nitro trial
    /// offer, and not a sticker with nothing written. The client also asks
    /// that it was sent, but only what Discord has accepted is ever listed.
    /// Neucord leaves out what's built from components, which it couldn't
    /// send back as the client does.
    pub fn is_editable_by(&self, me: Snowflake) -> bool {
        self.author.id == me
            && self.is_user_message()
            && self.kind != Self::MEDIA_MENTION
            && (self.sticker_items.is_empty() || !self.content.is_empty())
            && !self.is_voice_message()
            && self.referral_trial_offer_id.is_none()
            && self.poll.is_none()
            && self.message_reference.as_ref().is_none_or(|r| r.kind != Self::FORWARD)
            && !self.is_components_v2()
    }

    /// Whether `me` may delete this message (483768), given whether we have
    /// Manage Messages where it is: our own, an app's answer to a command
    /// we ran with an app installed only to us (`canDeleteOwnMessage`), or
    /// anyone's with Manage Messages. AutoMod's notices take Manage
    /// Messages even when they're about us, and some types nobody deletes.
    pub fn is_deletable_by(&self, me: Snowflake, manage_messages: bool) -> bool {
        let user_installed = self
            .interaction_metadata
            .as_ref()
            .is_some_and(|i| i.user.id == me && i.authorizing_integration_owners.keys().eq(["1"]));
        let own = self.author.id == me || user_installed;
        (own || manage_messages)
            && (self.kind != Self::AUTO_MODERATION_ACTION || manage_messages)
            && !Self::UNDELETABLE.contains(&self.kind)
    }

    /// A reply that didn't ping the author of what it answers, as far as we
    /// know. Editing it keeps it so: an edit's mentions are worked out again.
    pub fn is_quiet_reply(&self) -> bool {
        match &self.referenced_message {
            Referenced::Message(original) if self.kind == Self::REPLY => {
                !self.mentions.iter().any(|u| u.id == original.author.id)
            }
            _ => false,
        }
    }

    /// What this message forwards, as a message of its own: dated by the
    /// original's id, in the place it was sent.
    pub fn forwarded(&self) -> Option<Message> {
        let reference = self.message_reference.as_ref().filter(|r| r.kind == Self::FORWARD)?;
        let snapshot = &self.message_snapshots.first()?.message;
        Some(Message {
            id: reference.message_id.unwrap_or(self.id),
            channel_id: reference.channel_id.unwrap_or(self.channel_id),
            guild_id: reference.guild_id,
            kind: snapshot.kind,
            content: snapshot.content.clone(),
            timestamp: snapshot.timestamp.clone(),
            edited_timestamp: snapshot.edited_timestamp.clone(),
            attachments: snapshot.attachments.clone(),
            embeds: snapshot.embeds.clone(),
            sticker_items: snapshot.sticker_items.clone(),
            components: snapshot.components.clone(),
            flags: snapshot.flags,
            mentions: Vec::new(),
            mention_everyone: false,
            mention_roles: Vec::new(),
            message_reference: None,
            referenced_message: Referenced::Unresolved,
            message_snapshots: Vec::new(),
            poll: None,
            reactions: Vec::new(),
            call: None,
            member: None,
            ..self.clone()
        })
    }

    /// What the official client's Copy Text copies (`getContentMessage` in
    /// 383233, then 761304): the markup as written, of the forwarded message
    /// for a forward, and of the texts for one built from components. `None`
    /// when that is empty.
    pub fn copy_text(&self) -> Option<String> {
        let forwarded = self.forwarded();
        let message = forwarded.as_ref().unwrap_or(self);
        let text = match message.is_components_v2() {
            true => crate::component::Component::texts(&message.components).join("\n"),
            false => message.content.clone(),
        };
        (!text.is_empty()).then_some(text)
    }

    /// The message's link in Discord's web app, which every client opens.
    /// `guild` is its channel's server, which messages often don't carry.
    pub fn link(&self, guild: Option<Snowflake>) -> String {
        let guild = guild.map_or("@me".to_owned(), |g| g.to_string());
        format!("https://discord.com/channels/{guild}/{}/{}", self.channel_id, self.id)
    }

    /// The message a thread was started from, which the thread's first
    /// message stands for (a `THREAD_STARTER_MESSAGE`), when Discord sent
    /// it along. The client shows it in its place (`nZ` in 567008).
    pub fn thread_starter(&self) -> Option<&Message> {
        match &self.referenced_message {
            Referenced::Message(original) if self.kind == Self::THREAD_STARTER => Some(original),
            _ => None,
        }
    }

    /// The thread a "started a thread" message (`THREAD_CREATED`) is about:
    /// the channel it references.
    pub fn created_thread(&self) -> Option<Snowflake> {
        if self.kind != Self::THREAD_CREATED {
            return None;
        }
        self.message_reference.as_ref()?.channel_id
    }

    /// Whether the client counts this message as blocked
    /// (`isBlockedForMessage`, 994500), `blocked` saying whom we blocked:
    /// its author, or for an app's answer who ran the command.
    pub fn is_blocked_by(&self, blocked: impl Fn(Snowflake) -> bool) -> bool {
        blocked(self.author.id) || self.interaction_metadata.as_ref().is_some_and(|i| blocked(i.user.id))
    }

    /// What a reply line shows for this message when it has no text to
    /// show, in the client's wording and order (448368).
    pub fn reply_placeholder(&self) -> &'static str {
        if !self.sticker_items.is_empty() {
            "Click to see sticker"
        } else if self.interaction.is_some() {
            "Click to see command"
        } else {
            "Click to see attachment"
        }
    }

    /// The id of the message this one replies to.
    pub fn reply_to(&self) -> Option<Snowflake> {
        if self.kind != Self::REPLY {
            return None;
        }
        self.message_reference.as_ref()?.message_id
    }

    /// Consecutive messages by one author within this window share a header.
    const GROUP_WINDOW_SECS: i64 = 7 * 60;

    /// Discord's message types for an app's answer to a slash command and
    /// to a user or message command.
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    pub const CONTEXT_MENU_COMMAND: u8 = 23;

    /// An app's answer to a command (`isCommandType`, 383233).
    pub fn is_command(&self) -> bool {
        matches!(self.kind, Self::CHAT_INPUT_COMMAND | Self::CONTEXT_MENU_COMMAND)
    }

    /// Whether this message shares `previous`'s header (`u` in 857069): a
    /// user message by the same author, under seven minutes before it and
    /// on the same local day.
    pub fn groups_under(&self, previous: Option<&Message>) -> bool {
        let Some(previous) = previous.filter(|p| p.is_user_message()) else { return false };
        // A reply always shows what it answers, and a command's answer who
        // ran it, under a full header.
        if previous.author.id != self.author.id || self.reply_to().is_some() || self.is_command() {
            return false;
        }
        let local = |timestamp: &str| glib::DateTime::from_iso8601(timestamp, None).ok()?.to_local().ok();
        match (local(&previous.timestamp), local(&self.timestamp)) {
            (Some(a), Some(b)) => b.difference(&a).as_seconds() < Self::GROUP_WINDOW_SECS && a.ymd() == b.ymd(),
            _ => false,
        }
    }
}

/// A custom emoji has an id; a Unicode one only its name, the emoji itself.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<Snowflake>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Sticker {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
}

impl Sticker {
    /// The official client's size for a sticker in a message.
    pub const SIZE: u32 = 160;

    /// Where the sticker's image is, at `size` pixels, and whether it is
    /// animated. `None` for Lottie stickers, which are vector animations.
    pub fn image(&self, size: u32) -> Option<(String, bool)> {
        let (ext, animated) = match self.format_type {
            1 => ("png", false),
            // An APNG comes animated unless asked for a still.
            2 => ("png", true),
            4 => ("gif", true),
            _ => return None,
        };
        Some((format!("https://media.discordapp.net/stickers/{}.{ext}?size={size}", self.id), animated))
    }

    /// The picker's image, which the client asks for as a preview (`zg` in
    /// 378058): a still, so an animated GIF sticker comes as its PNG and an
    /// APNG is asked not to animate, which every client but Android's does.
    /// `None` for Lottie.
    pub fn still(&self, size: u32) -> Option<String> {
        let (url, animated) = self.image(size)?;
        Some(match self.format_type {
            2 => format!("{url}&passthrough=false"),
            _ if animated => url.replace(".gif?", ".png?"),
            _ => url,
        })
    }

    /// Where the sticker file is, as Copy Sticker Image Link gives it
    /// (`zg` in 378058): the image at the size the client shows messages
    /// at, or a Lottie sticker's JSON. `None` for a format the client
    /// doesn't know.
    pub fn link(&self) -> Option<String> {
        match self.format_type {
            3 => Some(format!("https://cdn.discordapp.com/stickers/{}.json", self.id)),
            _ => self.image(Self::SIZE).map(|(url, _)| url),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MessageReference {
    /// 0 for a reply (or pin notice, …), 1 for a forward.
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub message_id: Option<Snowflake>,
    #[serde(default)]
    pub channel_id: Option<Snowflake>,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
}

/// A copy of a forwarded message, as it was when forwarded.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Snapshot {
    pub message: SnapshotMessage,
}

/// What a forward copies of the original: not its author, nor whom it
/// mentions.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SnapshotMessage {
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub sticker_items: Vec<Sticker>,
    #[serde(default)]
    pub components: Vec<crate::component::Component>,
    #[serde(default)]
    pub flags: u64,
}

/// `referenced_message` has three states that matter: absent when Discord
/// did not resolve it, null when the original was deleted.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Referenced {
    #[default]
    Unresolved,
    Deleted,
    Message(Box<Message>),
}

/// A nonce is a string, though some senders write a number.
fn nonce<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn referenced<'de, D: Deserializer<'de>>(d: D) -> Result<Referenced, D::Error> {
    Ok(match Option::<Box<Message>>::deserialize(d)? {
        Some(message) => Referenced::Message(message),
        None => Referenced::Deleted,
    })
}

/// A MESSAGE_UPDATE. Only the id and channel are always there: a text edit
/// carries the content, while link previews arrive after the message as an
/// update with only `embeds`.
#[derive(Clone, Debug, Default)]
pub struct MessageEdit {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub content: Option<String>,
    pub edited: bool,
    pub embeds: Option<Vec<Embed>>,
    pub flags: Option<u64>,
    /// A poll that changed, such as one ended early.
    pub poll: Option<crate::poll::Poll>,
    /// An app's response replacing the components.
    pub components: Option<Vec<crate::component::Component>>,
}

impl MessageEdit {
    pub fn apply(self, message: &mut Message) {
        if let Some(content) = self.content {
            message.content = content;
        }
        if self.edited && message.edited_timestamp.is_none() {
            message.edited_timestamp = Some(String::new());
        }
        if let Some(embeds) = self.embeds {
            message.embeds = embeds;
        }
        if let Some(flags) = self.flags {
            message.flags = flags;
        }
        // When it ends and whether it has; the counts stay as the votes we
        // saw made them, since the client builds them from those.
        if let (Some(update), Some(poll)) = (self.poll, &mut message.poll) {
            poll.expiry = update.expiry;
            if update.results.is_some_and(|r| r.is_finalized) {
                poll.results.get_or_insert_with(Default::default).is_finalized = true;
            }
        }
        if let Some(components) = self.components {
            message.components = components;
        }
        // The menus show what the message says again (`MESSAGE_UPDATE` in 935548).
        message.local.forget_picks();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_channel_kind_gives_back_its_number() {
        for raw in 0..=u8::MAX {
            assert_eq!(ChannelKind::from_raw(raw).raw(), raw);
        }
    }

    /// One of ours (5), changed by `extra`.
    fn ours(extra: Value) -> Message {
        let mut message = serde_json::json!({"id": "3", "channel_id": "9", "type": 0, "content": "hi",
                                             "author": {"id": "5", "username": "me"}, "timestamp": ""});
        message.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(message).unwrap()
    }

    #[test]
    fn accounts_that_are_no_ones() {
        let user = |json: Value| serde_json::from_value::<User>(json).unwrap();
        let base = serde_json::json!({"id": "1", "username": "u"});
        let with = |extra: Value| {
            let mut json = base.clone();
            json.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            user(json)
        };
        assert!(!user(base.clone()).non_user_bot());
        // A webhook's author is a bot with the discriminator 0000; a bot of
        // today has 0, which a profile belongs to.
        assert!(with(serde_json::json!({"bot": true, "discriminator": "0000"})).non_user_bot());
        assert!(!with(serde_json::json!({"bot": true, "discriminator": "0"})).non_user_bot());
        assert!(!with(serde_json::json!({"discriminator": "0000"})).non_user_bot());
        assert!(with(serde_json::json!({"system": true})).non_user_bot());
    }

    #[test]
    fn tags_show_a_discriminator_only_when_it_counts() {
        let user = |json: Value| serde_json::from_value::<User>(json).unwrap();
        assert_eq!(user(serde_json::json!({"id": "1", "username": "pat", "discriminator": "1234"})).tag(), "pat#1234");
        assert_eq!(user(serde_json::json!({"id": "1", "username": "pat", "discriminator": "0"})).tag(), "pat");
        assert_eq!(user(serde_json::json!({"id": "1", "username": "hook", "discriminator": "0000"})).tag(), "hook");
        assert_eq!(user(serde_json::json!({"id": "1", "username": "pat"})).tag(), "pat");
        assert_eq!(user(serde_json::json!({"id": "1", "username": "", "discriminator": "1234"})).tag(), "???");
    }

    #[test]
    fn a_message_says_how_its_author_shows_in_the_server() {
        let message = ours(serde_json::json!({"member": {"roles": ["4"], "nick": "Nick", "avatar": ""}}));
        let known = message.member.unwrap().known();
        assert_eq!((known.roles, known.nick.as_deref(), known.avatar), (vec![Snowflake(4)], Some("Nick"), None));
    }

    #[test]
    fn a_linked_role_is_told_by_its_tags() {
        let role = |tags: Value| serde_json::from_value::<Role>(serde_json::json!({"id": "3", "tags": tags})).unwrap();
        assert!(role(serde_json::json!({"guild_connections": null})).linked);
        assert!(!role(serde_json::json!({"bot_id": "9"})).linked);
        assert!(!role(Value::Null).linked);
        assert!(!serde_json::from_value::<Role>(serde_json::json!({"id": "3"})).unwrap().linked);
    }

    #[test]
    fn an_app_that_has_not_answered_has_fifteen_minutes() {
        use serde_json::json;
        let loading = ours(json!({"type": 20, "flags": 1 << 7}));
        let sent = loading.id.unix_ms();
        assert!(loading.is_loading() && !loading.is_ephemeral());
        assert!(!loading.is_timed_out(sent + 14 * 60 * 1000));
        assert!(loading.is_timed_out(sent + 16 * 60 * 1000));
        assert!(!ours(json!({"type": 20})).is_timed_out(sent + 16 * 60 * 1000), "answered");
    }

    #[test]
    fn what_we_can_edit() {
        use serde_json::json;
        let me = Snowflake(5);
        assert!(ours(json!({})).is_editable_by(me));
        assert!(ours(json!({"type": 19})).is_editable_by(me), "a reply");
        assert!(!ours(json!({})).is_editable_by(Snowflake(6)), "someone else's");
        assert!(!ours(json!({"type": 7})).is_editable_by(me), "a system message");
        assert!(!ours(json!({"type": 68})).is_editable_by(me), "a media mention");
        let sticker = json!([{"id": "8", "name": "s"}]);
        assert!(!ours(json!({"content": "", "sticker_items": sticker})).is_editable_by(me), "a bare sticker");
        assert!(ours(json!({"sticker_items": sticker})).is_editable_by(me), "a sticker with text");
        assert!(!ours(json!({"flags": 1 << 13})).is_editable_by(me), "a voice message");
        assert!(!ours(json!({"flags": 1 << 15})).is_editable_by(me), "built from components");
        assert!(!ours(json!({"referral_trial_offer_id": "4"})).is_editable_by(me));
        assert!(!ours(json!({"poll": {"question": {"text": "?"}, "answers": []}})).is_editable_by(me));
        let forward = json!({"type": 1, "message_id": "1"});
        assert!(!ours(json!({"message_reference": forward})).is_editable_by(me));
    }

    #[test]
    fn what_we_can_delete() {
        use serde_json::json;
        let (me, them) = (Snowflake(5), Snowflake(6));
        assert!(ours(json!({})).is_deletable_by(me, false));
        assert!(!ours(json!({})).is_deletable_by(them, false) && ours(json!({})).is_deletable_by(them, true));
        assert!(!ours(json!({"type": 3})).is_deletable_by(me, true), "a call, by anyone");
        assert!(ours(json!({"type": 7})).is_deletable_by(them, true), "a join, by moderators");
        assert!(!ours(json!({"type": 21})).is_deletable_by(me, true), "a thread's starter");
        let automod = ours(json!({"type": 24}));
        assert!(!automod.is_deletable_by(me, false) && automod.is_deletable_by(me, true));

        // An app's answer to our command, deletable if the app is only ours.
        let answer = |owners: Value| {
            let metadata = json!({"user": {"id": "5", "username": "me"}, "authorizing_integration_owners": owners});
            ours(json!({"type": 20, "author": {"id": "7", "username": "app"}, "interaction_metadata": metadata}))
        };
        assert!(answer(json!({"1": "5"})).is_deletable_by(me, false));
        assert!(!answer(json!({"1": "5"})).is_deletable_by(them, false), "someone else's command");
        assert!(!answer(json!({"0": "1"})).is_deletable_by(me, false), "installed to the server");
        assert!(!answer(json!({"0": "1", "1": "5"})).is_deletable_by(me, false), "to both");
    }

    #[test]
    fn a_command_answer_starts_its_own_group() {
        let at = |id: &str, kind: u8, time: &str| {
            ours(json!({"id": id, "type": kind, "author": {"id": "7", "username": "app"}, "timestamp": time}))
        };
        // Noon UTC is the same local day anywhere.
        let first = at("3", 0, "2024-05-01T12:00:00+00:00");
        assert!(at("4", 0, "2024-05-01T12:01:00+00:00").groups_under(Some(&first)));
        for kind in [20, 23] {
            assert!(!at("4", kind, "2024-05-01T12:01:00+00:00").groups_under(Some(&first)), "type {kind}");
        }
    }

    #[test]
    fn a_reply_to_an_app_answer_asks_to_see_the_command() {
        let interaction = json!({"id": "8", "type": 2, "name": "roll", "user": {"id": "5", "username": "me"}});
        let embed = json!([{"title": "a roll"}]);
        let answer = ours(json!({"type": 20, "content": "", "embeds": embed, "interaction": interaction}));
        assert_eq!(answer.reply_placeholder(), "Click to see command");
        let sticker = json!([{"id": "4", "name": "wave", "format_type": 1}]);
        let with_sticker =
            ours(json!({"type": 20, "content": "", "interaction": interaction, "sticker_items": sticker}));
        assert_eq!(with_sticker.reply_placeholder(), "Click to see sticker", "a sticker comes first");
        assert_eq!(ours(json!({"content": "", "embeds": embed})).reply_placeholder(), "Click to see attachment");
    }

    #[test]
    fn an_answer_to_a_blocked_users_command_counts_as_blocked() {
        let blocked = |id: Snowflake| id == Snowflake(6);
        let metadata = |user: &str| json!({"id": "8", "type": 2, "user": {"id": user, "username": "someone"}});
        let answer = |user: &str| {
            ours(json!({"type": 20, "author": {"id": "7", "username": "app"}, "interaction_metadata": metadata(user)}))
        };
        assert!(answer("6").is_blocked_by(blocked), "a bot's answer to a blocked user's command");
        assert!(!answer("5").is_blocked_by(blocked));
        assert!(ours(json!({"author": {"id": "6", "username": "them"}})).is_blocked_by(blocked));
        assert!(!ours(json!({})).is_blocked_by(blocked));
    }

    #[test]
    fn an_app_answer_reads_the_command_it_answers() {
        let me = json!({"id": "5", "username": "me"});
        let interaction = json!({"id": "8", "type": 2, "name": "settings set", "user": me});
        let metadata = json!({"id": "8", "type": 5, "user": me, "triggering_interaction_metadata":
            {"id": "9", "type": 2, "user": me, "command_type": 2, "target_user": {"id": "6", "username": "them"}}});
        let answer = ours(json!({"type": 20, "interaction": interaction, "interaction_metadata": metadata,
                                 "activity_instance": {"id": "a1"}}));
        let ran = answer.interaction.as_ref().unwrap();
        assert_eq!((ran.kind, ran.display_name(), ran.user.id), (2, "settings set", Snowflake(5)));
        let metadata = answer.interaction_metadata.as_ref().unwrap();
        let triggering = metadata.triggering_interaction_metadata.as_deref().unwrap();
        assert_eq!((metadata.kind, metadata.command_type, metadata.target_user.as_ref()), (5, None, None));
        assert_eq!((triggering.kind, triggering.command_type), (2, Some(2)));
        assert_eq!(triggering.target_user.as_ref().map(|u| u.id), Some(Snowflake(6)));
        assert!(answer.activity_instance.is_some());

        let localized = json!({"name": "roll", "name_localized": "würfeln", "user": me});
        assert_eq!(ours(json!({"interaction": localized})).interaction.unwrap().display_name(), "würfeln");
        let emptied = json!({"name": "roll", "name_localized": "", "user": me});
        assert_eq!(ours(json!({"interaction": emptied})).interaction.unwrap().display_name(), "", "?? keeps it");
        let plain = ours(json!({}));
        assert!(plain.interaction.is_none() && plain.activity_instance.is_none());
    }

    #[test]
    fn slowmode_off() {
        let channel = |rate: &str| -> Channel {
            serde_json::from_str(&format!(r#"{{"id":"1","type":0,"rate_limit_per_user":{rate}}}"#)).unwrap()
        };
        assert_eq!(channel("null").rate_limit_per_user, 0);
        assert_eq!(channel("0").rate_limit_per_user, 0);
        assert_eq!(channel("21600").rate_limit_per_user, 21600);
    }

    #[test]
    fn guild_with_top_level_name() {
        let g: Guild = serde_json::from_str(
            r#"{"id":"1","name":"Legacy","icon":null,"owner_id":"7","channels":[
                {"id":"10","type":4,"name":"Cat","position":0},
                {"id":"11","type":0,"name":"b","position":1,"parent_id":"10"},
                {"id":"12","type":0,"name":"a","position":0,"parent_id":"10"},
                {"id":"13","type":2,"name":"voice","position":2,"parent_id":"10"},
                {"id":"14","type":0,"name":"top","position":5},
                {"id":"15","type":0,"name":"staff","position":6,
                 "permission_overwrites":[{"id":"1","type":0,"allow":"0","deny":"1024"}]}
            ]}"#,
        )
        .unwrap();
        assert_eq!((g.name.as_str(), g.owner_id), ("Legacy", Some(Snowflake(7))));
        let sidebar = g.sidebar(&Account { id: Snowflake(5), ..Default::default() }, 0);
        let names: Vec<(Option<&str>, Vec<&str>)> = sidebar
            .iter()
            .map(|(cat, chans)| {
                (cat.and_then(|c| c.name.as_deref()), chans.iter().map(|c| c.name.as_deref().unwrap()).collect())
            })
            .collect();
        assert_eq!(names, vec![(None, vec!["top"]), (Some("Cat"), vec!["a", "b", "voice"])], "staff is hidden from us");
        let owner = g.sidebar(&Account { id: Snowflake(7), ..Default::default() }, 0);
        assert_eq!(owner[0].1.len(), 2, "but not from the owner");
    }

    #[test]
    fn a_channels_link() {
        let channel: Channel = serde_json::from_str(r#"{"id":"11","type":2}"#).unwrap();
        assert_eq!(channel.link(Some(Snowflake(3))), "https://discord.com/channels/3/11");
        assert_eq!(channel.link(None), "https://discord.com/channels/@me/11");
    }

    #[test]
    fn voice_and_stage_channels_list_after_the_rest_where_we_can_connect() {
        // 2 voice and 13 stage: the sidebar sorts them as 10 000 further down, whatever their position.
        let g: Guild = serde_json::from_str(
            r#"{"id":"1","name":"G","owner_id":"7","roles":[{"id":"1","name":"@everyone","permissions":"1049600"}],"channels":[
                {"id":"10","type":4,"name":"Cat","position":0},
                {"id":"11","type":2,"name":"lounge","position":0,"parent_id":"10"},
                {"id":"12","type":0,"name":"chat","position":5,"parent_id":"10"},
                {"id":"13","type":13,"name":"stage","position":1,"parent_id":"10"},
                {"id":"14","type":15,"name":"forum","position":9,"parent_id":"10"},
                {"id":"15","type":2,"name":"hall","position":0},
                {"id":"16","type":0,"name":"rules","position":3},
                {"id":"17","type":2,"name":"no-connect","position":0,"parent_id":"10",
                 "permission_overwrites":[{"id":"1","type":0,"allow":"0","deny":"1048576"}]},
                {"id":"18","type":13,"name":"hidden-stage","position":0,"parent_id":"10",
                 "permission_overwrites":[{"id":"1","type":0,"allow":"0","deny":"1024"}]},
                {"id":"19","type":0,"name":"late","position":10002,"parent_id":"10"}
            ]}"#,
        )
        .unwrap();
        let sidebar = g.sidebar(&Account { id: Snowflake(5), ..Default::default() }, 0);
        let names: Vec<(Option<&str>, Vec<&str>)> = sidebar
            .iter()
            .map(|(cat, chans)| {
                (cat.and_then(|c| c.name.as_deref()), chans.iter().map(|c| c.name.as_deref().unwrap()).collect())
            })
            .collect();
        assert_eq!(
            names,
            vec![(None, vec!["rules", "hall"]), (Some("Cat"), vec!["chat", "forum", "lounge", "stage", "late"])],
            "Connect and View Channel both decide, and a text channel past 10 000 sorts after them"
        );
        assert!(ChannelKind::Voice.is_vocal() && ChannelKind::Stage.is_vocal() && !ChannelKind::Text.is_vocal());
    }

    #[test]
    fn roles_and_settings() {
        let mut g: Guild = serde_json::from_str(
            r#"{"id":"1","properties":{"name":"G","owner_id":"7","mfa_level":1},"roles":[
                {"id":"1","name":"@everyone","permissions":"1024"},
                {"id":"40","name":"Bot","permissions":8,"managed":true},
                {"id":"41","permissions":{}}
            ]}"#,
        )
        .unwrap();
        assert!(g.requires_mfa);
        let roles: Vec<(u64, &str, u64, bool)> =
            g.roles.iter().map(|r| (r.id.0, r.name.as_str(), r.permissions.0, r.managed)).collect();
        assert_eq!(roles, [(1, "@everyone", 1024, false), (40, "Bot", 8, true)], "an unreadable role is left out");

        g.my_roles = vec![Snowflake(40)];
        let update: Guild = serde_json::from_str(
            r#"{"id":"1","name":"Renamed","owner_id":"8","mfa_level":0,"roles":[{"id":"1","permissions":"0"}]}"#,
        )
        .unwrap();
        g.update(update);
        assert_eq!(
            (g.name.as_str(), g.owner_id, g.requires_mfa, g.roles.len()),
            ("Renamed", Some(Snowflake(8)), false, 1)
        );
        assert_eq!(g.my_roles, [Snowflake(40)], "our member stays");
    }

    #[test]
    fn administrators_see_no_timeout() {
        let mut g: Guild = serde_json::from_str(
            r#"{"id":"1","name":"G","roles":[{"id":"1","permissions":"3072"},{"id":"40","permissions":"8"}]}"#,
        )
        .unwrap();
        let me = Account { id: Snowflake(5), ..Default::default() };
        g.timed_out_until = Some(100);
        assert_eq!(g.timeout_left(&me, 40), Some(60));
        g.my_roles = vec![Snowflake(40)];
        assert_eq!(g.timeout_left(&me, 40), None);
        g.requires_mfa = true;
        assert_eq!(g.timeout_left(&me, 40), Some(60), "Administrator needs 2FA here, so it doesn't count");
    }

    #[test]
    fn guild_with_properties() {
        let g: Guild =
            serde_json::from_str(r#"{"id":"2","properties":{"name":"New","icon":"abc","owner_id":"7"},"channels":[]}"#)
                .unwrap();
        assert_eq!((g.name.as_str(), g.icon.as_deref(), g.owner_id), ("New", Some("abc"), Some(Snowflake(7))));
    }

    #[test]
    fn content_levels_in_either_guild_shape() {
        let nested: Guild = serde_json::from_str(
            r#"{"id":"2","properties":{"name":"N","nsfw_level":3,"owner_configured_content_level":1},"channels":[]}"#,
        )
        .unwrap();
        let flat: Guild =
            serde_json::from_str(r#"{"id":"2","name":"N","nsfw_level":3,"owner_configured_content_level":1}"#).unwrap();
        for g in [&nested, &flat] {
            assert_eq!((g.nsfw_level, g.owner_configured_content_level), (3, 1));
        }
        let none: Guild =
            serde_json::from_str(r#"{"id":"2","name":"N","owner_configured_content_level":null}"#).unwrap();
        assert_eq!((none.nsfw_level, none.owner_configured_content_level), (0, 0), "DEFAULT");
        let mut kept = nested;
        kept.update(none);
        assert_eq!((kept.nsfw_level, kept.owner_configured_content_level), (0, 0), "GUILD_UPDATE replaces them");
    }

    #[test]
    fn a_server_is_age_restricted_when_explicit_or_age_restricted() {
        let guild = |level: u8, owner: u8| {
            let mut g: Guild = serde_json::from_str(r#"{"id":"2","name":"N"}"#).unwrap();
            (g.nsfw_level, g.owner_configured_content_level) = (level, owner);
            g
        };
        for (level, restricted) in [(0, false), (1, true), (2, false), (3, true)] {
            assert_eq!(guild(level, 0).age_restricted(true), restricted, "nsfw_level {level}");
            assert_eq!(guild(0, level).age_restricted(false), restricted, "owner's level {level}");
        }
        assert!(!guild(3, 0).age_restricted(false), "outside the experiment the owner's level decides");
        assert!(!guild(0, 3).age_restricted(true), "inside it Discord's");
    }

    #[test]
    fn rules_channel_in_either_guild_shape() {
        let nested: Guild =
            serde_json::from_str(r#"{"id":"2","properties":{"name":"N","rules_channel_id":"30"},"channels":[]}"#)
                .unwrap();
        let flat: Guild =
            serde_json::from_str(r#"{"id":"2","name":"N","rules_channel_id":"30","channels":[]}"#).unwrap();
        let none: Guild =
            serde_json::from_str(r#"{"id":"2","name":"N","rules_channel_id":null,"channels":[]}"#).unwrap();
        assert_eq!((nested.rules_channel_id, flat.rules_channel_id), (Some(Snowflake(30)), Some(Snowflake(30))));
        assert_eq!(none.rules_channel_id, None);
        let mut kept = nested;
        kept.update(none);
        assert_eq!(kept.rules_channel_id, None, "GUILD_UPDATE clears it too");
    }

    #[test]
    fn a_message_knows_its_thread() {
        let message = |extra: &str| -> Message {
            serde_json::from_str(&format!(
                r#"{{"id":"20","channel_id":"9","author":{{"id":"6","username":"bob"}},"content":"hi",
                    "timestamp":"2026-10-26T12:00:00.000000+00:00"{extra}}}"#
            ))
            .unwrap()
        };
        let started = message(
            r#","flags":32,"thread":{"id":"20","type":11,"parent_id":"9","name":"plans","message_count":4,
                "thread_metadata":{"archived":true,"auto_archive_duration":60}}"#,
        );
        assert!(started.has_thread());
        let thread = started.thread.as_ref().expect("the thread comes along");
        assert_eq!((thread.0.id, thread.0.message_count, thread.0.is_archived()), (Snowflake(20), Some(4), true));
        assert!(!message("").has_thread() && message("").thread.is_none());
        let not_a_thread = message(r#","thread":{"id":"20","type":0}"#);
        assert!(not_a_thread.thread.is_none(), "only a thread counts");
        let created = message(r#","type":18,"message_reference":{"channel_id":"20"}"#);
        assert_eq!(created.created_thread(), Some(Snowflake(20)));
        assert_eq!(message(r#","message_reference":{"channel_id":"20"}"#).created_thread(), None);
    }

    #[test]
    fn guild_threads() {
        let mut g: Guild = serde_json::from_str(
            r#"{"id":"1","name":"G","channels":[{"id":"9","type":0},{"id":"15","type":15}],"threads":[
                {"id":"20","type":11,"parent_id":"9","last_message_id":"30",
                 "thread_metadata":{"archived":false,"locked":false,"auto_archive_duration":60},
                 "member":{"flags":0,"muted":false,"mute_config":null,"join_timestamp":"2026-10-28T00:00:00.250000+00:00"}},
                {"id":"9","type":0}]}"#,
        )
        .unwrap();
        assert_eq!(g.threads.len(), 1, "only threads");
        assert_eq!(g.find(Snowflake(20)).and_then(|t| t.member.as_ref()?.join_timestamp), Some(1_793_145_600_250));
        assert!(g.thread(Snowflake(9)).is_none() && g.find(Snowflake(9)).is_some());
        assert!(g.channel(Snowflake(15)).is_some_and(|c| c.kind.is_forum_like() && !c.kind.is_thread()));

        let update: Channel = serde_json::from_str(
            r#"{"id":"20","type":11,"parent_id":"9","last_message_id":"25",
                "thread_metadata":{"archived":true,"locked":true}}"#,
        )
        .unwrap();
        g.set_thread(update);
        let t = g.thread(Snowflake(20)).unwrap();
        assert!(t.is_archived() && t.is_locked() && !t.is_active_thread());
        assert!(t.member.is_some(), "an update leaves our membership out");
        assert_eq!(t.last_message_id, Some(Snowflake(30)), "and may not know of newer messages");
        g.set_thread_member(Snowflake(20), None);
        assert!(g.thread(Snowflake(20)).unwrap().member.is_none(), "we left");
    }

    #[test]
    fn unavailable_guild_is_rejected() {
        assert!(serde_json::from_str::<Guild>(r#"{"id":"3","unavailable":true}"#).is_err());
    }

    #[test]
    fn dm_titles() {
        let dm: Channel = serde_json::from_str(
            r#"{"id":"20","type":1,"last_message_id":"30",
                "recipients":[{"id":"5","username":"alice","global_name":"Alice"}]}"#,
        )
        .unwrap();
        assert_eq!((dm.kind, dm.title(), dm.activity()), (ChannelKind::Dm, "Alice".into(), Snowflake(30)));
        assert!(dm.kind.is_private() && dm.kind.is_text());

        let group: Channel = serde_json::from_str(
            r#"{"id":"21","type":3,"name":null,"recipients":[
                {"id":"5","username":"a"},{"id":"6","username":"b"}]}"#,
        )
        .unwrap();
        assert_eq!((group.title(), group.activity()), ("a, b".into(), Snowflake(21)));

        let named: Channel = serde_json::from_str(r#"{"id":"22","type":3,"name":"crew","recipients":[]}"#).unwrap();
        assert_eq!(named.title(), "crew");
        assert!(!named.is_request());
    }

    #[test]
    fn message_request_flags() {
        let dm =
            |flags: &str| -> Channel { serde_json::from_str(&format!(r#"{{"id":"1","type":1{flags}}}"#)).unwrap() };
        assert!(dm(r#","is_message_request":true,"is_spam":false"#).is_request());
        assert!(dm(r#","is_message_request":false,"is_spam":true"#).is_request());
        assert!(!dm(r#","is_message_request":null,"is_spam":null"#).is_request());
    }

    #[test]
    fn videos_are_not_images() {
        let attachment = |extra: &str| -> Attachment {
            serde_json::from_str(&format!(r#"{{"id":"1","filename":"f","url":"u"{extra}}}"#)).unwrap()
        };
        assert!(!attachment(r#","content_type":"video/quicktime","width":640,"height":480"#).is_image());
        assert!(attachment(r#","content_type":"image/png","width":640,"height":480"#).is_image());
        assert!(attachment(r#","width":640,"height":480"#).is_image(), "no type: dimensions decide");
        assert!(!attachment("").is_image());
        assert!(attachment(r#","flags":8"#).is_spoiler());
        assert!(!attachment("").is_spoiler());
        assert!(attachment(r#","flags":2"#).is_hidden());
        assert!(!attachment(r#","flags":32"#).is_animated(), "the flag counts for WebP and AVIF only");
    }

    #[test]
    fn attachment_kinds() {
        let attachment = |name: &str, extra: &str| -> Attachment {
            serde_json::from_str(&format!(r#"{{"id":"1","filename":"{name}","url":"u"{extra}}}"#)).unwrap()
        };
        assert!(attachment("clip.MP4", r#","width":640,"height":480"#).is_video());
        assert!(!attachment("clip.mp4", "").is_video(), "no dimensions");
        assert!(attachment("song.flac", "").is_audio());
        assert!(!attachment("notes.txt", "").is_audio());
    }

    #[test]
    fn voice_waveforms() {
        assert_eq!((voice_bars(0.2), voice_bars(10.0), voice_bars(60.0)), (7, 16, 49));
        // Four samples: 0, 255, 51, 102.
        let voice: Attachment =
            serde_json::from_str(r#"{"id":"1","filename":"voice-message.ogg","url":"u","waveform":"AP8zZg=="}"#)
                .unwrap();
        assert_eq!(voice.waveform_bars(2), [0.5, 0.3]);
        assert_eq!(voice.waveform_bars(5), [0.0, 1.0, 0.2, 0.4, 0.0]);
    }

    #[test]
    fn snowflake_times() {
        assert_eq!(Snowflake(1_088_216_706_570_268_682).unix_ms(), 1_679_521_480_935);
    }

    #[test]
    fn message_parses() {
        let m: Message = serde_json::from_str(
            r#"{"id":"100","channel_id":"11","author":{"id":"5","username":"alice","global_name":null},
                "content":"hi","timestamp":"2026-10-26T12:00:00.000000+00:00","attachments":[],"extra":1}"#,
        )
        .unwrap();
        assert_eq!((m.id, m.author.display_name()), (Snowflake(100), "alice"));
        assert!(m.reply_to().is_none());
    }

    #[test]
    fn replies() {
        let reply = |referenced: &str| -> Message {
            serde_json::from_str(&format!(
                r#"{{"id":"101","channel_id":"11","author":{{"id":"5","username":"a"}},"timestamp":"t","type":19,
                    "message_reference":{{"message_id":"100","channel_id":"11"}}{referenced}}}"#
            ))
            .unwrap()
        };
        let resolved = reply(
            r#","referenced_message":{"id":"100","channel_id":"11","author":{"id":"6","username":"b"},"timestamp":"t"}"#,
        );
        assert_eq!(resolved.reply_to(), Some(Snowflake(100)));
        assert!(matches!(&resolved.referenced_message, Referenced::Message(m) if m.author.username == "b"));
        assert!(matches!(reply(r#","referenced_message":null"#).referenced_message, Referenced::Deleted));
        assert!(matches!(reply("").referenced_message, Referenced::Unresolved));

        // A forward also carries a reference, but is not a reply.
        let forward: Message = serde_json::from_str(
            r#"{"id":"102","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t","type":0,
                "message_reference":{"type":1,"message_id":"100","channel_id":"12"}}"#,
        )
        .unwrap();
        assert!(forward.reply_to().is_none());
        assert!(forward.is_replyable() && resolved.is_replyable());
        let pin: Message = serde_json::from_str(
            r#"{"id":"103","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t","type":6}"#,
        )
        .unwrap();
        assert!(!pin.is_replyable());
    }

    #[test]
    fn ephemeral_messages_take_no_reply() {
        let ephemeral: Message = serde_json::from_str(
            r#"{"id":"104","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t","type":20,
                "flags":64}"#,
        )
        .unwrap();
        assert!(!ephemeral.is_replyable());
    }

    #[test]
    fn headings_only_since_discord_had_them() {
        let message = |id: &str, edited: &str| -> Message {
            serde_json::from_str(&format!(
                r##"{{"id":"{id}","channel_id":"1","author":{{"id":"5","username":"a"}},"content":"# hi",
                    "timestamp":"t","edited_timestamp":{edited}}}"##
            ))
            .unwrap()
        };
        let heading = |m: &Message| matches!(m.markup()[0], crate::markup::Span::Heading { .. });
        assert!(heading(&message("1088216706570268682", "null")));
        assert!(!heading(&message("1088216706570268681", "null")));
        assert!(heading(&message("100", r#""2024-01-01T00:00:00.000000+00:00""#)), "edited since");
        assert!(!heading(&message("100", r#""2022-01-01T00:00:00.000000+00:00""#)));
    }

    #[test]
    fn stickers() {
        let sticker = |format: u8| -> Sticker {
            serde_json::from_str(&format!(r#"{{"id":"7","name":"wave","format_type":{format}}}"#)).unwrap()
        };
        assert_eq!(sticker(1).image(160), Some(("https://media.discordapp.net/stickers/7.png?size=160".into(), false)));
        assert_eq!(sticker(4).image(320), Some(("https://media.discordapp.net/stickers/7.gif?size=320".into(), true)));
        assert_eq!(sticker(2).image(160), Some(("https://media.discordapp.net/stickers/7.png?size=160".into(), true)));
        assert_eq!(sticker(3).image(160), None, "Lottie");
        assert_eq!(
            sticker(2).still(160),
            Some("https://media.discordapp.net/stickers/7.png?size=160&passthrough=false".into()),
            "a preview of an APNG is asked to stand still"
        );
        assert_eq!(sticker(4).still(160), Some("https://media.discordapp.net/stickers/7.png?size=160".into()));
        assert_eq!(sticker(1).still(80), Some("https://media.discordapp.net/stickers/7.png?size=80".into()));
        assert_eq!(sticker(3).still(160), None);
        assert_eq!(sticker(3).link(), Some("https://cdn.discordapp.com/stickers/7.json".into()));
        assert_eq!(sticker(4).link(), Some("https://media.discordapp.net/stickers/7.gif?size=160".into()));
        assert_eq!(sticker(9).link(), None);
    }

    #[test]
    fn forwards() {
        let forward: Message = serde_json::from_str(
            r##"{"id":"200","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t","content":"",
                "message_reference":{"type":1,"message_id":"100","channel_id":"12","guild_id":"3"},
                "message_snapshots":[{"message":{"type":0,"content":"# hi","timestamp":"2026-01-01T00:00:00+00:00",
                    "attachments":[],"embeds":[],"flags":0}}]}"##,
        )
        .unwrap();
        let original = forward.forwarded().unwrap();
        assert_eq!(
            (original.id, original.channel_id, original.content.as_str()),
            (Snowflake(100), Snowflake(12), "# hi")
        );
        assert!(forward.reply_to().is_none());
        // A reply's reference is no forward.
        let reply: Message = serde_json::from_str(
            r#"{"id":"201","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t","type":19,
                "message_reference":{"message_id":"100"}}"#,
        )
        .unwrap();
        assert!(reply.forwarded().is_none());
        assert_eq!(forward.copy_text().as_deref(), Some("# hi"), "the forwarded text");
    }

    #[test]
    fn copy_text_and_link() {
        let message = |content: &str, flags: u64, components: &str| -> Message {
            serde_json::from_str(&format!(
                r#"{{"id":"100","channel_id":"11","author":{{"id":"5","username":"a"}},"timestamp":"t",
                    "content":"{content}","flags":{flags},"components":{components}}}"#
            ))
            .unwrap()
        };
        assert_eq!(message("**hi**", 0, "[]").copy_text().as_deref(), Some("**hi**"), "as written");
        assert_eq!(message("", 0, "[]").copy_text(), None);
        let texts = r#"[{"type":10,"content":"a"},{"type":17,"components":[{"type":10,"content":"b"}]}]"#;
        assert_eq!(message("ignored", 1 << 15, texts).copy_text().as_deref(), Some("a\nb"));
        assert_eq!(message("", 1 << 15, r#"[{"type":14}]"#).copy_text(), None);

        let m = message("", 0, "[]");
        assert_eq!(m.link(Some(Snowflake(3))), "https://discord.com/channels/3/11/100");
        assert_eq!(m.link(None), "https://discord.com/channels/@me/11/100");
    }

    #[test]
    fn link_preview_arrives_as_an_update() {
        let mut m: Message = serde_json::from_str(
            r#"{"id":"100","channel_id":"11","author":{"id":"5","username":"a"},"content":"https://e.com",
                "timestamp":"t","flags":0}"#,
        )
        .unwrap();
        assert!(m.embeds.is_empty() && !m.embeds_suppressed());
        let embeds = vec![serde_json::from_str(r#"{"type":"link","url":"https://e.com"}"#).unwrap()];
        MessageEdit { embeds: Some(embeds), ..Default::default() }.apply(&mut m);
        assert_eq!((m.embeds.len(), m.content.as_str(), m.edited_timestamp.is_some()), (1, "https://e.com", false));
        MessageEdit { flags: Some(4), ..Default::default() }.apply(&mut m);
        assert!(m.embeds_suppressed());
    }

    #[test]
    fn a_poll_ended_early_arrives_as_an_update() {
        let poll = |expiry: &str, finalized: bool, count: u32| -> crate::poll::Poll {
            serde_json::from_value(serde_json::json!({
                "question": {"text": "Lunch?"}, "expiry": expiry,
                "answers": [{"answer_id": 1, "poll_media": {"text": "Soup"}}],
                "results": {"is_finalized": finalized, "answer_counts": [{"id": 1, "count": count, "me_voted": false}]}
            }))
            .unwrap()
        };
        let mut m: Message = serde_json::from_str(
            r#"{"id":"100","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t"}"#,
        )
        .unwrap();
        m.poll = Some(poll("2099-01-01T00:00:00+00:00", false, 2));
        m.poll.as_mut().unwrap().vote(1, true, false);
        // The update's counts aren't ours to take: the client builds them from the votes it saw.
        MessageEdit { poll: Some(poll("2026-01-01T00:00:00+00:00", true, 0)), ..Default::default() }.apply(&mut m);
        let poll = m.poll.unwrap();
        assert_eq!(poll.expiry.as_deref(), Some("2026-01-01T00:00:00+00:00"));
        assert!(poll.results.as_ref().is_some_and(|r| r.is_finalized));
        assert_eq!((poll.votes(1), poll.me_voted(1)), (3, true));
    }

    #[test]
    fn an_update_replaces_components_and_clears_what_was_picked() {
        let mut m: Message = serde_json::from_str(
            r#"{"id":"100","channel_id":"11","author":{"id":"5","username":"a"},"timestamp":"t",
                "application_id":"77","nonce":42,
                "components":[{"type":1,"components":[{"type":2,"style":1,"custom_id":"a"}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            (m.application_id, m.nonce.as_deref()),
            (Some(Snowflake(77)), Some("42")),
            "a number is a nonce too"
        );
        m.local.queue(Snowflake(1), "menu", Some(&["x".to_owned()]));
        MessageEdit { embeds: Some(Vec::new()), ..Default::default() }.apply(&mut m);
        assert_eq!(m.local.picked("menu"), None, "any update, as the client clears it");
        let update = vec![crate::component::Component::Unknown];
        MessageEdit { components: Some(update.clone()), ..Default::default() }.apply(&mut m);
        assert_eq!(m.components, update);
        assert!(!m.local.can_queue(), "what is on its way stays until Discord answers");
    }

    #[test]
    fn a_voice_messages_audio_is_its_first_attachment() {
        let attachments = serde_json::json!([
            {"id": "1", "filename": "voice-message.ogg", "url": "https://cdn.example/a"},
            {"id": "2", "filename": "other.ogg", "url": "https://cdn.example/b"}
        ]);
        let audio = |flags: u64| ours(serde_json::json!({"flags": flags, "attachments": attachments})).voice_audio();
        assert_eq!(audio(Message::IS_VOICE_MESSAGE).map(|a| a.id), Some(Snowflake(1)));
        assert_eq!(audio(0).map(|a| a.id), None, "an ordinary upload is no voice message");
        assert!(
            ours(serde_json::json!({"flags": Message::IS_VOICE_MESSAGE})).voice_audio().is_none(),
            "nothing to save"
        );

        let forward = ours(serde_json::json!({
            "message_reference": {"type": 1, "message_id": "7", "channel_id": "9"},
            "message_snapshots": [{"message": {"type": 0, "content": "", "timestamp": "", "flags": Message::IS_VOICE_MESSAGE,
                                               "attachments": attachments}}]
        }));
        assert_eq!(forward.voice_audio().map(|a| a.id), Some(Snowflake(1)), "a forward saves what it forwards");
    }

    #[test]
    fn a_hidden_channel_is_called_no_access() {
        let channel = |flags: u64| -> Channel {
            serde_json::from_value(serde_json::json!({"id": "1", "type": 0, "name": "secret-plans", "flags": flags}))
                .unwrap()
        };
        assert_eq!(channel(0).title(), "secret-plans");
        assert!(!channel(1 << 16).is_obfuscated());
        let hidden = channel(1 << 17 | 1 << 4);
        assert!(hidden.is_obfuscated());
        assert_eq!((hidden.title().as_str(), hidden.shown_name()), ("No Access", Some("No Access")));
        assert_eq!(hidden.name.as_deref(), Some("secret-plans"), "the name stays as sent");
    }
}
