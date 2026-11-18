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

use serde::{Deserialize, Serialize};
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

fn yes() -> bool {
    true
}

/// How long a mute lasts.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MuteConfig {
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    selected_time_window: Option<i64>,
}

impl MuteConfig {
    /// When the mute ends, an ISO 8601 time; none for a mute that lasts until
    /// it is turned off.
    pub fn end_time(&self) -> Option<&str> {
        self.end_time.as_deref()
    }
}

/// The caption of an Unmute item (`j7h4AJ` in 145408): "Muted until 11/18,
/// 9:30 PM", the end time in `zone` as the client writes it
/// (`toLocaleString` with a numeric month and day, hour and two-digit
/// minute, in English). A mute with no end time has none.
pub fn muted_until(end_time: Option<&str>, zone: &glib::TimeZone) -> Option<String> {
    let end = glib::DateTime::from_iso8601(end_time?, Some(&glib::TimeZone::utc())).ok()?.to_timezone(zone).ok()?;
    Some(until_text(end.month(), end.day_of_month(), end.hour(), end.minute()))
}

fn until_text(month: i32, day: i32, hour: i32, minute: i32) -> String {
    let (hour12, half) = match hour {
        0 => (12, "AM"),
        1..12 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("Muted until {month}/{day}, {hour12}:{minute:02} {half}")
}

/// Whether something is muted, and until when (an ISO 8601 time) if it ends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mute {
    pub on: bool,
    pub until: Option<String>,
}

/// Fold `patch` into `base` the way the client's settings reducers do:
/// objects merge key by key, and `channel_overrides` (an array here, an
/// object in a patch) merge by channel.
fn merge(base: &mut Value, patch: &Value) {
    let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) else { return };
    for (key, value) in patch {
        if key == "channel_overrides" {
            let overrides = base.entry(key.clone()).or_insert_with(|| Value::Array(Vec::new()));
            let Some(list) = overrides.as_array_mut() else { continue };
            for (channel, change) in value.as_object().into_iter().flatten() {
                let at = list.iter().position(|o| o["channel_id"] == *channel);
                let slot = match at {
                    Some(at) => &mut list[at],
                    None => {
                        list.push(serde_json::json!({"channel_id": channel, "muted": false, "message_notifications": INHERIT}));
                        list.last_mut().expect("just pushed")
                    }
                };
                merge(slot, change);
            }
        } else if let Some(existing) = base.get_mut(key).filter(|e| e.is_object() && value.is_object()) {
            merge(existing, value);
        } else {
            base.insert(key.clone(), value.clone());
        }
    }
}

/// Muted with no end time, or an end time still in the future. The client
/// clears a mute with a timer that fires at the end time (544743), so it
/// ends there; `tG`'s `>=` only matters for a tie in milliseconds.
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ChannelOverride {
    channel_id: Snowflake,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    mute_config: Option<MuteConfig>,
    #[serde(default = "inherit")]
    message_notifications: u8,
    /// `NEW_FORUM_THREADS_ON`/`_OFF` and `OPT_IN_ENABLED` (355097), which a
    /// forum's New Posts switch and a channel's opt-in set.
    #[serde(default)]
    flags: u64,
}

/// One server's settings, or the DM settings when `guild_id` is `None`.
#[derive(Clone, Debug, Deserialize, Serialize)]
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
    #[serde(default = "yes")]
    mobile_push: bool,
    /// `notify_highlights`: 0 unset, 1 disabled (Suppress Highlights on), 2 enabled.
    #[serde(default)]
    notify_highlights: u8,
    #[serde(default)]
    hide_muted_channels: bool,
    /// `OPT_IN_CHANNELS_ON`/`_OFF` (355097).
    #[serde(default)]
    flags: u64,
    #[serde(default)]
    mute_scheduled_events: bool,
    #[serde(default)]
    channel_overrides: Vec<ChannelOverride>,
}

impl GuildSettings {
    /// What a server without an entry has: the defaults.
    fn unset(guild: Option<Snowflake>) -> Self {
        GuildSettings {
            guild_id: guild,
            muted: false,
            mute_config: None,
            message_notifications: INHERIT,
            suppress_everyone: false,
            suppress_roles: false,
            mobile_push: true,
            notify_highlights: 0,
            hide_muted_channels: false,
            flags: 0,
            mute_scheduled_events: false,
            channel_overrides: Vec::new(),
        }
    }

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
    /// The account's own flags (`notification_settings.flags` of READY), which
    /// `NOTIFICATION_SETTINGS_UPDATE` replaces.
    flags: u64,
}

/// What the flags say (`I.i` in 406535).
const USE_NEW_NOTIFICATIONS: u64 = 1 << 4;
const MENTION_ON_ALL_MESSAGES: u64 = 1 << 5;

impl NotificationSettings {
    pub fn set_flags(&mut self, flags: u64) {
        self.flags = flags;
    }

    /// The account uses the newer notification levels, with unread settings
    /// of their own (`useNewNotifications`).
    pub fn use_new_notifications(&self) -> bool {
        self.flags & USE_NEW_NOTIFICATIONS != 0
    }

    pub fn mention_on_all_messages(&self) -> bool {
        self.flags & MENTION_ON_ALL_MESSAGES != 0
    }

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
        NotificationSettings { by_guild, flags: 0 }
    }

    /// What we changed, as the client applies it before Discord's event
    /// arrives (`USER_GUILD_SETTINGS_GUILD_UPDATE`, `_CHANNEL_UPDATE`):
    /// `patch` is the body sent for `guild`, or for the DMs when `None`.
    pub fn apply(&mut self, guild: Option<Snowflake>, patch: &Value) {
        let entry = self.by_guild.entry(guild).or_insert_with(|| GuildSettings::unset(guild));
        let mut merged = serde_json::to_value(&*entry).unwrap_or(Value::Null);
        merge(&mut merged, patch);
        if let Ok(settings) = GuildSettings::deserialize(&merged) {
            *entry = settings;
        }
    }

    /// The server's own raw level: 0 all, 1 mentions, 2 nothing, 3 the default.
    pub fn guild_level_raw(&self, guild: Option<Snowflake>) -> u8 {
        self.by_guild.get(&guild).map_or(INHERIT, |s| s.message_notifications)
    }

    /// A channel's own raw level, 3 when it has none.
    pub fn channel_level_raw(&self, guild: Option<Snowflake>, channel: Snowflake) -> u8 {
        self.by_guild.get(&guild).and_then(|s| s.channel(channel)).map_or(INHERIT, |o| o.message_notifications)
    }

    /// The server, the DMs (`None`) or a channel is muted, and until when.
    pub fn mute_state(&self, guild: Option<Snowflake>, channel: Option<Snowflake>, now_unix: i64) -> Mute {
        let Some(settings) = self.by_guild.get(&guild) else { return Mute::default() };
        let (muted, config) = match channel {
            None => (settings.muted, settings.mute_config.as_ref()),
            Some(id) => match settings.channel(id) {
                Some(o) => (o.muted, o.mute_config.as_ref()),
                None => return Mute::default(),
            },
        };
        Mute { on: mute_active(muted, config, now_unix), until: config.and_then(|c| c.end_time.clone()) }
    }

    pub fn suppress_everyone(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.suppress_everyone)
    }

    pub fn suppress_roles(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.suppress_roles)
    }

    /// Suppress Highlights is on: `notify_highlights` is "disabled".
    pub fn suppress_highlights(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.notify_highlights == 1)
    }

    pub fn mobile_push(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_none_or(|s| s.mobile_push)
    }

    /// Mute New Events is on (`isMuteScheduledEventsEnabled`, 543465).
    pub fn mute_scheduled_events(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.mute_scheduled_events)
    }

    /// Hide Muted Channels is on (`isGuildCollapsed`, 543465).
    pub fn hide_muted_channels(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.hide_muted_channels)
    }

    /// A channel's own mute is running: the set the sidebar hides muted
    /// channels by (`getMutedChannels`), which a muted category isn't part of
    /// for its channels.
    pub fn channel_muted_itself(&self, guild: Snowflake, channel: Snowflake, now_unix: i64) -> bool {
        self.mute_state(Some(guild), Some(channel), now_unix).on
    }

    /// Whether a forum's new posts notify (`getNewForumThreadsCreated`,
    /// 543465): what its New Posts switch set, else whether the forum
    /// notifies for every message.
    pub fn new_forum_posts(&self, place: &Place) -> bool {
        let flags = self.by_guild.get(&place.guild).and_then(|s| s.channel(place.channel)).map_or(0, |o| o.flags);
        if flags & forum_flags::NEW_POSTS_ON != 0 {
            true
        } else if flags & forum_flags::NEW_POSTS_OFF != 0 {
            false
        } else {
            self.level(place) == Level::All
        }
    }

    /// A channel's own override flags, which `message_notifications` saves
    /// alongside (`getChannelIdFlags`).
    pub fn channel_flags(&self, guild: Snowflake, channel: Snowflake) -> u64 {
        self.by_guild.get(&Some(guild)).and_then(|s| s.channel(channel)).map_or(0, |o| o.flags)
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

    /// Anything here is muted: the server (for DMs, their own settings
    /// entry: `isMuted(null)`), the category or the channel.
    pub fn muted(&self, place: &Place, now_unix: i64) -> bool {
        self.by_guild.get(&place.guild).is_some_and(|s| mute_active(s.muted, s.mute_config.as_ref(), now_unix))
            || self.channel_muted(place, now_unix)
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
        let opted_out = self.opted_out(&as_channel) && !self.opted_in(&Place { channel: thread.id, ..as_channel });
        if self.muted(&as_channel, now_unix) || opted_out || self.level(&as_channel) == Level::Nothing {
            return false;
        }
        match self.thread_level(thread, parent, now_unix) {
            Level::Nothing => false,
            Level::Mentions => mentions(message, me, my_roles, false, false),
            Level::All => true,
        }
    }

    /// `resolveUnreadSetting` (543465) of a server channel; a thread or a DM is
    /// always [`All`](UnreadSetting::All) (`Le` and `Gw`, 95701), which callers
    /// settle. Without `useNewNotifications` it is always `All`; with it, the
    /// channel's own setting, its category's, the server's, then whether the
    /// level is "all messages".
    pub fn unread_setting(&self, place: &Place) -> UnreadSetting {
        let Some(guild) = place.guild.filter(|_| self.use_new_notifications()) else { return UnreadSetting::All };
        let from = |flags: u64, all, only| {
            if flags & all != 0 {
                Some(UnreadSetting::All)
            } else if flags & only != 0 {
                Some(UnreadSetting::OnlyMentions)
            } else {
                None
            }
        };
        [Some(place.channel), place.parent]
            .into_iter()
            .flatten()
            .find_map(|id| from(self.channel_flags(guild, id), unread_flags::ALL_MESSAGES, unread_flags::ONLY_MENTIONS))
            .or_else(|| {
                let flags = self.by_guild.get(&place.guild).map_or(0, |s| s.flags);
                from(flags, guild_flags::UNREADS_ALL_MESSAGES, guild_flags::UNREADS_ONLY_MENTIONS)
            })
            .unwrap_or(if self.level(place) == Level::All { UnreadSetting::All } else { UnreadSetting::OnlyMentions })
    }

    /// A message in a channel that `counts_mention` doesn't count still does,
    /// at low importance, when the account asks for `mentionOnAllMessages` and
    /// the channel notifies for all messages (218461): not in a call's chat, and
    /// not in a channel muted itself (its category or server don't count here).
    pub fn low_importance_mention(&self, place: &Place, vocal: bool, now_unix: i64) -> bool {
        self.mention_on_all_messages()
            && !vocal
            && !self.mute_state(place.guild, Some(place.channel), now_unix).on
            && self.level(place) == Level::All
    }

    /// [`low_importance_mention`](Self::low_importance_mention) for a thread,
    /// which asks the thread's own level (`parent` is its channel's place).
    pub fn low_importance_thread_mention(&self, thread: &Channel, parent: &Place, now_unix: i64) -> bool {
        self.mention_on_all_messages() && self.thread_level(thread, parent, now_unix) == Level::All
    }

    /// The server has opt-in channels on (`isOptInEnabled`, 543465).
    pub fn opt_in_enabled(&self, guild: Snowflake) -> bool {
        self.by_guild.get(&Some(guild)).is_some_and(|s| s.flags & guild_flags::OPT_IN_CHANNELS_ON != 0)
    }

    fn channel_opted_in(&self, guild: Snowflake, channel: Snowflake) -> bool {
        self.channel_flags(guild, channel) & forum_flags::OPT_IN_ENABLED != 0
    }

    /// The channel or its parent (its category, or a thread's channel) is
    /// opted in (`isChannelRecordOrParentOptedIn`, 543465).
    pub fn opted_in(&self, place: &Place) -> bool {
        place.guild.is_some_and(|g| {
            [Some(place.channel), place.parent].into_iter().flatten().any(|c| self.channel_opted_in(g, c))
        })
    }

    /// `allowNoMessages`' opt-in clause: the server has opt-in channels on
    /// and this isn't one, which silences it whatever else is set.
    fn opted_out(&self, place: &Place) -> bool {
        place.guild.is_some_and(|g| self.opt_in_enabled(g)) && !self.opted_in(place)
    }

    /// What a channel adds to its server's badge when the server has opt-in
    /// channels on, as `getGuildChannelUnreadState` (218461) and the
    /// server's tally (458294) decide: nothing when its last message is
    /// over a week old, mentions included, and without a mention only a
    /// channel opted in. (The client also lets a channel we recently read
    /// through here, but that tally leaves it out again.)
    pub fn opt_in_badge(
        &self,
        place: &Place,
        last_message: Option<Snowflake>,
        unread: bool,
        mentions: u32,
        now_ms: i64,
    ) -> (bool, u32) {
        let fresh = last_message.is_some_and(|m| m.unix_ms() >= now_ms - STALE_MS);
        if !fresh || (mentions == 0 && !self.opted_in(place)) {
            return (false, 0);
        }
        (unread, mentions)
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
            && (pinged || place.is_some_and(|p| p.guild.is_none() && !self.muted(p, now_unix)))
    }

    /// Should a message here raise a desktop notification? `allowNoMessages`
    /// (543465) comes first: opt-in channels on and this one not opted in, a mute on the server (for DMs, their settings
    /// entry), category or channel, or a level of "nothing", silences
    /// everything, mentions included. Then `mentioned` (`mentions_me`, so
    /// what the server suppresses is already out) decides under "mentions";
    /// a DM is no mention for that, so a DM level of "mentions" needs a ping.
    pub fn should_notify(&self, place: &Place, mentioned: bool, now_unix: i64) -> bool {
        self.notify(place, mentioned, false, now_unix)
    }

    /// [`should_notify`](Self::should_notify) for the chat of a voice or stage
    /// channel. The client lets level "all" notify there only while connected
    /// to that channel (`RTCConnectionStore`'s, in 411238): anything else
    /// needs a mention, as under "mentions". Neucord never joins a call, so
    /// "all" never applies.
    pub fn should_notify_in_vocal(&self, place: &Place, mentioned: bool, now_unix: i64) -> bool {
        self.notify(place, mentioned, true, now_unix)
    }

    fn notify(&self, place: &Place, mentioned: bool, vocal: bool, now_unix: i64) -> bool {
        if self.muted(place, now_unix) || self.opted_out(place) {
            return false;
        }
        match self.level(place) {
            Level::Nothing => false,
            Level::Mentions => mentioned,
            Level::All => mentioned || !vocal,
        }
    }
}

/// Messages the client neither counts nor notifies (218461, 411238): from
/// an author `blocked` names (blocked or ignored people), answering a command
/// one ran, or a server space's leaderboard naming one
/// (`isBlockedOrIgnoredForMessage`), and a group DM's notice that someone
/// left.
pub fn is_dropped(message: &Message, group_dm: bool, blocked: impl Fn(Snowflake) -> bool) -> bool {
    (group_dm && message.kind == RECIPIENT_REMOVE) || message.is_blocked_by(blocked)
}

const RECIPIENT_REMOVE: u8 = 2;

/// Whether `message` mentions `me` or one of `my_roles`, unless the kind is suppressed.
pub fn mentions(message: &Message, me: Snowflake, my_roles: &[Snowflake], no_everyone: bool, no_roles: bool) -> bool {
    message.mentions.iter().any(|u| u.id == me)
        || (message.mention_everyone && !no_everyone)
        || (!no_roles && message.mention_roles.iter().any(|r| my_roles.contains(r)))
}

/// Our thread member's flags: which messages in the thread notify.
pub mod thread_flags {
    pub const ALL_MESSAGES: u64 = 1 << 1;
    pub const ONLY_MENTIONS: u64 = 1 << 2;
    pub const NO_MESSAGES: u64 = 1 << 3;
}

/// A channel override's flags (355097's `NEW_FORUM_THREADS_ON` and `_OFF`).
pub mod forum_flags {
    pub const NEW_POSTS_ON: u64 = 1 << 14;
    pub const NEW_POSTS_OFF: u64 = 1 << 13;
    /// `OPT_IN_ENABLED`: the channel is opted in.
    pub const OPT_IN_ENABLED: u64 = 1 << 12;
}

/// A server entry's flags (355097's `OPT_IN_CHANNELS_ON`, `UNREADS_*`).
pub mod guild_flags {
    pub const OPT_IN_CHANNELS_ON: u64 = 1 << 14;
    pub const UNREADS_ONLY_MENTIONS: u64 = 1 << 12;
    pub const UNREADS_ALL_MESSAGES: u64 = 1 << 11;
}

/// What a channel override's flags say of its unreads (355097's `vv`).
mod unread_flags {
    pub const ONLY_MENTIONS: u64 = 1 << 9;
    pub const ALL_MESSAGES: u64 = 1 << 10;
}

/// Whether a channel's unread messages count, besides its mentions
/// (`resolveUnreadSetting`, 543465).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadSetting {
    All,
    OnlyMentions,
}

/// A channel with no message in this long drops out of an opt-in server's
/// badge (`eC`, 218461).
const STALE_MS: i64 = 7 * 86_400_000;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn an_unmute_caption_says_when_the_mute_ends_in_the_viewers_time() {
        let utc = glib::TimeZone::utc();
        let east = glib::TimeZone::from_offset(2 * 3600);
        let at = Some("2026-11-18T21:30:00.250Z");
        assert_eq!(muted_until(at, &utc).as_deref(), Some("Muted until 11/18, 9:30 PM"));
        assert_eq!(muted_until(at, &east).as_deref(), Some("Muted until 11/18, 11:30 PM"));
        // Across midnight the day moves with the zone.
        assert_eq!(muted_until(Some("2026-11-18T23:05:00Z"), &east).as_deref(), Some("Muted until 11/19, 1:05 AM"));
    }

    #[test]
    fn the_caption_counts_hours_on_a_twelve_hour_clock_with_two_digit_minutes() {
        let text = |h| until_text(1, 5, h, 7);
        assert_eq!(text(0), "Muted until 1/5, 12:07 AM");
        assert_eq!(text(9), "Muted until 1/5, 9:07 AM");
        assert_eq!(text(12), "Muted until 1/5, 12:07 PM");
        assert_eq!(text(23), "Muted until 1/5, 11:07 PM");
    }

    #[test]
    fn a_mute_with_no_end_has_no_caption() {
        assert_eq!(muted_until(None, &glib::TimeZone::utc()), None);
        assert_eq!(muted_until(Some("not a time"), &glib::TimeZone::utc()), None);
    }

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
    fn a_change_applies_before_discords_event_comes() {
        let mut s = settings();
        let (guild, other) = (Some(Snowflake(1)), Some(Snowflake(9)));
        s.apply(guild, &json!({"message_notifications": 2, "suppress_roles": true}));
        assert_eq!(s.guild_level_raw(guild), 2);
        assert!(s.suppress_roles(Snowflake(1)) && s.suppress_everyone(Snowflake(1)), "the rest is kept");
        assert_eq!(s.channel_level_raw(guild, Snowflake(10)), 0, "channel overrides are kept");

        // A channel's override is changed in place, or made.
        s.apply(guild, &json!({"channel_overrides": {"10": {"message_notifications": 1}}}));
        assert_eq!(s.channel_level_raw(guild, Snowflake(10)), 1);
        s.apply(guild, &json!({"channel_overrides": {"30": {"muted": true, "mute_config": {"selected_time_window": -1, "end_time": null}}}}));
        assert!(s.mute_state(guild, Some(Snowflake(30)), NOW).on);
        assert_eq!(s.channel_level_raw(guild, Snowflake(30)), 3, "a new override inherits");

        // A server with no entry gets one with the defaults.
        assert_eq!(s.guild_level_raw(other), 3);
        assert!(s.mobile_push(Snowflake(9)));
        let end = "2999-01-01T00:00:00.000Z";
        s.apply(other, &json!({"muted": true, "mute_config": {"selected_time_window": 900, "end_time": end}}));
        assert_eq!(s.mute_state(other, None, NOW), Mute { on: true, until: Some(end.into()) });
        s.apply(other, &json!({"muted": false}));
        assert!(!s.mute_state(other, None, NOW).on);
        s.apply(other, &json!({"notify_highlights": 1, "mobile_push": false}));
        assert!(s.suppress_highlights(Snowflake(9)) && !s.mobile_push(Snowflake(9)));

        // The DMs are the entry with no server.
        s.apply(None, &json!({"channel_overrides": {"98": {"muted": true}}}));
        assert!(s.channel_muted(&place(None, 98, None), NOW));
    }

    #[test]
    fn the_switches_of_a_server_read_and_apply() {
        let mut s = NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "hide_muted_channels": true, "mute_scheduled_events": false,
             "channel_overrides": [{"channel_id": "12", "muted": true, "message_notifications": 3, "flags": 8192}]}
        ]));
        assert!(s.hide_muted_channels(Snowflake(1)) && !s.mute_scheduled_events(Snowflake(1)));
        assert!(!s.hide_muted_channels(Snowflake(2)), "no entry: off");
        s.apply(Some(Snowflake(1)), &json!({"hide_muted_channels": false, "mute_scheduled_events": true}));
        assert!(!s.hide_muted_channels(Snowflake(1)) && s.mute_scheduled_events(Snowflake(1)));
        assert!(s.channel_muted_itself(Snowflake(1), Snowflake(12), NOW));
        assert!(!s.channel_muted_itself(Snowflake(1), Snowflake(13), NOW));
        assert_eq!(s.channel_flags(Snowflake(1), Snowflake(12)), 8192, "kept through a change");
    }

    #[test]
    fn a_forums_new_posts_follow_its_switch_else_its_level() {
        let mut s = NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "message_notifications": 1,
             "channel_overrides": [{"channel_id": "20", "message_notifications": 3, "flags": 16384},
                                   {"channel_id": "21", "message_notifications": 0, "flags": 8192}]}
        ]));
        let forum = |id| place(Some(1), id, None);
        assert!(s.new_forum_posts(&forum(20)), "switched on over a mentions-only server");
        assert!(!s.new_forum_posts(&forum(21)), "switched off over all messages");
        assert!(!s.new_forum_posts(&forum(22)), "no switch: the server's level, mentions");
        s.apply(Some(Snowflake(1)), &json!({"message_notifications": 0}));
        assert!(s.new_forum_posts(&forum(22)), "no switch: all messages");
        s.apply(Some(Snowflake(1)), &json!({"channel_overrides": {"20": {"flags": 8192}}}));
        assert!(!s.new_forum_posts(&forum(20)));
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
    fn a_mute_ends_at_its_end_time() {
        let s = NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "muted": true, "mute_config": {"end_time": "2027-01-15T08:00:00Z"}}
        ]));
        let end = glib::DateTime::from_iso8601("2027-01-15T08:00:00Z", None).unwrap().to_unix();
        assert!(s.guild_muted(Snowflake(1), end - 1));
        assert!(!s.guild_muted(Snowflake(1), end), "the client's timer clears it at the end time");
    }

    #[test]
    fn dms_go_by_their_settings_entry() {
        let dm = place(None, 5, None);
        let entry = |extra: Value| {
            let mut e =
                json!({"guild_id": null, "channel_overrides": [{"channel_id": "6", "message_notifications": 0}]});
            e.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            NotificationSettings::from_ready(&json!([e]))
        };
        let muted = entry(json!({"muted": true, "mute_config": null}));
        assert!(!muted.should_notify(&dm, false, NOW), "all DMs muted");
        assert!(!muted.should_notify(&dm, true, NOW), "mentions too");
        let expired = entry(json!({"muted": true, "mute_config": {"end_time": "2020-01-01T00:00:00Z"}}));
        assert!(expired.should_notify(&dm, false, NOW), "an expired mute");
        let nothing = entry(json!({"message_notifications": 2}));
        assert!(!nothing.should_notify(&dm, true, NOW), "level nothing");
        assert!(nothing.should_notify(&place(None, 6, None), false, NOW), "a DM's own level wins");
        let mentions = entry(json!({"message_notifications": 1}));
        assert!(!mentions.should_notify(&dm, false, NOW), "level mentions");
        assert!(mentions.should_notify(&dm, true, NOW));
        // A mention count follows the same mute.
        let message: Message = serde_json::from_value(json!({"id": "1", "channel_id": "5",
            "author": {"id": "7", "username": "x"}, "content": "", "timestamp": "2026-10-26T00:00:00+00:00"}))
        .unwrap();
        assert!(!muted.counts_mention(Some(&dm), false, &message, false, |_| false, NOW));
        assert!(nothing.counts_mention(Some(&dm), false, &message, false, |_| false, NOW), "a level doesn't");
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
    fn a_voice_or_stage_chat_notifies_on_a_mention_while_we_are_not_connected() {
        let s = settings();
        let notify = |place: &Place, mentioned| s.should_notify_in_vocal(place, mentioned, NOW);
        let all = place(Some(1), 10, None);
        assert!(!notify(&all, false), "level all needs the call");
        assert!(notify(&all, true), "a mention does not");
        let mentions = place(Some(1), 12, None);
        assert!(!notify(&mentions, false) && notify(&mentions, true), "level mentions is as anywhere");
        assert!(!notify(&place(Some(1), 11, None), true), "a mute still silences mentions");
        assert!(!notify(&place(Some(2), 5, None), true), "a muted server too");
        let quiet = NotificationSettings::from_ready(&json!([{"guild_id": "3", "message_notifications": 2}]));
        assert!(!quiet.should_notify_in_vocal(&place(Some(3), 5, None), true, NOW), "level nothing");
        assert!(s.should_notify(&all, false, NOW), "text channels are unchanged");
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

    fn opt_in_settings() -> NotificationSettings {
        NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "flags": 16384, "message_notifications": 0,
             "channel_overrides": [{"channel_id": "10", "flags": 4096},
                                   {"channel_id": "11", "flags": 0},
                                   {"channel_id": "50", "flags": 4096}]},
            {"guild_id": "2", "flags": 8192, "message_notifications": 0},
            {"guild_id": "3", "message_notifications": 0}
        ]))
    }

    #[test]
    fn with_opt_in_on_only_opted_in_channels_notify() {
        let s = opt_in_settings();
        let notify = |p: Place, mentioned| s.should_notify(&p, mentioned, NOW);
        assert!(notify(place(Some(1), 10, None), false), "opted in");
        assert!(!notify(place(Some(1), 11, None), false), "not opted in");
        assert!(!notify(place(Some(1), 11, None), true), "a mention doesn't change that");
        assert!(!notify(place(Some(1), 12, None), true), "no override at all");
        assert!(notify(place(Some(1), 12, Some(50)), false), "its category is opted in");
        assert!(!s.should_notify_in_vocal(&place(Some(1), 11, None), true, NOW));
        assert!(s.should_notify_in_vocal(&place(Some(1), 10, None), true, NOW));
        assert!(notify(place(Some(2), 12, None), false), "opt-in turned off");
        assert!(notify(place(Some(3), 12, None), true), "never turned on");
        assert!(notify(place(None, 12, None), true), "DMs have no opt-in");
        assert!(s.opt_in_enabled(Snowflake(1)) && !s.opt_in_enabled(Snowflake(2)) && !s.opt_in_enabled(Snowflake(4)));
    }

    #[test]
    fn a_thread_notifies_only_where_its_parent_is_opted_in() {
        let s = opt_in_settings();
        let thread = |parent: u64| -> Channel {
            serde_json::from_value(json!({"id": "30", "type": 11, "parent_id": parent.to_string(),
                "member": {"flags": 1 << 1, "muted": false, "join_timestamp": "2026-10-28T00:00:00Z"}}))
            .unwrap()
        };
        let message: Message = serde_json::from_value(json!({
            "id": "1", "channel_id": "30", "guild_id": "1", "author": {"id": "7", "username": "x"},
            "content": "", "timestamp": "2026-10-26T00:00:00+00:00"}))
        .unwrap();
        let me = Snowflake(5);
        let notify = |parent: u64, above: Option<u64>| {
            let parent_place = place(Some(1), parent, above);
            s.should_notify_in_thread(&thread(parent), &parent_place, &message, me, &[], NOW)
        };
        assert!(notify(10, None), "its parent is opted in");
        assert!(!notify(11, None), "its parent isn't");
        assert!(!notify(12, Some(50)), "a category above the parent doesn't count for a thread");
    }

    #[test]
    fn opt_in_keeps_a_servers_badge_to_fresh_and_opted_in_channels() {
        let s = opt_in_settings();
        let now_ms = Snowflake::from_unix_ms(1_800_000_000_000).unix_ms();
        let day = 86_400_000;
        let at = |days_ago: i64| Some(Snowflake::from_unix_ms(now_ms - days_ago * day));
        let badge = |channel: u64, last: Option<Snowflake>, unread, mentions| {
            s.opt_in_badge(&place(Some(1), channel, None), last, unread, mentions, now_ms)
        };
        assert_eq!(badge(10, at(1), true, 2), (true, 2), "opted in and fresh");
        assert_eq!(badge(10, at(8), true, 2), (false, 0), "older than a week: nothing, mentions too");
        assert_eq!(badge(10, None, true, 2), (false, 0), "no message at all");
        assert_eq!(badge(11, at(1), true, 0), (false, 0), "not opted in, no mention");
        assert_eq!(badge(11, at(1), true, 3), (true, 3), "not opted in but mentioned");
        assert_eq!(badge(11, at(9), true, 3), (false, 0), "mentioned, but stale");
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

    /// The account's flags with `useNewNotifications` (bit 4) and
    /// `mentionOnAllMessages` (bit 5) as asked.
    fn with_flags(mut s: NotificationSettings, new_levels: bool, all_messages: bool) -> NotificationSettings {
        s.set_flags(u64::from(new_levels) << 4 | u64::from(all_messages) << 5);
        s
    }

    #[test]
    fn a_channel_that_notifies_for_all_mentions_at_low_importance_when_asked() {
        let at = |s: &NotificationSettings, p: &Place, vocal| s.low_importance_mention(p, vocal, NOW);
        let all = place(Some(1), 10, None);
        assert!(!at(&settings(), &all, false), "not without the flag");
        let s = with_flags(settings(), false, true);
        assert!(at(&s, &all, false));
        assert!(!at(&s, &all, true), "not in a call's chat");
        assert!(!at(&s, &place(Some(1), 12, None), false), "level mentions");
        assert!(!at(&s, &place(Some(1), 11, None), false), "a muted channel");
        // Only the channel's own mute counts, not its server's or its category's.
        assert!(at(&s, &place(Some(2), 10, None), false), "server 2 is muted, with no level of its own");
        let in_muted_category = place(Some(1), 10, Some(11));
        assert!(at(&s, &in_muted_category, false), "a muted category isn't asked");
    }

    #[test]
    fn a_thread_that_notifies_for_all_mentions_at_low_importance_when_asked() {
        let thread = |flags: u64| -> Channel {
            let member = json!({"flags": flags, "muted": false, "join_timestamp": "2026-10-28T00:00:00Z"});
            serde_json::from_value(json!({"id": "30", "type": 11, "parent_id": "10", "member": member})).unwrap()
        };
        let parent = place(Some(1), 10, None);
        let on = with_flags(settings(), false, true);
        assert!(on.low_importance_thread_mention(&thread(0), &parent, NOW), "its parent's level is all");
        assert!(on.low_importance_thread_mention(&thread(1 << 1), &place(Some(1), 12, None), NOW), "we chose all");
        assert!(!on.low_importance_thread_mention(&thread(1 << 2), &parent, NOW), "we chose mentions");
        assert!(!settings().low_importance_thread_mention(&thread(1 << 1), &parent, NOW), "not without the flag");
    }

    #[test]
    fn unread_settings_resolve_from_the_channel_up_and_only_with_new_notifications() {
        let s = NotificationSettings::from_ready(&json!([
            {"guild_id": "1", "message_notifications": 1, "flags": 1 << 11, "channel_overrides": [
                {"channel_id": "10", "message_notifications": 3, "flags": 1 << 9},
                {"channel_id": "50", "message_notifications": 3, "flags": 1 << 10}]},
            {"guild_id": "2", "message_notifications": 1, "flags": 1 << 12, "channel_overrides": []},
            {"guild_id": "3", "message_notifications": 0, "channel_overrides": []},
            {"guild_id": "4", "message_notifications": 1, "channel_overrides": []},
        ]));
        let setting =
            |s: &NotificationSettings, guild, channel, parent| s.unread_setting(&place(Some(guild), channel, parent));
        let old = with_flags(s.clone(), false, false);
        assert_eq!(setting(&old, 4, 5, None), UnreadSetting::All, "without the flag everything counts");
        assert_eq!(setting(&old, 1, 10, None), UnreadSetting::All);

        let s = with_flags(s, true, false);
        assert_eq!(setting(&s, 1, 10, None), UnreadSetting::OnlyMentions, "the channel's own");
        assert_eq!(setting(&s, 1, 11, Some(10)), UnreadSetting::OnlyMentions, "its category's");
        assert_eq!(setting(&s, 1, 50, Some(10)), UnreadSetting::All, "the channel before the category");
        assert_eq!(setting(&s, 1, 12, None), UnreadSetting::All, "the server's, though its level is mentions");
        assert_eq!(setting(&s, 2, 12, None), UnreadSetting::OnlyMentions, "the server's");
        assert_eq!(setting(&s, 3, 12, None), UnreadSetting::All, "no setting: the level is all messages");
        assert_eq!(setting(&s, 4, 12, None), UnreadSetting::OnlyMentions, "no setting: the level is mentions");
        assert_eq!(s.unread_setting(&place(None, 12, None)), UnreadSetting::All, "a DM");
    }
}
