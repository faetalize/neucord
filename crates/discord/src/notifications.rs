//! The user's per-server notification settings (`user_guild_settings`), and
//! the decision Discord's client makes from them: does this message mention
//! me, is this channel muted, should it raise a desktop notification?
//!
//! Levels resolve from the most specific setting: channel override, then its
//! category's override, then the server setting, then the server's default.
//! DM settings live in the entry whose `guild_id` is null.
//!
//! Threads go by our membership instead: see [`NotificationSettings::thread_level`].

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{Channel, Message, Snowflake};

/// `message_notifications` value meaning "inherit from the level above".
const INHERIT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    All,
    Mentions,
    Nothing,
}

impl Level {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Level::All),
            1 => Some(Level::Mentions),
            2 => Some(Level::Nothing),
            _ => None,
        }
    }
}

fn inherit() -> u8 {
    INHERIT
}

/// How long a mute lasts.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MuteConfig {
    #[serde(default)]
    end_time: Option<String>,
}

/// Muted with no end time, or an end time still in the future.
pub(crate) fn mute_active(muted: bool, config: Option<&MuteConfig>, now_unix: i64) -> bool {
    if !muted {
        return false;
    }
    match config.and_then(|c| c.end_time.as_deref()) {
        None => true,
        Some(end) => glib::DateTime::from_iso8601(end, Some(&glib::TimeZone::utc()))
            .map(|t| t.to_unix() > now_unix)
            // Unparseable: trust the flag.
            .unwrap_or(true),
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ChannelOverride {
    channel_id: Snowflake,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    mute_config: Option<MuteConfig>,
    #[serde(default = "inherit")]
    message_notifications: u8,
}

/// One server's settings, or the DM settings when `guild_id` is `None`.
#[derive(Clone, Debug, Deserialize)]
pub struct GuildSettings {
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    mute_config: Option<MuteConfig>,
    #[serde(default = "inherit")]
    message_notifications: u8,
    #[serde(default)]
    suppress_everyone: bool,
    #[serde(default)]
    suppress_roles: bool,
    #[serde(default)]
    channel_overrides: Vec<ChannelOverride>,
}

impl GuildSettings {
    fn channel(&self, id: Snowflake) -> Option<&ChannelOverride> {
        self.channel_overrides.iter().find(|o| o.channel_id == id)
    }
}

/// Where a message was posted, as far as notification rules care.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// `None` for DMs.
    pub guild: Option<Snowflake>,
    /// The server's `default_message_notifications` (0 all, 1 mentions).
    pub guild_default: u8,
    pub channel: Snowflake,
    /// The channel's category.
    pub parent: Option<Snowflake>,
}

#[derive(Clone, Debug, Default)]
pub struct NotificationSettings {
    by_guild: HashMap<Option<Snowflake>, GuildSettings>,
}

impl NotificationSettings {
    /// From READY's `user_guild_settings`: a plain array, or `{entries: [...]}`.
    pub fn from_ready(value: &Value) -> Self {
        let entries = value.get("entries").unwrap_or(value);
        let by_guild = entries
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|raw| GuildSettings::deserialize(raw).ok())
            .map(|s| (s.guild_id, s))
            .collect();
        NotificationSettings { by_guild }
    }

    /// A USER_GUILD_SETTINGS_UPDATE: replaces that server's settings.
    pub fn update(&mut self, settings: GuildSettings) {
        self.by_guild.insert(settings.guild_id, settings);
    }

    pub fn guild_muted(&self, guild: Snowflake, now_unix: i64) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| mute_active(s.muted, s.mute_config.as_ref(), now_unix))
    }

    /// The channel itself or its category is muted (not the server).
    pub fn channel_muted(&self, place: &Place, now_unix: i64) -> bool {
        let Some(settings) = self.by_guild.get(&place.guild) else { return false };
        [Some(place.channel), place.parent]
            .into_iter()
            .flatten()
            .filter_map(|id| settings.channel(id))
            .any(|o| mute_active(o.muted, o.mute_config.as_ref(), now_unix))
    }

    /// Anything here is muted: the server, the category or the channel.
    pub fn muted(&self, place: &Place, now_unix: i64) -> bool {
        place.guild.is_some_and(|g| self.guild_muted(g, now_unix)) || self.channel_muted(place, now_unix)
    }

    pub fn level(&self, place: &Place) -> Level {
        let settings = self.by_guild.get(&place.guild);
        let overrides = [Some(place.channel), place.parent]
            .into_iter()
            .flatten()
            .filter_map(|id| settings.and_then(|s| s.channel(id)))
            .map(|o| o.message_notifications);
        overrides
            .chain(settings.map(|s| s.message_notifications))
            .find_map(Level::from_raw)
            .or_else(|| Level::from_raw(place.guild_default))
            .unwrap_or(Level::All)
    }

    /// Which messages in `thread` notify (607508): what we chose for it, and
    /// otherwise its parent's level, at `parent`, unless anything there is
    /// muted. A thread we haven't joined notifies nothing.
    pub fn thread_level(&self, thread: &Channel, parent: &Place, now_unix: i64) -> Level {
        let Some(member) = &thread.member else { return Level::Nothing };
        if member.flags & thread_flags::ALL_MESSAGES != 0 {
            Level::All
        } else if member.flags & thread_flags::ONLY_MENTIONS != 0 {
            Level::Mentions
        } else if member.flags & thread_flags::NO_MESSAGES != 0 || self.muted(parent, now_unix) {
            Level::Nothing
        } else {
            self.level(parent)
        }
    }

    /// Should `message`, in `thread`, raise a desktop notification
    /// (411238)? Not in a thread we muted, and not where `allowNoMessages`
    /// (543465) blocks it first: it asks about the thread as a channel,
    /// whose "category" is the parent channel, so it sees the server's mute
    /// and the parent's own mute and level, never the category above it.
    /// Mentions count here whatever the server suppresses.
    pub fn should_notify_in_thread(
        &self,
        thread: &Channel,
        parent: &Place,
        message: &Message,
        me: Snowflake,
        my_roles: &[Snowflake],
        now_unix: i64,
    ) -> bool {
        if thread.member.as_ref().is_none_or(|m| m.is_muted(now_unix)) {
            return false;
        }
        let as_channel = Place { parent: None, ..*parent };
        if self.muted(&as_channel, now_unix) || self.level(&as_channel) == Level::Nothing {
            return false;
        }
        match self.thread_level(thread, parent, now_unix) {
            Level::Nothing => false,
            Level::Mentions => mentions(message, me, my_roles, false, false),
            Level::All => true,
        }
    }

    /// Whether `message` pings us: directly, via @everyone/@here, or via one
    /// of our roles, unless the server suppresses that kind.
    pub fn mentions_me(&self, message: &Message, me: Snowflake, my_roles: &[Snowflake]) -> bool {
        let settings = self.by_guild.get(&message.guild_id);
        let suppress_everyone = settings.is_some_and(|s| s.suppress_everyone);
        let suppress_roles = settings.is_some_and(|s| s.suppress_roles);
        mentions(message, me, my_roles, suppress_everyone, suppress_roles)
    }

    /// Does a message add to the channel's mention count (the read state's
    /// MESSAGE_CREATE, 218461)? `pinged` is `mentions_me`. Nothing from
    /// a message `is_dropped` drops. Otherwise a ping, or any message in
    /// a DM that isn't muted: a DM is a mention by itself.
    pub fn counts_mention(
        &self,
        place: Option<&Place>,
        group_dm: bool,
        message: &Message,
        pinged: bool,
        blocked: impl Fn(Snowflake) -> bool,
        now_unix: i64,
    ) -> bool {
        !is_dropped(message, group_dm, blocked)
            && (pinged || place.is_some_and(|p| p.guild.is_none() && !self.channel_muted(p, now_unix)))
    }

    /// Should a message here raise a desktop notification? DMs notify unless
    /// muted. In servers, `allowNoMessages` (543465) comes first: a mute on
    /// the server, category or channel, or a level of "nothing", silences
    /// everything, mentions included. Then `mentioned` (`mentions_me`, so
    /// what the server suppresses is already out) decides under "mentions".
    pub fn should_notify(&self, place: &Place, mentioned: bool, now_unix: i64) -> bool {
        if place.guild.is_none() {
            return !self.channel_muted(place, now_unix);
        }
        if self.muted(place, now_unix) {
            return false;
        }
        match self.level(place) {
            Level::Nothing => false,
            Level::Mentions => mentioned,
            Level::All => true,
        }
    }
}

/// Messages the client neither counts nor notifies (218461, 411238): from
/// a blocked author, or answering a command a blocked user ran
/// (`isBlockedOrIgnoredForMessage`), and a group DM's notice that someone
/// left. Ignored users and a guild space's leaderboard aren't modelled.
pub fn is_dropped(message: &Message, group_dm: bool, blocked: impl Fn(Snowflake) -> bool) -> bool {
    (group_dm && message.kind == RECIPIENT_REMOVE)
        || blocked(message.author.id)
        || message.interaction_metadata.as_ref().is_some_and(|i| blocked(i.user.id))
}

const RECIPIENT_REMOVE: u8 = 2;

fn mentions(message: &Message, me: Snowflake, my_roles: &[Snowflake], no_everyone: bool, no_roles: bool) -> bool {
    message.mentions.iter().any(|u| u.id == me)
        || (message.mention_everyone && !no_everyone)
        || (!no_roles && message.mention_roles.iter().any(|r| my_roles.contains(r)))
}

/// Our thread member's flags: which messages in the thread notify.
mod thread_flags {
    pub const ALL_MESSAGES: u64 = 1 << 1;
    pub const ONLY_MENTIONS: u64 = 1 << 2;
    pub const NO_MESSAGES: u64 = 1 << 3;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000;

    fn settings() -> NotificationSettings {
        NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "muted": false, "message_notifications": 1, "suppress_everyone": true,
             "channel_overrides": [
                {"channel_id": "10", "muted": false, "message_notifications": 0},
                {"channel_id": "11", "muted": true, "mute_config": null, "message_notifications": 3},
                {"channel_id": "20", "muted": true, "mute_config": {"end_time": "2020-01-01T00:00:00Z"}, "message_notifications": 3}
             ]},
            {"guild_id": "2", "muted": true, "mute_config": null, "message_notifications": 3, "channel_overrides": []},
            {"guild_id": null, "channel_overrides": [{"channel_id": "99", "muted": true}]}
        ]))
    }

    fn place(guild: Option<u64>, channel: u64, parent: Option<u64>) -> Place {
        Place {
            guild: guild.map(Snowflake),
            guild_default: 0,
            channel: Snowflake(channel),
            parent: parent.map(Snowflake),
        }
    }

    #[test]
    fn levels_resolve_most_specific_first() {
        let s = settings();
        assert_eq!(s.level(&place(Some(1), 10, None)), Level::All, "channel override");
        assert_eq!(s.level(&place(Some(1), 12, Some(10))), Level::All, "category override");
        assert_eq!(s.level(&place(Some(1), 12, None)), Level::Mentions, "server setting");
        assert_eq!(s.level(&place(Some(2), 5, None)), Level::All, "server default");
        let mut quiet = place(Some(3), 5, None);
        quiet.guild_default = 1;
        assert_eq!(s.level(&quiet), Level::Mentions, "no settings at all: server default");
    }

    #[test]
    fn mutes_and_expiry() {
        let s = settings();
        assert!(s.channel_muted(&place(Some(1), 11, None), NOW));
        assert!(s.channel_muted(&place(Some(1), 12, Some(11)), NOW), "muted category");
        assert!(!s.channel_muted(&place(Some(1), 20, None), NOW), "mute expired");
        assert!(s.guild_muted(Snowflake(2), NOW));
        assert!(s.muted(&place(Some(2), 5, None), NOW));
    }

    #[test]
    fn mention_counts() {
        let s = settings();
        let dm = place(None, 5, None);
        let muted_dm = place(None, 99, None);
        let guild = place(Some(1), 10, None);
        let message = |extra: Value| -> Message {
            let mut m = json!({"id": "1", "channel_id": "5", "author": {"id": "7", "username": "x"},
                               "content": "", "timestamp": "2026-10-26T00:00:00+00:00"});
            m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(m).unwrap()
        };
        let plain = message(json!({}));
        let nobody = |_| false;
        let count = |place, group_dm, message: &Message, pinged, blocked: &[u64]| {
            let blocked = |id: Snowflake| blocked.contains(&id.0);
            s.counts_mention(Some(place), group_dm, message, pinged, blocked, NOW)
        };
        assert!(s.counts_mention(Some(&dm), false, &plain, false, nobody, NOW), "a DM is a mention");
        assert!(!count(&muted_dm, false, &plain, false, &[]), "not in a muted DM");
        assert!(count(&muted_dm, false, &plain, true, &[]), "unless it pings us");
        assert!(!count(&guild, false, &plain, false, &[]), "a server message needs a ping");
        assert!(count(&guild, false, &plain, true, &[]));
        assert!(!s.counts_mention(None, false, &plain, false, nobody, NOW), "an unknown channel isn't a DM");
        assert!(s.counts_mention(None, false, &plain, true, nobody, NOW));

        for place in [&dm, &guild] {
            assert!(!count(place, false, &plain, true, &[7]), "never from a blocked author");
        }
        let ran = message(json!({"interaction_metadata": {"user": {"id": "8", "username": "y"}}}));
        assert!(!count(&dm, false, &ran, true, &[8]), "nor an app's answer to a blocked user's command");
        assert!(count(&dm, false, &ran, true, &[9]), "someone else blocked");

        let left = message(json!({"type": 2}));
        assert!(!count(&dm, true, &left, true, &[]), "a group DM's notice that someone left");
        assert!(count(&dm, false, &left, true, &[]), "in a DM it is an ordinary message");
        assert!(count(&dm, true, &message(json!({"type": 1})), true, &[]), "other notices count");
    }

    #[test]
    fn notify_decisions() {
        let s = settings();
        assert!(s.should_notify(&place(Some(1), 10, None), false, NOW), "level all");
        assert!(!s.should_notify(&place(Some(1), 12, None), false, NOW), "mentions only, not mentioned");
        assert!(s.should_notify(&place(Some(1), 12, None), true, NOW));
        assert!(!s.should_notify(&place(Some(2), 5, None), false, NOW), "muted server");
        assert!(!s.should_notify(&place(Some(2), 5, None), true, NOW), "a mute silences mentions too");
        assert!(!s.should_notify(&place(Some(1), 11, None), true, NOW), "muted channel, mentions level");
        assert!(!s.should_notify(&place(Some(1), 12, Some(11)), true, NOW), "muted category");
        assert!(s.should_notify(&place(Some(1), 20, None), true, NOW), "an expired mute no longer silences");
        assert!(!s.should_notify(&place(Some(1), 20, None), false, NOW), "but the level still applies");
        assert!(s.should_notify(&place(None, 98, None), false, NOW), "DM");
        assert!(!s.should_notify(&place(None, 99, None), false, NOW), "muted DM");
    }

    #[test]
    fn threads_go_by_our_membership() {
        let s = settings();
        let thread = |member: Value| -> Channel {
            serde_json::from_value(json!({"id": "30", "type": 11, "parent_id": "12", "member": member})).unwrap()
        };
        let member = |flags: u64| json!({"flags": flags, "muted": false, "join_timestamp": "2026-10-28T00:00:00Z"});
        let parent = place(Some(1), 12, None);
        assert_eq!(s.thread_level(&thread(Value::Null), &parent, NOW), Level::Nothing, "not joined");
        assert_eq!(s.thread_level(&thread(member(0)), &parent, NOW), Level::Mentions, "the parent's: the server's");
        assert_eq!(s.thread_level(&thread(member(0)), &place(Some(1), 10, None), NOW), Level::All);
        assert_eq!(s.thread_level(&thread(member(1 << 1)), &parent, NOW), Level::All, "our choice wins");
        assert_eq!(s.thread_level(&thread(member(1 << 3)), &place(Some(1), 10, None), NOW), Level::Nothing);
        let muted_parent = place(Some(1), 11, None);
        assert_eq!(s.thread_level(&thread(member(0)), &muted_parent, NOW), Level::Nothing, "muted parent");
        assert_eq!(s.thread_level(&thread(member(1 << 2)), &muted_parent, NOW), Level::Mentions, "unless we chose");

        let message = |extra: Value| -> Message {
            let mut m = json!({"id": "1", "channel_id": "30", "guild_id": "1", "author": {"id": "7", "username": "x"},
                               "content": "", "timestamp": "2026-10-26T00:00:00+00:00"});
            m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(m).unwrap()
        };
        let me = Snowflake(5);
        let everyone = message(json!({"mention_everyone": true}));
        assert!(!s.mentions_me(&everyone, me, &[]), "the server suppresses @everyone");
        assert!(s.should_notify_in_thread(&thread(member(0)), &parent, &everyone, me, &[], NOW), "not in threads");
        assert!(!s.should_notify_in_thread(&thread(member(0)), &parent, &message(json!({})), me, &[], NOW));
        let mut muted = member(1 << 1);
        muted["muted"] = json!(true);
        assert!(!s.should_notify_in_thread(&thread(muted), &parent, &everyone, me, &[], NOW), "muted thread");

        // The client's mute check sees the server and the parent channel itself, whatever our thread flags
        // say; a category above the parent is only looked at when we chose nothing for the thread.
        let notify = |flags: u64, parent: &Place| {
            s.should_notify_in_thread(&thread(member(flags)), parent, &everyone, me, &[], NOW)
        };
        assert!(!notify(1 << 1, &place(Some(2), 5, None)), "muted server, thread set to all messages");
        assert!(!notify(1 << 2, &place(Some(2), 5, None)), "muted server, thread set to mentions");
        assert!(!notify(1 << 1, &place(Some(1), 11, None)), "muted parent channel");
        assert!(notify(1 << 1, &place(Some(1), 12, Some(11))), "muted category: not looked at");
        assert!(!notify(0, &place(Some(1), 12, Some(11))), "muted category, nothing chosen");
    }

    #[test]
    fn mention_detection() {
        let s = settings();
        let message = |guild: &str, extra: Value| -> Message {
            let mut m = json!({"id": "1", "channel_id": "10", "guild_id": guild, "author": {"id": "7", "username": "x"},
                               "content": "", "timestamp": "2026-10-26T00:00:00+00:00"});
            m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(m).unwrap()
        };
        let me = Snowflake(5);
        let roles = [Snowflake(40)];
        assert!(s.mentions_me(&message("2", json!({"mentions": [{"id": "5", "username": "me"}]})), me, &roles));
        assert!(s.mentions_me(&message("2", json!({"mention_everyone": true})), me, &roles));
        assert!(!s.mentions_me(&message("1", json!({"mention_everyone": true})), me, &roles), "suppressed");
        assert!(s.mentions_me(&message("1", json!({"mention_roles": ["40"]})), me, &roles));
        assert!(!s.mentions_me(&message("1", json!({"mention_roles": ["41"]})), me, &roles));
    }
}
