//! The subset of Discord's data model Neucord uses so far. Unknown fields are
//! ignored, so Discord adding fields never breaks parsing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::embed::Embed;

/// Discord IDs are 64-bit integers sent as JSON strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Snowflake(pub u64);

impl Snowflake {
    const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

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
    /// Our roles here, for role mentions. Filled in by the gateway.
    pub my_roles: Vec<Snowflake>,
}

impl Guild {
    /// Text channels in sidebar order: uncategorised first, then each
    /// category by position, channels by position within it. Returns
    /// `(category name, channels)` groups.
    pub fn sidebar(&self) -> Vec<(Option<&str>, Vec<&Channel>)> {
        let by_position =
            |a: &&Channel, b: &&Channel| (a.position.unwrap_or(0), a.id).cmp(&(b.position.unwrap_or(0), b.id));
        let text_in = |parent: Option<Snowflake>| {
            let mut v: Vec<&Channel> =
                self.channels.iter().filter(|c| c.kind.is_text() && c.parent_id == parent).collect();
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
        }

        let raw = Raw::deserialize(d)?;
        let (name, icon, default) = match (raw.name, raw.properties) {
            (Some(name), _) => (name, raw.icon, raw.default_message_notifications),
            (None, Some(p)) => (p.name, p.icon, p.default_message_notifications),
            (None, None) => return Err(serde::de::Error::custom("guild has no name (unavailable)")),
        };
        Ok(Guild {
            id: raw.id,
            name,
            icon,
            channels: raw.channels,
            default_message_notifications: default.unwrap_or(0),
            my_roles: Vec::new(),
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
    /// Only counted, to describe a sticker-only message.
    #[serde(default)]
    pub sticker_items: Vec<serde::de::IgnoredAny>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub flags: u64,
}

impl Message {
    const SUPPRESS_EMBEDS: u64 = 1 << 2;

    /// The author (or a moderator) removed the embeds.
    pub fn embeds_suppressed(&self) -> bool {
        self.flags & Self::SUPPRESS_EMBEDS != 0
    }

    /// Discord's message type for a reply.
    const REPLY: u8 = 19;

    /// Whether the official client offers Reply on this message: ordinary
    /// messages and replies, commands, and a few system messages people
    /// answer (boosts, joins, …), but not, say, pins or calls.
    pub fn is_replyable(&self) -> bool {
        matches!(self.kind, 0 | 7 | 19 | 20 | 23 | 24 | 25 | 32..=42 | 45 | 46 | 69)
    }

    /// The id of the message this one replies to.
    pub fn reply_to(&self) -> Option<Snowflake> {
        if self.kind != Self::REPLY {
            return None;
        }
        self.message_reference.as_ref()?.message_id
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MessageReference {
    #[serde(default)]
    pub message_id: Option<Snowflake>,
    #[serde(default)]
    pub channel_id: Option<Snowflake>,
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
    fn guild_with_top_level_name() {
        let g: Guild = serde_json::from_str(
            r#"{"id":"1","name":"Legacy","icon":null,"channels":[
                {"id":"10","type":4,"name":"Cat","position":0},
                {"id":"11","type":0,"name":"b","position":1,"parent_id":"10"},
                {"id":"12","type":0,"name":"a","position":0,"parent_id":"10"},
                {"id":"13","type":2,"name":"voice","position":2,"parent_id":"10"},
                {"id":"14","type":0,"name":"top","position":5}
            ]}"#,
        )
        .unwrap();
        assert_eq!(g.name, "Legacy");
        let sidebar = g.sidebar();
        let names: Vec<(Option<&str>, Vec<&str>)> = sidebar
            .iter()
            .map(|(cat, chans)| (*cat, chans.iter().map(|c| c.name.as_deref().unwrap()).collect()))
            .collect();
        assert_eq!(names, vec![(None, vec!["top"]), (Some("Cat"), vec!["a", "b"])]);
    }

    #[test]
    fn guild_with_properties() {
        let g: Guild =
            serde_json::from_str(r#"{"id":"2","properties":{"name":"New","icon":"abc"},"channels":[]}"#).unwrap();
        assert_eq!((g.name.as_str(), g.icon.as_deref()), ("New", Some("abc")));
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
