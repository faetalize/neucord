//! Forwarding a message: which messages may be, where they may go, and what
//! the request says. The client's pieces are `canForward` (843626), the
//! destination picker (92961 and the results it shows, 223863), the reason a
//! destination is greyed out (`getDestinationIsUnavailable`, 969223) and
//! `sendForward` (92961).
//!
//! Not copied: a forward from a channel gated behind a role subscription
//! (`isChannelOrThreadParentGated`, which Neucord has no notion of),
//! external stickers and the checkpoint card, which Neucord doesn't read, and
//! the experiment that bars age-restricted destinations (off unless the
//! account is in it).

use serde_json::{Value, json};

use crate::media::Source;
use crate::permissions::{self, Permissions};
use crate::scheduled::{self, SUPPRESS_NOTIFICATIONS};
use crate::switcher::{self, Hit, Home, Kind, Mode, Row, Sources};
use crate::verification::Account;
use crate::{Channel, ChannelKind, Guild, Message, Snowflake, User};

/// How many places one forward goes to (`eh` in the picker).
pub const MAX_DESTINATIONS: usize = 5;

/// What the picker lists with nothing typed (`.slice(0, 15)` in 223863).
pub const SUGGESTED: usize = 15;

/// Where a forward goes: a channel, or a person (whose DM may not exist
/// yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Destination {
    User(Snowflake),
    Channel(Snowflake),
}

impl Destination {
    /// The channel `channel` stands for: a DM is its other person (`_5`).
    pub fn of_channel(channel: &Channel) -> Destination {
        match (channel.kind, channel.recipients.first()) {
            (ChannelKind::Dm, Some(user)) => Destination::User(user.id),
            _ => Destination::Channel(channel.id),
        }
    }
}

/// The part of a message that is forwarded, when a picture or a file is
/// forwarded on its own (the viewer's Forward).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Only {
    pub attachments: Option<Vec<Snowflake>>,
    pub embeds: Option<Vec<usize>>,
}

impl Only {
    fn is_some(&self) -> bool {
        self.attachments.is_some() || self.embeds.is_some()
    }
}

impl Only {
    /// What forwarding the viewer's `source` alone sends (`eF` in 2323): an
    /// attachment by its id, an embed's media by the embed's place. That
    /// place counts among the embeds as they are drawn, which is where the
    /// message holds them (`embeds_as_held`) unless some are merged or
    /// left out; then there is no telling which one Discord means.
    pub fn of_source(source: Source, embeds_as_held: bool) -> Option<Only> {
        match source {
            Source::Attachment(id) => Some(Only { attachments: Some(vec![id]), embeds: None }),
            Source::Embed(n) | Source::Thumbnail(n) if embeds_as_held => {
                Some(Only { attachments: None, embeds: Some(vec![n]) })
            }
            _ => None,
        }
    }
}

const FORWARDABLE: [u8; 5] = [0, 19, 20, 23, 35];
/// The flags that keep a message from being forwarded: crossposts, a thread
/// started on it, a voice message, a forward, embeds suppressed, and the
/// rest of `E` in 843626.
const BARRED_FLAGS: u64 = (1 << 0) // crossposted
    | (1 << 1) // is a crosspost
    | (1 << 2) // embeds suppressed
    | (1 << 4) // urgent
    | (1 << 5) // has a thread
    | (1 << 8) // failed to mention some roles in the thread
    | (1 << 9) // hidden from the server's feed
    | (1 << 10) // a link not of Discord's warning
    | (1 << 12) // silent
    | (1 << 13) // voice message
    | (1 << 14) // has a snapshot
    | (1 << 15) // components v2
    | (1 << 19); // official

/// Whether the client offers Forward on `message` in `channel` (`p` in
/// 843626): a kind of message that carries over, with none of what can't
/// (a poll, a call, an activity, [`BARRED_FLAGS`]); readable history; and
/// not in a server that turned forwarding off or a moderator report.
pub fn can_forward(guild: Option<&Guild>, channel: &Channel, message: &Message, me: &Account, now: i64) -> bool {
    if !FORWARDABLE.contains(&message.kind)
        || message.poll.is_some()
        || message.call.is_some()
        || message.activity_instance.is_some()
        || message.flags & BARRED_FLAGS != 0
    {
        return false;
    }
    let Some(guild) = guild else { return true };
    let history = permissions::in_channel(guild, channel, me, now).contains(Permissions::READ_MESSAGE_HISTORY);
    history && !guild.features.iter().any(|f| f == "FORWARDING_DISABLED") && !permissions::is_report(guild, channel)
}

/// Whether a person is somebody to forward to (`E` in 223863): not Discord's
/// own account, and not one that was deleted (`deleted_user_` and twelve
/// hex digits, the name Discord gives it).
pub fn user_can_receive(user: &User) -> bool {
    const SYSTEM_USER: u64 = 456_226_577_798_135_808;
    let deleted = user
        .username
        .strip_prefix("deleted_user_")
        .is_some_and(|hex| hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    !(user.system || user.id.0 == SYSTEM_USER || deleted)
}

/// Whether a channel is somewhere to forward to (`E` in 223863): any group
/// DM, else a channel that isn't a forum or media channel where we may view
/// and send.
pub fn channel_can_receive(guild: Option<&Guild>, channel: &Channel, me: &Account, now: i64) -> bool {
    match channel.kind {
        ChannelKind::GroupDm => true,
        ChannelKind::Dm => permissions::can_send(None, channel, me, now),
        _ => !channel.kind.is_forum_like() && permissions::can_send(guild, channel, me, now),
    }
}

/// What `destination` is, for [`unavailable`].
#[derive(Clone, Copy)]
pub enum Target<'a> {
    User,
    Channel(Option<&'a Guild>, &'a Channel),
}

/// Why `target` is greyed out for `message` in `origin` (`pE` in 969223):
/// the words under its name, or `None` if it takes it. A forward from an
/// age-restricted channel goes only to one that is too; in a server, what
/// the message holds must be allowed there.
pub fn unavailable(
    message: &Message,
    origin: &Channel,
    target: Target<'_>,
    me: &Account,
    now: i64,
) -> Option<&'static str> {
    if origin.nsfw && !matches!(target, Target::Channel(_, c) if c.nsfw) {
        return Some("Messages cannot be forwarded from age-restricted to unrestricted channels.");
    }
    let Target::Channel(Some(guild), channel) = target else { return None };
    let can = |permission| permissions::in_channel(guild, channel, me, now).contains(permission);
    let snapshots = || message.message_snapshots.iter().map(|s| &s.message);
    if (!message.attachments.is_empty() || snapshots().any(|s| !s.attachments.is_empty()))
        && !can(Permissions::ATTACH_FILES)
    {
        return Some("Attachments are disabled in this channel.");
    }
    // Text carries a link along, so embeds are lost only when nothing else is.
    let has_words =
        !message.content.is_empty() || snapshots().any(|s| !s.content.is_empty() || !s.attachments.is_empty());
    if (!message.embeds.is_empty() || snapshots().any(|s| !s.embeds.is_empty()))
        && !can(Permissions::EMBED_LINKS)
        && !has_words
    {
        return Some("Embeds are disabled in this channel.");
    }
    if (message.is_voice_message() || snapshots().any(|s| s.flags & Message::IS_VOICE_MESSAGE != 0))
        && !can(Permissions::SEND_VOICE_MESSAGES)
    {
        return Some("Voice messages are disabled in this channel.");
    }
    None
}

/// The `message_reference` of a forward (`s` in `sendForward`): where the
/// message is, `type` 1, and, for one picture or file, what of it
/// (`forward_only`). `guild` is the original's server, if it is in one.
pub fn reference(guild: Option<Snowflake>, message: &Message, only: &Only) -> Value {
    let mut reference = json!({ "channel_id": message.channel_id, "message_id": message.id, "type": 1 });
    if let Some(guild) = guild {
        reference["guild_id"] = json!(guild);
    }
    if only.is_some() {
        let mut parts = json!({});
        if let Some(ids) = &only.attachments {
            parts["attachment_ids"] = json!(ids);
        }
        if let Some(indices) = &only.embeds {
            parts["embed_indices"] = json!(indices);
        }
        reference["forward_only"] = parts;
    }
    reference
}

/// What typing `comment` with a forward does: its text, and the flags of
/// both messages (a leading `@silent` makes them silent, `O.Ay`).
pub fn comment(comment: &str) -> (String, u64) {
    scheduled::split_silent(comment, 0)
}

/// Whether the comment goes too: it does unless slowmode would refuse it
/// right after the forward (`lP` in 969223), which we can't skip.
pub fn comment_goes(text: &str, channel: &Channel, bypasses_slowmode: bool) -> bool {
    !text.is_empty() && !(channel.rate_limit_per_user > 0 && !bypasses_slowmode)
}

impl Destination {
    /// Where a result goes: a person, or a channel.
    pub fn of_hit(hit: &Hit) -> Destination {
        match hit.kind {
            Kind::User => Destination::User(hit.id),
            _ => Destination::Channel(hit.id),
        }
    }

    fn hit(self, sources: &Sources) -> Option<Hit> {
        match self {
            Destination::User(id) => sources.user_hit(id),
            Destination::Channel(id) => sources.as_hit(id),
        }
    }
}

/// What the picker lists (`Ay` in 223863) for what is `typed` (the same
/// prefixes as the quick switcher's, narrowing to people, text or voice
/// channels): with something typed, what the search finds; with nothing,
/// what we picked, then the channels we opened and use most, without where
/// the message is from unless it is picked, and only [`SUGGESTED`] of them.
/// Only what `eligible` takes is listed, once.
pub fn listing(
    sources: &Sources,
    typed: &str,
    selected: &[Destination],
    history: &[Snowflake],
    frequent: &[Snowflake],
    origin: Destination,
    eligible: &dyn Fn(&Hit) -> bool,
) -> Vec<Hit> {
    let mut query = switcher::parse(typed);
    // Servers and game profiles aren't places a message goes (the client
    // searches people and channels whatever the prefix).
    let narrowed = match query.mode {
        Some(Mode::Users | Mode::AllUsers) => Some(Kind::User),
        Some(Mode::Channels) => Some(Kind::Channel),
        Some(Mode::Voice) => Some(Kind::Voice),
        _ => None,
    };
    if narrowed.is_none() {
        query.mode = None;
    }
    let mut seen = std::collections::HashSet::new();
    if query.text.is_empty() && !selected.contains(&origin) {
        seen.insert(origin);
    }
    let mut keep = |hit: &Hit| eligible(hit) && seen.insert(Destination::of_hit(hit));
    if !query.text.is_empty() {
        let found = switcher::search(sources, &Home::default(), &query);
        let hits = found.into_iter().filter_map(|row| match row {
            Row::Hit(hit) if !matches!(hit.kind, Kind::Server) => Some(hit),
            _ => None,
        });
        return hits.filter(|hit| keep(hit)).collect();
    }
    let channels = history.iter().chain(frequent).filter_map(|&id| sources.as_hit(id));
    let hits = selected.iter().filter_map(|d| d.hit(sources)).chain(channels);
    let kept = hits.filter(|hit| narrowed.is_none_or(|kind| hit.kind == kind)).filter(|hit| keep(hit));
    match narrowed {
        Some(_) => kept.collect(),
        None => kept.take(SUGGESTED).collect(),
    }
}

/// What the picker shows of the message (`eR` in 92961): its text, and a line
/// for what it holds besides.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// Empty when the message has no text, or a single attachment is forwarded.
    pub text: String,
    /// "2 images", "1 file", ...
    pub summary: Option<String>,
}

/// The message as the picker shows it, for what of it goes (`only`).
pub fn preview(message: &Message, only: &Only) -> Preview {
    let (content, attachments, embeds) = match message.message_snapshots.first() {
        Some(snapshot) => (&snapshot.message.content, &snapshot.message.attachments, &snapshot.message.embeds),
        None => (&message.content, &message.attachments, &message.embeds),
    };
    let attachments: Vec<_> = match (&only.attachments, &only.embeds) {
        (Some(ids), _) => attachments.iter().filter(|a| ids.contains(&a.id)).collect(),
        (None, Some(_)) => Vec::new(),
        (None, None) => attachments.iter().collect(),
    };
    let embeds: Vec<_> = match (&only.embeds, &only.attachments) {
        (Some(indices), _) => embeds.iter().enumerate().filter(|(i, _)| indices.contains(i)).map(|(_, e)| e).collect(),
        (None, Some(_)) => Vec::new(),
        (None, None) => embeds.iter().collect(),
    };
    let mut text = content.clone();
    if only.embeds.is_some() || (text.is_empty() && !embeds.is_empty()) {
        text = embeds.iter().filter_map(|e| e.url.as_deref()).collect::<Vec<_>>().join("\n");
    }
    if text.is_empty() {
        text = embeds.first().and_then(|e| e.description.clone()).unwrap_or_default();
    }
    if only.attachments.is_some() {
        text.clear();
    }
    let kind = |a: &&crate::Attachment, prefix: &str| a.content_type.as_deref().is_some_and(|t| t.starts_with(prefix));
    let images = attachments.iter().filter(|a| kind(a, "image/")).count();
    let videos = attachments.iter().filter(|a| kind(a, "video/")).count();
    let noun = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    let summary = match (images, videos, attachments.len()) {
        (_, _, 0) => None,
        (i, v, _) if i > 0 && v > 0 => Some(format!("{}, {}", noun(i, "image"), noun(v, "video"))),
        (_, v, _) if v > 0 => Some(noun(v, "video")),
        (i, _, _) if i > 0 => Some(noun(i, "image")),
        (_, _, n) => Some(noun(n, "file")),
    };
    Preview { text, summary }
}

/// What the flags of a forward's messages are.
pub const SILENT: u64 = SUPPRESS_NOTIFICATIONS;

#[cfg(test)]
mod tests {
    use super::*;

    fn message(extra: Value) -> Message {
        let mut message = json!({
            "id": "10", "channel_id": "20", "timestamp": "2024-01-01T00:00:00+00:00",
            "author": {"id": "1", "username": "a", "discriminator": "0"}, "content": "hi", "type": 0,
        });
        message.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(message).unwrap()
    }

    fn channel(extra: Value) -> Channel {
        let mut channel = json!({"id": "20", "type": 0, "guild_id": "5"});
        channel.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(channel).unwrap()
    }

    fn me() -> Account {
        Account {
            id: Snowflake(1),
            verified: true,
            email: true,
            phone: false,
            mfa: false,
            nsfw_allowed: false,
            nsfw_set: false,
            age_status: None,
        }
    }

    fn guild(extra: Value) -> Guild {
        let all = (Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY).0;
        let mut guild = json!({
            "id": "5", "name": "g", "owner_id": "9",
            "roles": [{"id": "5", "name": "@everyone", "permissions": all.to_string()}],
        });
        guild.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(guild).unwrap()
    }

    #[test]
    fn only_some_messages_can_be_forwarded() {
        let (guild, channel) = (guild(json!({})), channel(json!({})));
        let can = |m: Message| can_forward(Some(&guild), &channel, &m, &me(), 0);
        assert!(can(message(json!({}))));
        assert!(can(message(json!({"type": 19}))), "a reply");
        assert!(!can(message(json!({"type": 7}))), "a join");
        assert!(!can(message(json!({"flags": 1 << 13}))), "a voice message");
        assert!(!can(message(json!({"flags": 1 << 14}))), "a forward");
        assert!(!can(message(json!({"flags": 1 << 5}))), "one with a thread");
        assert!(!can(message(json!({"call": {"participants": []}}))));
        assert!(!can_forward(None, &channel, &message(json!({"type": 7})), &me(), 0));
        assert!(can_forward(None, &channel, &message(json!({})), &me(), 0), "a DM needs no permission");
    }

    #[test]
    fn a_server_can_turn_forwarding_off_or_hide_history() {
        let channel = channel(json!({}));
        let m = message(json!({}));
        assert!(!can_forward(Some(&guild(json!({"features": ["FORWARDING_DISABLED"]}))), &channel, &m, &me(), 0));
        let blind = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES;
        let blind = guild(json!({"roles": [{"id": "5", "name": "@everyone", "permissions": blind.0.to_string()}]}));
        assert!(!can_forward(Some(&blind), &channel, &m, &me(), 0), "no Read Message History");
    }

    #[test]
    fn deleted_and_system_accounts_are_not_destinations() {
        let user = |name: &str, system: bool| -> User {
            serde_json::from_value(json!({"id": "7", "username": name, "discriminator": "0", "system": system}))
                .unwrap()
        };
        assert!(user_can_receive(&user("someone", false)));
        assert!(!user_can_receive(&user("deleted_user_0123456789ab", false)));
        assert!(user_can_receive(&user("deleted_user_xyz", false)), "not the pattern");
        assert!(!user_can_receive(&user("Discord", true)));
    }

    #[test]
    fn forums_and_unwritable_channels_are_not_destinations() {
        let g = guild(json!({}));
        assert!(channel_can_receive(Some(&g), &channel(json!({})), &me(), 0));
        assert!(!channel_can_receive(Some(&g), &channel(json!({"type": 15})), &me(), 0), "a forum");
        let read_only = guild(
            json!({"roles": [{"id": "5", "name": "@everyone", "permissions": Permissions::VIEW_CHANNEL.0.to_string()}]}),
        );
        assert!(!channel_can_receive(Some(&read_only), &channel(json!({})), &me(), 0));
        let group = serde_json::from_value(json!({"id": "30", "type": 3})).unwrap();
        assert!(channel_can_receive(None, &group, &me(), 0));
    }

    #[test]
    fn what_a_message_holds_must_be_allowed_where_it_goes() {
        let origin = channel(json!({}));
        let target_guild = guild(
            json!({"roles": [{"id": "5", "name": "@everyone", "permissions": (Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES).0.to_string()}]}),
        );
        let target = channel(json!({"id": "21"}));
        let why = |m: &Message| unavailable(m, &origin, Target::Channel(Some(&target_guild), &target), &me(), 0);
        let file = json!({"attachments": [{"id": "3", "filename": "a.png", "url": "https://x/a.png"}]});
        assert_eq!(why(&message(file)), Some("Attachments are disabled in this channel."));
        let embed = json!({"content": "", "embeds": [{"type": "rich", "title": "t"}]});
        assert_eq!(why(&message(embed)), Some("Embeds are disabled in this channel."));
        let linked = json!({"embeds": [{"type": "rich", "title": "t"}]});
        assert_eq!(why(&message(linked)), None, "its text carries the link");
        assert_eq!(why(&message(json!({"flags": 1 << 13}))), Some("Voice messages are disabled in this channel."));
        assert_eq!(why(&message(json!({}))), None);
        assert_eq!(unavailable(&message(json!({})), &origin, Target::User, &me(), 0), None);
    }

    #[test]
    fn age_restricted_messages_stay_among_age_restricted_places() {
        let origin = channel(json!({"nsfw": true}));
        let g = guild(json!({}));
        let m = message(json!({}));
        let words = Some("Messages cannot be forwarded from age-restricted to unrestricted channels.");
        assert_eq!(unavailable(&m, &origin, Target::User, &me(), 0), words);
        assert_eq!(unavailable(&m, &origin, Target::Channel(Some(&g), &channel(json!({"id": "21"}))), &me(), 0), words);
        assert_eq!(
            unavailable(&m, &origin, Target::Channel(Some(&g), &channel(json!({"id": "21", "nsfw": true}))), &me(), 0),
            None
        );
    }

    #[test]
    fn the_reference_names_the_message_and_what_of_it() {
        let m = message(json!({}));
        assert_eq!(
            reference(Some(Snowflake(5)), &m, &Only::default()),
            json!({"guild_id": "5", "channel_id": "20", "message_id": "10", "type": 1})
        );
        assert_eq!(reference(None, &m, &Only::default()), json!({"channel_id": "20", "message_id": "10", "type": 1}));
        let one = Only { attachments: Some(vec![Snowflake(3)]), embeds: None };
        assert_eq!(reference(None, &m, &one)["forward_only"], json!({"attachment_ids": ["3"]}));
        let embed = Only { attachments: None, embeds: Some(vec![0]) };
        assert_eq!(reference(None, &m, &embed)["forward_only"], json!({"embed_indices": [0]}));
    }

    #[test]
    fn the_viewer_forwards_one_attachment_or_one_embed() {
        let attachment = Only::of_source(Source::Attachment(Snowflake(3)), false).unwrap();
        assert_eq!(attachment, Only { attachments: Some(vec![Snowflake(3)]), embeds: None });
        let embed = Only { attachments: None, embeds: Some(vec![1]) };
        assert_eq!(Only::of_source(Source::Embed(1), true), Some(embed.clone()));
        assert_eq!(Only::of_source(Source::Thumbnail(1), true), Some(embed));
        assert_eq!(Only::of_source(Source::Embed(1), false), None, "merged embeds have no place to name");
    }

    #[test]
    fn the_comment_goes_unless_slowmode_would_refuse_it() {
        assert_eq!(comment("@silent hello"), ("hello".to_owned(), SILENT));
        assert_eq!(comment("hello"), ("hello".to_owned(), 0));
        let slow = channel(json!({"rate_limit_per_user": 5}));
        assert!(!comment_goes("hello", &slow, false));
        assert!(comment_goes("hello", &slow, true));
        assert!(!comment_goes("", &channel(json!({})), false));
        assert!(comment_goes("hello", &channel(json!({})), false));
    }

    #[test]
    fn the_preview_shows_the_text_and_what_goes_with_it() {
        let file =
            |id: &str, kind: &str| json!({"id": id, "filename": "f", "url": "https://x/f", "content_type": kind});
        let m =
            message(json!({"attachments": [file("3", "image/png"), file("4", "video/mp4"), file("5", "text/plain")]}));
        let all = preview(&m, &Only::default());
        assert_eq!((all.text.as_str(), all.summary.as_deref()), ("hi", Some("1 image, 1 video")));
        let one = preview(&m, &Only { attachments: Some(vec![Snowflake(5)]), embeds: None });
        assert_eq!((one.text.as_str(), one.summary.as_deref()), ("", Some("1 file")), "one file goes alone");
        let two = message(json!({"content": "", "attachments": [file("3", "image/png"), file("4", "image/png")]}));
        assert_eq!(preview(&two, &Only::default()).summary.as_deref(), Some("2 images"));
        let embed = json!({"content": "", "embeds": [{"type": "link", "url": "https://x.example/a"}, {"type": "link", "url": "https://x.example/b"}]});
        let embed = message(embed);
        assert_eq!(preview(&embed, &Only::default()).text, "https://x.example/a\nhttps://x.example/b");
        let second = preview(&embed, &Only { attachments: None, embeds: Some(vec![1]) });
        assert_eq!((second.text.as_str(), second.summary), ("https://x.example/b", None));
    }

    struct World {
        guilds: Vec<Guild>,
        private: Vec<Channel>,
        relationships: crate::relationships::Relationships,
    }

    impl World {
        fn new() -> World {
            let guild = serde_json::from_value(json!({
                "id": "1", "name": "Rustaceans",
                "channels": [
                    {"id": "11", "type": 0, "name": "general"}, {"id": "12", "type": 0, "name": "rust-help"},
                    {"id": "13", "type": 2, "name": "lounge"}, {"id": "14", "type": 15, "name": "ideas"},
                ],
            }))
            .unwrap();
            let user = |id: &str, name: &str| json!({"id": id, "username": name});
            let private = |id: &str, kind: u8, name: Value, people: Vec<Value>| {
                serde_json::from_value(json!({"id": id, "type": kind, "name": name, "recipients": people})).unwrap()
            };
            World {
                guilds: vec![guild],
                private: vec![
                    private("30", 1, Value::Null, vec![user("300", "alice")]),
                    private("31", 3, json!("book club"), vec![user("301", "bob"), user("302", "carol")]),
                ],
                relationships: crate::relationships::Relationships::from_ready(&json!({
                    "relationships": [{"id": "303", "type": 1, "user": user("303", "dave")}],
                })),
            }
        }

        fn list(
            &self,
            typed: &str,
            selected: &[Destination],
            history: &[u64],
            origin: Destination,
        ) -> Vec<Destination> {
            let access = |_: &Guild, _: &Channel| true;
            let member = |_: Snowflake, _: Snowflake| false;
            let sources = Sources {
                me: Snowflake(999),
                guild: None,
                channel: None,
                guilds: &self.guilds,
                private_channels: &self.private,
                relationships: &self.relationships,
                can_access: &access,
                is_member: &member,
            };
            let history: Vec<_> = history.iter().map(|&i| Snowflake(i)).collect();
            // No forum, as `channel_can_receive` says.
            let eligible = |hit: &Hit| hit.id != Snowflake(14);
            listing(&sources, typed, selected, &history, &[], origin, &eligible)
                .iter()
                .map(Destination::of_hit)
                .collect()
        }
    }

    #[test]
    fn with_nothing_typed_the_picked_come_first_and_the_origin_is_left_out() {
        let world = World::new();
        let (alice, general) = (Destination::User(Snowflake(300)), Destination::Channel(Snowflake(11)));
        let origin = Destination::Channel(Snowflake(12));
        let all = world.list("", &[], &[30, 12, 11, 31, 11], origin);
        assert_eq!(all, [alice, general, Destination::Channel(Snowflake(31))], "once each, a DM as its person");
        let picked = world.list("", &[Destination::Channel(Snowflake(31))], &[30, 12, 11], origin);
        assert_eq!(picked[0], Destination::Channel(Snowflake(31)));
        assert_eq!(picked.len(), 3);
        // Where the message is from shows once it is picked.
        let from = world.list("", &[origin], &[30, 12], origin);
        assert_eq!(from, [origin, alice]);
        assert_eq!(world.list("@", &[], &[30, 12, 11], origin), [alice], "narrowed to people");
        assert_eq!(world.list("#", &[], &[30, 12, 11], origin), [general]);
    }

    #[test]
    fn a_search_finds_people_and_channels_but_not_servers_or_ineligible_ones() {
        let world = World::new();
        let origin = Destination::Channel(Snowflake(11));
        let found = world.list("rust", &[], &[], origin);
        assert!(found.contains(&Destination::Channel(Snowflake(12))), "{found:?}");
        assert!(!found.contains(&Destination::Channel(Snowflake(1))), "not the server");
        assert_eq!(world.list("alice", &[], &[], origin), [Destination::User(Snowflake(300))]);
        assert_eq!(
            world.list("dave", &[], &[], origin),
            [Destination::User(Snowflake(303))],
            "a friend with no DM yet"
        );
        assert!(world.list("ideas", &[], &[], origin).is_empty(), "a forum takes no messages");
        // The origin is found by a search; only the empty list leaves it out.
        assert_eq!(world.list("general", &[], &[], origin), [origin]);
    }
}
