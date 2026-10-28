//! The subset of Discord's data model Neucord uses so far. Unknown fields are
//! ignored, so Discord adding fields never breaks parsing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::embed::Embed;
use crate::permissions::{self, Permissions};
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

#[derive(Clone, Debug, Deserialize)]
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
}

impl User {
    /// What Discord shows: the display name if set, else the username.
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
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
    Stage,
    Forum,
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
            13 => Self::Stage,
            15 => Self::Forum,
            other => Self::Other(other),
        }
    }

    /// Channels that hold a plain message history.
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::Announcement | Self::Dm | Self::GroupDm)
    }

    /// DMs and group DMs: channels outside any guild.
    pub fn is_private(self) -> bool {
        matches!(self, Self::Dm | Self::GroupDm)
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
}

impl Channel {
    /// What the sidebar and header call this channel: its name if it has one,
    /// else (DMs, unnamed group DMs) the recipients' names.
    pub fn title(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_owned();
        }
        let names: Vec<&str> = self.recipients.iter().map(User::display_name).collect();
        if names.is_empty() { "Unnamed".to_owned() } else { names.join(", ") }
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
}

#[derive(Clone, Debug)]
pub struct Guild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub channels: Vec<Channel>,
    /// Notification level for members who never changed it: 0 all, 1 mentions.
    pub default_message_notifications: u8,
    /// The server's owner, who has every permission.
    pub owner_id: Option<Snowflake>,
    /// Every role, `@everyone`'s (the server's own id) included.
    pub roles: Vec<Role>,
    /// Moderation takes 2FA on the account (`mfa_level` 1).
    pub requires_mfa: bool,
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
fn unix_rounded_up(value: &Value) -> Option<i64> {
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
        self.verification = update.verification;
    }

    pub fn role(&self, id: Snowflake) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == id)
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

    /// Text channels we can view, in sidebar order: uncategorised first, then
    /// each category by position, channels by position within it. Returns
    /// `(category name, channels)` groups.
    pub fn sidebar(&self, me: &Account, now_unix: i64) -> Vec<(Option<&str>, Vec<&Channel>)> {
        let by_position =
            |a: &&Channel, b: &&Channel| (a.position.unwrap_or(0), a.id).cmp(&(b.position.unwrap_or(0), b.id));
        let text_in = |parent: Option<Snowflake>| {
            let mut v: Vec<&Channel> = self
                .channels
                .iter()
                .filter(|c| c.kind.is_text() && c.parent_id == parent && self.can_view(c, me, now_unix))
                .collect();
            v.sort_by(by_position);
            v
        };

        let mut groups = vec![(None, text_in(None))];
        let mut categories: Vec<&Channel> = self.channels.iter().filter(|c| c.kind == ChannelKind::Category).collect();
        categories.sort_by(by_position);
        for category in categories {
            groups.push((category.name.as_deref(), text_in(Some(category.id))));
        }
        groups.retain(|(_, channels)| !channels.is_empty());
        groups
    }

    /// Whether `channel` shows at all: what the sidebar lists, unread dots
    /// count and Neucord opens.
    pub fn can_view(&self, channel: &Channel, me: &Account, now_unix: i64) -> bool {
        permissions::in_channel(self, channel, me, now_unix).contains(Permissions::VIEW_CHANNEL)
    }
}

/// Guilds arrive in two shapes depending on the gateway capabilities: name and
/// icon either at the top level or inside `properties`. Unavailable guilds have
/// neither and are rejected here; they arrive later via GUILD_CREATE.
impl<'de> Deserialize<'de> for Guild {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
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

        let value = Value::deserialize(d)?;
        let raw = Raw::deserialize(&value).map_err(serde::de::Error::custom)?;
        let (name, icon, default, owner_id, mfa_level) = match (raw.name, raw.properties) {
            (Some(name), _) => (name, raw.icon, raw.default_message_notifications, raw.owner_id, raw.mfa_level),
            (None, Some(p)) => (p.name, p.icon, p.default_message_notifications, p.owner_id, p.mfa_level),
            (None, None) => return Err(serde::de::Error::custom("guild has no name (unavailable)")),
        };
        // Roles stay at the top level in both shapes. One we can't read is
        // left out, which only ever withholds what it grants.
        let roles = value["roles"].as_array().into_iter().flatten().filter_map(|r| Role::deserialize(r).ok()).collect();
        Ok(Guild {
            id: raw.id,
            name,
            icon,
            channels: raw.channels,
            default_message_notifications: default.unwrap_or(0),
            owner_id,
            roles,
            requires_mfa: mfa_level == Some(1),
            my_roles: Vec::new(),
            timed_out_until: None,
            verification: Rules::from_guild(&value),
            joined_at: unix_rounded_up(&value["joined_at"]),
            pending: false,
            member_flags: 0,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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
    /// What a forward carries.
    #[serde(default)]
    pub message_snapshots: Vec<Snapshot>,
    #[serde(default)]
    pub components: Vec<crate::component::Component>,
    /// For a call's system message: who joined it, and when it ended.
    #[serde(default)]
    pub call: Option<Call>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub participants: Vec<Snowflake>,
    /// ISO 8601; `None` while the call goes on.
    #[serde(default)]
    pub ended_timestamp: Option<String>,
}

impl Message {
    const SUPPRESS_EMBEDS: u64 = 1 << 2;
    const IS_VOICE_MESSAGE: u64 = 1 << 13;
    const IS_COMPONENTS_V2: u64 = 1 << 15;

    /// Built from components alone: its attachments are what they show.
    pub fn is_components_v2(&self) -> bool {
        self.flags & Self::IS_COMPONENTS_V2 != 0
    }

    /// A recorded voice message: its audio attachment plays with a waveform.
    pub fn is_voice_message(&self) -> bool {
        self.flags & Self::IS_VOICE_MESSAGE != 0
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
    /// answer (boosts, joins, …), but not, say, pins or calls.
    pub fn is_replyable(&self) -> bool {
        matches!(self.kind, 0 | 7 | 19 | 20 | 23 | 24 | 25 | 32..=42 | 45 | 46 | 69)
    }

    const FORWARD: u8 = 1;

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
            call: None,
            ..self.clone()
        })
    }

    /// The id of the message this one replies to.
    pub fn reply_to(&self) -> Option<Snowflake> {
        if self.kind != Self::REPLY {
            return None;
        }
        self.message_reference.as_ref()?.message_id
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

#[derive(Clone, Debug, Deserialize)]
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
            // PNG, and APNG (which shows its first frame).
            1 | 2 => ("png", false),
            4 => ("gif", true),
            _ => return None,
        };
        Some((format!("https://media.discordapp.net/stickers/{}.{ext}?size={size}", self.id), animated))
    }
}

#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
pub struct Snapshot {
    pub message: SnapshotMessage,
}

/// What a forward copies of the original: not its author, nor whom it
/// mentions.
#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Default)]
pub enum Referenced {
    #[default]
    Unresolved,
    Deleted,
    Message(Box<Message>),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|(cat, chans)| (*cat, chans.iter().map(|c| c.name.as_deref().unwrap()).collect()))
            .collect();
        assert_eq!(names, vec![(None, vec!["top"]), (Some("Cat"), vec!["a", "b"])], "staff is hidden from us");
        let owner = g.sidebar(&Account { id: Snowflake(7), ..Default::default() }, 0);
        assert_eq!(owner[0].1.len(), 2, "but not from the owner");
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
        assert_eq!(sticker(3).image(160), None, "Lottie");
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
}
