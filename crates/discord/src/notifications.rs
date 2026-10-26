//! The user's per-server notification settings (`user_guild_settings`), and
//! the decision Discord's client makes from them: does this message mention
//! me, is this channel muted, should it raise a desktop notification?
//!
//! Levels resolve from the most specific setting: channel override, then its
//! category's override, then the server setting, then the server's default.
//! DM settings live in the entry whose `guild_id` is null.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{Message, Snowflake};

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

#[derive(Clone, Debug, Default, Deserialize)]
struct MuteConfig {
    #[serde(default)]
    end_time: Option<String>,
}

/// Muted with no end time, or an end time still in the future.
fn mute_active(muted: bool, config: Option<&MuteConfig>, now_unix: i64) -> bool {
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
        self.by_guild
            .get(&Some(guild))
            .is_some_and(|s| mute_active(s.muted, s.mute_config.as_ref(), now_unix))
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

    /// Whether `message` pings us: directly, via @everyone/@here, or via one
    /// of our roles, unless the server suppresses that kind.
    pub fn mentions_me(&self, message: &Message, me: Snowflake, my_roles: &[Snowflake]) -> bool {
        let settings = self.by_guild.get(&message.guild_id);
        let suppress_everyone = settings.is_some_and(|s| s.suppress_everyone);
        let suppress_roles = settings.is_some_and(|s| s.suppress_roles);
        message.mentions.iter().any(|u| u.id == me)
            || (message.mention_everyone && !suppress_everyone)
            || (!suppress_roles && message.mention_roles.iter().any(|r| my_roles.contains(r)))
    }

    /// Should a message here raise a desktop notification? DMs notify unless
    /// muted. In servers, a mention gets through a mute (as in Discord), but
    /// not a level of "nothing".
    pub fn should_notify(&self, place: &Place, mentioned: bool, now_unix: i64) -> bool {
        if place.guild.is_none() {
            return !self.channel_muted(place, now_unix);
        }
        match self.level(place) {
            Level::Nothing => false,
            Level::Mentions => mentioned,
            Level::All => mentioned || !self.muted(place, now_unix),
        }
    }
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
        Place { guild: guild.map(Snowflake), guild_default: 0, channel: Snowflake(channel), parent: parent.map(Snowflake) }
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
    fn notify_decisions() {
        let s = settings();
        assert!(s.should_notify(&place(Some(1), 10, None), false, NOW), "level all");
        assert!(!s.should_notify(&place(Some(1), 12, None), false, NOW), "mentions only, not mentioned");
        assert!(s.should_notify(&place(Some(1), 12, None), true, NOW));
        assert!(!s.should_notify(&place(Some(2), 5, None), false, NOW), "muted server");
        assert!(s.should_notify(&place(Some(2), 5, None), true, NOW), "a mention gets through a mute");
        assert!(s.should_notify(&place(None, 98, None), false, NOW), "DM");
        assert!(!s.should_notify(&place(None, 99, None), false, NOW), "muted DM");
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
