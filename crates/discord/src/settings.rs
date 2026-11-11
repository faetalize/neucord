//! The account's client settings that Neucord honours or edits: server
//! folders (which also define the order of servers in the rail), how media
//! and embeds show, Developer Mode, compact display, the account-wide
//! notification and privacy toggles of the settings window, the status and
//! custom status, and the language.
//!
//! They come twice: as JSON in READY's `user_settings` and
//! USER_SETTINGS_UPDATE, and as protobuf in READY's `user_settings_proto` and
//! USER_SETTINGS_PROTO_UPDATE (see [`crate::settings_proto`]), which is all
//! the official client reads. Both are read as a [`SettingsPatch`], the
//! settings a payload mentions, so either can stand alone.

use serde_json::Value;

use crate::Snowflake;
use crate::emoji::Tone;
use crate::gates::Gates;

/// One entry of `guild_folders`. Entries with no `id` are not real folders:
/// they only place a single server in the order.
#[derive(Clone, Debug, PartialEq)]
pub struct GuildFolder {
    pub id: Option<u64>,
    pub name: Option<String>,
    /// 0xRRGGBB.
    pub color: Option<u32>,
    pub guild_ids: Vec<Snowflake>,
}

/// `guild_folders` from a settings object (READY's `user_settings`, or a
/// USER_SETTINGS_UPDATE). `None` if the object does not mention folders.
pub fn guild_folders(settings: &Value) -> Option<Vec<GuildFolder>> {
    // Ids and colours have been sent both as numbers and as strings.
    let number = |v: &Value| v.as_u64().or_else(|| v.as_str()?.parse().ok());
    let folders = settings.get("guild_folders")?.as_array()?;
    Some(
        folders
            .iter()
            .map(|f| GuildFolder {
                id: number(&f["id"]),
                name: f["name"].as_str().filter(|n| !n.is_empty()).map(String::from),
                color: number(&f["color"]).map(|c| c as u32),
                guild_ids: f["guild_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str()?.parse().ok().map(Snowflake))
                    .collect(),
            })
            .collect(),
    )
}

/// "Animate stickers": which stickers move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StickerAnimation {
    #[default]
    Always,
    /// Still, until the pointer is on them.
    OnInteraction,
    Never,
}

impl StickerAnimation {
    pub fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Always),
            1 => Some(Self::OnInteraction),
            2 => Some(Self::Never),
            _ => None,
        }
    }

    pub fn raw(self) -> u64 {
        self as u64
    }
}

/// The "Chat" settings for embeds, media and reactions. Discord's defaults
/// are all on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaSettings {
    /// Show embeds and link previews at all.
    pub render_embeds: bool,
    /// Show images and videos inside embeds.
    pub inline_embed_media: bool,
    /// Show image attachments inline.
    pub inline_attachment_media: bool,
    /// Play GIFs without hovering them.
    pub gif_auto_play: bool,
    /// Show the reactions under messages.
    pub render_reactions: bool,
    /// Animated custom emoji move.
    pub animate_emoji: bool,
    pub animate_stickers: StickerAnimation,
}

impl Default for MediaSettings {
    fn default() -> Self {
        MediaSettings {
            render_embeds: true,
            inline_embed_media: true,
            inline_attachment_media: true,
            gif_auto_play: true,
            render_reactions: true,
            animate_emoji: true,
            animate_stickers: StickerAnimation::Always,
        }
    }
}

/// The media settings a settings object (READY's `user_settings`, or a
/// partial USER_SETTINGS_UPDATE) mentions; the rest are `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaPatch {
    pub render_embeds: Option<bool>,
    pub inline_embed_media: Option<bool>,
    pub inline_attachment_media: Option<bool>,
    pub gif_auto_play: Option<bool>,
    pub render_reactions: Option<bool>,
    pub animate_emoji: Option<bool>,
    pub animate_stickers: Option<StickerAnimation>,
}

impl MediaPatch {
    pub fn from_settings(settings: &Value) -> Self {
        let flag = |key: &str| settings.get(key).and_then(Value::as_bool);
        MediaPatch {
            render_embeds: flag("render_embeds"),
            inline_embed_media: flag("inline_embed_media"),
            inline_attachment_media: flag("inline_attachment_media"),
            gif_auto_play: flag("gif_auto_play"),
            render_reactions: flag("render_reactions"),
            animate_emoji: flag("animate_emoji"),
            animate_stickers: settings["animate_stickers"].as_u64().and_then(StickerAnimation::from_raw),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl MediaSettings {
    pub fn apply(&mut self, patch: MediaPatch) {
        let set = |slot: &mut bool, value: Option<bool>| *slot = value.unwrap_or(*slot);
        set(&mut self.render_embeds, patch.render_embeds);
        set(&mut self.inline_embed_media, patch.inline_embed_media);
        set(&mut self.inline_attachment_media, patch.inline_attachment_media);
        set(&mut self.gif_auto_play, patch.gif_auto_play);
        set(&mut self.render_reactions, patch.render_reactions);
        set(&mut self.animate_emoji, patch.animate_emoji);
        self.animate_stickers = patch.animate_stickers.unwrap_or(self.animate_stickers);
    }
}

/// Whose reactions to our messages notify us.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReactionNotifications {
    #[default]
    Enabled,
    OnlyDms,
    Disabled,
}

impl ReactionNotifications {
    pub fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Enabled),
            1 => Some(Self::OnlyDms),
            2 => Some(Self::Disabled),
            _ => None,
        }
    }

    pub fn raw(self) -> u64 {
        self as u64
    }
}

/// What Discord notifies the account about, which it decides on its side:
/// the "Notify me when…" toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notifications {
    pub reactions: ReactionNotifications,
    pub friend_online: bool,
    pub friend_anniversary: bool,
    pub profile_updates: bool,
    /// People we know start streaming in small servers.
    pub go_live: bool,
    pub upcoming_event: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            reactions: ReactionNotifications::Enabled,
            friend_online: true,
            friend_anniversary: true,
            profile_updates: true,
            go_live: true,
            upcoming_event: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationsPatch {
    pub reactions: Option<ReactionNotifications>,
    pub friend_online: Option<bool>,
    pub friend_anniversary: Option<bool>,
    pub profile_updates: Option<bool>,
    pub go_live: Option<bool>,
    pub upcoming_event: Option<bool>,
}

impl Notifications {
    fn apply(&mut self, patch: NotificationsPatch) {
        self.reactions = patch.reactions.unwrap_or(self.reactions);
        self.friend_online = patch.friend_online.unwrap_or(self.friend_online);
        self.friend_anniversary = patch.friend_anniversary.unwrap_or(self.friend_anniversary);
        self.profile_updates = patch.profile_updates.unwrap_or(self.profile_updates);
        self.go_live = patch.go_live.unwrap_or(self.go_live);
        self.upcoming_event = patch.upcoming_event.unwrap_or(self.upcoming_event);
    }
}

/// Whether the notification settings offer "A server has an upcoming event":
/// the client shows the toggle for variants 1 to 3 of the user experiment
/// `2026-04-upcoming-server-event` and hides it otherwise (960111).
pub fn upcoming_event_toggle(experiments: &crate::experiments::Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-04-upcoming-server-event").is_some_and(|a| (1..=3).contains(&a.variant))
}

/// How much of the profile others see (`profile_visibility`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileVisibility {
    FriendsOnly,
    FriendsAndSmallServers,
    /// What an account that never chose has: the client reads `UNSET` as this.
    #[default]
    FriendsAndAllServers,
}

impl ProfileVisibility {
    pub const ALL: [ProfileVisibility; 3] =
        [Self::FriendsAndAllServers, Self::FriendsAndSmallServers, Self::FriendsOnly];

    pub fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 | 3 => Some(Self::FriendsAndAllServers),
            2 => Some(Self::FriendsAndSmallServers),
            1 => Some(Self::FriendsOnly),
            _ => None,
        }
    }

    pub fn raw(self) -> u64 {
        match self {
            Self::FriendsOnly => 1,
            Self::FriendsAndSmallServers => 2,
            Self::FriendsAndAllServers => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FriendsAndAllServers => "Friends & All Servers",
            Self::FriendsAndSmallServers => "Friends & Small Servers Only",
            Self::FriendsOnly => "Friends Only",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FriendsAndAllServers => "",
            Self::FriendsAndSmallServers => {
                "Your full profile is visible to friends and any server you join with 200 or fewer members. Everyone else sees a limited version."
            }
            Self::FriendsOnly => "Your full profile is visible to friends. Everyone else sees a limited version.",
        }
    }
}

/// Who may send friend requests, as the bits of `friend_source_flags`
/// (`friendSourceFlags`; the client's default, everyone, is 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FriendSources(pub u32);

impl FriendSources {
    const MUTUAL_FRIENDS: u32 = 2;
    const MUTUAL_GUILDS: u32 = 4;
    const NO_RELATION: u32 = 8;
    const EVERYONE: u32 = 14;

    pub fn everyone(self) -> bool {
        self.0 & Self::EVERYONE == Self::EVERYONE
    }

    /// Friends of friends: all of everyone is also that.
    pub fn mutual_friends(self) -> bool {
        self.everyone() || self.0 & Self::MUTUAL_FRIENDS == Self::MUTUAL_FRIENDS
    }

    pub fn mutual_servers(self) -> bool {
        self.everyone() || self.0 & Self::MUTUAL_GUILDS == Self::MUTUAL_GUILDS
    }

    /// Server members can send requests and nobody else beyond friends of
    /// friends: DMs and friend requests then have one switch.
    pub fn only_server_members(self) -> bool {
        self.0 & Self::MUTUAL_GUILDS == Self::MUTUAL_GUILDS && !self.everyone()
    }

    /// The flags after switching "Everyone" (`14` or `14 & ~NO_RELATION`).
    pub fn with_everyone(on: bool) -> Self {
        FriendSources(if on { Self::EVERYONE } else { Self::EVERYONE & !Self::NO_RELATION })
    }

    /// Switching one of the two: on sets its bit, off clears it and
    /// "no relation" with it.
    pub fn with_mutual_friends(self, on: bool) -> Self {
        self.with(Self::MUTUAL_FRIENDS, on)
    }

    pub fn with_mutual_servers(self, on: bool) -> Self {
        self.with(Self::MUTUAL_GUILDS, on)
    }

    fn with(self, bit: u32, on: bool) -> Self {
        FriendSources(if on { self.0 | bit } else { self.0 & !bit & !Self::NO_RELATION })
    }
}

/// The servers the lists of restrictions name, as the client reads them
/// (`getRestrictedGuildIds` and its kin, 115063): when no server is
/// unavailable, those we are no longer in are dropped.
pub fn live_servers(ids: &[Snowflake], ours: &[Snowflake], unavailable: usize) -> Vec<Snowflake> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().copied().filter(|id| (unavailable != 0 || ours.contains(id)) && seen.insert(*id)).collect()
}

/// `servers` with `server` in it or out of it, in the order a set built from
/// them gives.
pub fn with_server(servers: &[Snowflake], server: Snowflake, listed: bool) -> Vec<Snowflake> {
    let mut servers = servers.to_vec();
    match (listed, servers.contains(&server)) {
        (true, false) => servers.push(server),
        (false, true) => servers.retain(|s| *s != server),
        _ => {}
    }
    servers
}

/// Who may join the games the account plays, who may message and send
/// requests, and how much of the profile is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Privacy {
    pub friends_join_game: bool,
    pub voice_participants_join_game: bool,
    pub friend_sources: FriendSources,
    /// `default_guilds_restricted_v2`: servers' members may not message us
    /// unless the server says so. The older `default_guilds_restricted` is
    /// the answer where this was never set.
    pub servers_restricted: Option<bool>,
    pub servers_restricted_before: bool,
    /// Servers that differ from that default.
    pub restricted_servers: Vec<Snowflake>,
    /// Messages from people we may not know go to requests.
    pub message_requests_restricted: Option<bool>,
    pub message_request_restricted_servers: Vec<Snowflake>,
    pub profile_visibility: ProfileVisibility,
    /// `default_guilds_activity_restricted_v2`; none until set.
    pub activity_default: Option<ActivityDefault>,
    /// Servers the account shares no activity in (`activity_restricted_guild_ids`).
    pub activity_restricted_servers: Vec<Snowflake>,
    /// Servers where others may not join its activity
    /// (`activity_joining_restricted_guild_ids`).
    pub activity_joining_restricted_servers: Vec<Snowflake>,
}

/// `GuildActivityStatusRestrictionDefaultV2`: in which servers the account
/// shares its activity until a server says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityDefault {
    Off,
    LargeServers,
    On,
}

impl ActivityDefault {
    /// `UNSET` is none: the client then takes what the region says.
    pub fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            1 => Some(Self::Off),
            2 => Some(Self::LargeServers),
            3 => Some(Self::On),
            _ => None,
        }
    }

    pub fn raw(self) -> u64 {
        match self {
            Self::Off => 1,
            Self::LargeServers => 2,
            Self::On => 3,
        }
    }

    /// How much is restricted, as the client ranks them (`E`, 365258).
    pub(crate) fn restraint(self) -> u8 {
        match self {
            Self::Off => 2,
            Self::LargeServers => 1,
            Self::On => 0,
        }
    }
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy {
            friends_join_game: true,
            voice_participants_join_game: true,
            friend_sources: FriendSources(FriendSources::EVERYONE),
            servers_restricted: None,
            servers_restricted_before: false,
            restricted_servers: Vec::new(),
            message_requests_restricted: None,
            message_request_restricted_servers: Vec::new(),
            profile_visibility: ProfileVisibility::default(),
            activity_default: None,
            activity_restricted_servers: Vec::new(),
            activity_joining_restricted_servers: Vec::new(),
        }
    }
}

impl Privacy {
    /// Whether members of servers may not message us by default
    /// (`useDefaultRestricted`, 428031).
    pub fn servers_restricted(&self) -> bool {
        self.servers_restricted.unwrap_or(self.servers_restricted_before)
    }

    /// Where activity is shared by default: what was set, else large servers
    /// where the region makes it a teen's default (`isSettingTeenByDefault`),
    /// else nowhere (`253932`).
    pub fn activity_default(&self, gates: &Gates) -> ActivityDefault {
        self.activity_default.unwrap_or(if gates.activity_status_by_default {
            ActivityDefault::LargeServers
        } else {
            ActivityDefault::Off
        })
    }

    /// "Share my activity": on where the server isn't listed.
    pub fn activity_status(&self, server: Snowflake) -> Row {
        Row { on: !self.activity_restricted_servers.contains(&server), disabled: false }
    }

    /// "Activity joining".
    pub fn activity_joining(&self, server: Snowflake) -> Row {
        Row { on: !self.activity_joining_restricted_servers.contains(&server), disabled: false }
    }
}

/// What "Direct Messages" says: its title, and under it where it applies
/// (`vx` and its subtitle in 960111, `b` in 632038). With only server members
/// allowed to send friend requests the one switch is for both. `server` is
/// none for the account's defaults, else whether the server is a hub.
pub fn direct_messages_text(only_server_members: bool, server: Option<bool>) -> (&'static str, &'static str) {
    let title = if only_server_members { "Direct Messages and Friend Requests" } else { "Direct Messages" };
    let note = match (server, only_server_members) {
        (None, true) => "Allow DMs and friend requests from other server members",
        (None, false) => "Allow DMs from other server members",
        (Some(true), _) => "Allow DMs from other members in this hub",
        (Some(false), true) => "Allow DMs and friend requests from other members in this server",
        (Some(false), false) => "Allow DMs from other members in this server",
    };
    (title, note)
}

/// What is under "Message requests" (`vy` and its subtitle in 960111, `y` in
/// 632038), for a hub or not.
pub fn message_requests_note(hub: bool) -> &'static str {
    if hub {
        "Filter messages from hub members you may not know."
    } else {
        "Filter messages from server members you may not know."
    }
}

/// What a switch of the server privacy rows shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub on: bool,
    pub disabled: bool,
}

impl Privacy {
    /// "Direct Messages": on lets a server's members message us. For the
    /// account's default (`server` none) or for one server.
    pub fn direct_messages(&self, gates: &Gates, server: Option<Snowflake>) -> Row {
        match server {
            None => Row { on: !self.servers_restricted(), disabled: gates.parent_linked },
            Some(server) => Row { on: !self.restricted_servers.contains(&server), disabled: false },
        }
    }

    /// "Message requests", on where messages from people we may not know are
    /// filtered into requests. The client turns it on for the accounts it must
    /// (`useDisabled` and `useValue` of the row in 960111): it cannot be
    /// changed where direct messages are off, or by a teen where the region
    /// forces it, or with a parent linked.
    pub fn message_requests(&self, gates: &Gates, server: Option<Snowflake>) -> Row {
        let restricted = self.message_requests_restricted.unwrap_or(false);
        let (value, disabled) = match server {
            None => {
                let direct_messages_off = self.servers_restricted();
                let value = if gates.parent_linked { !restricted } else { !direct_messages_off && !restricted };
                (value, direct_messages_off || gates.parent_linked)
            }
            Some(server) => {
                let off = self.restricted_servers.contains(&server);
                (!off && !self.message_request_restricted_servers.contains(&server), off)
            }
        };
        Row { on: gates.message_requests_forced || value, disabled: disabled || gates.message_requests_forced }
    }
}

/// The privacy settings a payload mentions. The lists and the plain fields
/// come with their whole message, so a message that leaves one out says it
/// is empty or off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrivacyPatch {
    pub friends_join_game: Option<bool>,
    pub voice_participants_join_game: Option<bool>,
    pub friend_sources: Option<FriendSources>,
    pub servers_restricted: Option<bool>,
    pub servers_restricted_before: Option<bool>,
    pub restricted_servers: Option<Vec<Snowflake>>,
    pub message_requests_restricted: Option<bool>,
    pub message_request_restricted_servers: Option<Vec<Snowflake>>,
    pub profile_visibility: Option<ProfileVisibility>,
    /// Inside, none is `UNSET`.
    pub activity_default: Option<Option<ActivityDefault>>,
    pub activity_restricted_servers: Option<Vec<Snowflake>>,
    pub activity_joining_restricted_servers: Option<Vec<Snowflake>>,
}

impl Privacy {
    fn apply(&mut self, patch: PrivacyPatch) {
        self.friends_join_game = patch.friends_join_game.unwrap_or(self.friends_join_game);
        self.voice_participants_join_game =
            patch.voice_participants_join_game.unwrap_or(self.voice_participants_join_game);
        self.friend_sources = patch.friend_sources.unwrap_or(self.friend_sources);
        self.servers_restricted = patch.servers_restricted.or(self.servers_restricted);
        self.servers_restricted_before = patch.servers_restricted_before.unwrap_or(self.servers_restricted_before);
        if let Some(servers) = patch.restricted_servers {
            self.restricted_servers = servers;
        }
        self.message_requests_restricted = patch.message_requests_restricted.or(self.message_requests_restricted);
        if let Some(servers) = patch.message_request_restricted_servers {
            self.message_request_restricted_servers = servers;
        }
        self.profile_visibility = patch.profile_visibility.unwrap_or(self.profile_visibility);
        if let Some(default) = patch.activity_default {
            self.activity_default = default;
        }
        if let Some(servers) = patch.activity_restricted_servers {
            self.activity_restricted_servers = servers;
        }
        if let Some(servers) = patch.activity_joining_restricted_servers {
            self.activity_joining_restricted_servers = servers;
        }
    }
}

/// What the account shows as: the `status` setting. Others see Idle too when
/// the device is idle, which Neucord does not tell apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

impl UserStatus {
    pub const ALL: [UserStatus; 4] = [Self::Online, Self::Idle, Self::DoNotDisturb, Self::Invisible];

    /// The string the setting and the gateway's presence update hold.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::DoNotDisturb => "dnd",
            Self::Invisible => "invisible",
        }
    }

    /// The status a setting's string stands for. `unknown`, which the
    /// client identifies with and an account that never set a status has,
    /// is Online, as in `SelfPresenceStore` (894768); so is anything the
    /// client's picker cannot set.
    pub fn from_wire(wire: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.wire() == wire).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Idle => "Idle",
            Self::DoNotDisturb => "Do Not Disturb",
            Self::Invisible => "Invisible",
        }
    }
}

/// `CustomStatus`: the text and emoji beside the name. Times are
/// milliseconds since 1970; an expiry of 0 is never.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomStatus {
    pub text: String,
    /// A custom emoji, else the Unicode one in `emoji_name`.
    pub emoji_id: Option<Snowflake>,
    pub emoji_name: String,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
}

impl CustomStatus {
    /// Nothing to show: the client then clears the setting instead of
    /// saving it.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.emoji_id.is_none() && self.emoji_name.is_empty()
    }
}

/// The `status` message, which the client saves whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusSettings {
    pub status: UserStatus,
    /// When a status that is not Online goes back to it, 0 for never.
    pub expires_at_ms: u64,
    pub created_at_ms: Option<u64>,
    pub custom: Option<CustomStatus>,
}

/// How long a custom status stays, as the dialog offers it (431273).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearAfter {
    /// A day from now, to the same time tomorrow.
    Today,
    Hours4,
    Hour1,
    Minutes30,
    DontClear,
}

impl ClearAfter {
    /// In the order the dialog lists them.
    pub const ALL: [ClearAfter; 5] = [Self::Today, Self::Hours4, Self::Hour1, Self::Minutes30, Self::DontClear];

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Hours4 => "4 hours",
            Self::Hour1 => "1 hour",
            Self::Minutes30 => "30 minutes",
            Self::DontClear => "Don't clear",
        }
    }

    /// The `expires_at_ms` to save, 0 for never. `today_ms` is how long it is
    /// until this time tomorrow, which a day with a clock change makes
    /// something other than 24 hours.
    pub fn expires_at_ms(self, now_ms: u64, today_ms: u64) -> u64 {
        const MINUTE: u64 = 60_000;
        now_ms
            + match self {
                Self::Today => today_ms,
                Self::Hours4 => 240 * MINUTE,
                Self::Hour1 => 60 * MINUTE,
                Self::Minutes30 => 30 * MINUTE,
                Self::DontClear => return 0,
            }
    }

    /// What the dialog starts on, given the expiry of the custom status the
    /// account has (`None` if it has none): never stays never, a status
    /// ending today starts on the shortest of 30 minutes, an hour and four
    /// hours that still covers it, and anything else on Today. `same_day`
    /// says whether the expiry is on today's date.
    pub fn initial(expires_at_ms: Option<u64>, now_ms: u64, same_day: bool) -> Self {
        let Some(at) = expires_at_ms else { return Self::Today };
        if at == 0 {
            return Self::DontClear;
        }
        let left = at as i64 - now_ms as i64;
        match same_day {
            true => [Self::Minutes30, Self::Hour1, Self::Hours4]
                .into_iter()
                .find(|choice| left <= choice.expires_at_ms(0, 0) as i64)
                .unwrap_or(Self::Today),
            false => Self::Today,
        }
    }
}

/// When the status and the custom status end, as the client's custom status
/// manager works it out (340137): how long from now, nothing for never, and
/// no time at all for one that has ended already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Expiries {
    /// Until the status goes back to Online. Online has no end.
    pub status: Option<std::time::Duration>,
    /// Until the custom status is cleared.
    pub custom: Option<std::time::Duration>,
}

impl StatusSettings {
    pub fn expiries(&self, now_ms: u64) -> Expiries {
        let until = |at: u64| (at != 0).then(|| std::time::Duration::from_millis(at.saturating_sub(now_ms)));
        Expiries {
            status: until(self.expires_at_ms).filter(|_| self.status != UserStatus::Online),
            custom: self.custom.as_ref().and_then(|c| until(c.expires_at_ms)),
        }
    }
}

/// Everything in this module, as the account has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// Empty if the user never arranged servers.
    pub guild_folders: Vec<GuildFolder>,
    pub media: MediaSettings,
    /// Menus then offer to copy ids.
    pub developer_mode: bool,
    /// Neucord has no compact layout, but the official client sizes its
    /// history pages by this.
    pub message_display_compact: bool,
    pub notifications: Notifications,
    pub privacy: Privacy,
    pub status: StatusSettings,
    /// The language the account is set to (`localization.locale`); `None`
    /// until someone picks one.
    pub locale: Option<String>,
    /// The skin tone emoji with tones show and send in; none is the default
    /// yellow.
    pub skin_tone: Option<Tone>,
}

/// The settings a payload mentions; the rest are `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsPatch {
    pub guild_folders: Option<Vec<GuildFolder>>,
    pub media: MediaPatch,
    pub developer_mode: Option<bool>,
    pub message_display_compact: Option<bool>,
    pub notifications: NotificationsPatch,
    pub privacy: PrivacyPatch,
    /// The whole `status` message, as it is saved whole.
    pub status: Option<StatusSettings>,
    /// The `localization` message: its locale, if it has one.
    pub localization: Option<Option<String>>,
    /// `diversity_surrogate`: the tone, or none for the default.
    pub skin_tone: Option<Option<Tone>>,
}

impl SettingsPatch {
    pub fn from_json(settings: &Value) -> Self {
        SettingsPatch {
            guild_folders: guild_folders(settings),
            media: MediaPatch::from_settings(settings),
            developer_mode: settings.get("developer_mode").and_then(Value::as_bool),
            message_display_compact: settings.get("message_display_compact").and_then(Value::as_bool),
            localization: settings.get("locale").and_then(Value::as_str).map(|locale| Some(locale.to_owned())),
            // Only the protobuf carries the rest.
            ..SettingsPatch::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl Settings {
    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(folders) = patch.guild_folders {
            self.guild_folders = folders;
        }
        self.media.apply(patch.media);
        self.developer_mode = patch.developer_mode.unwrap_or(self.developer_mode);
        self.message_display_compact = patch.message_display_compact.unwrap_or(self.message_display_compact);
        self.notifications.apply(patch.notifications);
        self.privacy.apply(patch.privacy);
        if let Some(status) = patch.status {
            self.status = status;
        }
        if let Some(locale) = patch.localization {
            self.locale = locale;
        }
        if let Some(tone) = patch.skin_tone {
            self.skin_tone = tone;
        }
    }
}

/// What the rail shows, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum RailEntry {
    Guild(Snowflake),
    Folder { id: u64, name: Option<String>, color: Option<u32>, guilds: Vec<Snowflake> },
}

/// Order `guilds` by the user's folders, as Discord does: servers the folders
/// don't mention (recent joins) first, then the folders in order. Folders
/// drop servers we are no longer in, and vanish if none are left.
pub fn rail_layout(folders: &[GuildFolder], guilds: &[Snowflake]) -> Vec<RailEntry> {
    let placed: std::collections::HashSet<Snowflake> =
        folders.iter().flat_map(|f| f.guild_ids.iter().copied()).collect();
    let mut entries: Vec<RailEntry> =
        guilds.iter().filter(|g| !placed.contains(g)).map(|g| RailEntry::Guild(*g)).collect();
    for folder in folders {
        let present: Vec<Snowflake> = folder.guild_ids.iter().copied().filter(|g| guilds.contains(g)).collect();
        match (folder.id, present.as_slice()) {
            (_, []) => {}
            (None, members) => entries.extend(members.iter().map(|g| RailEntry::Guild(*g))),
            (Some(id), _) => {
                entries.push(RailEntry::Folder { id, name: folder.name.clone(), color: folder.color, guilds: present })
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(v: &[u64]) -> Vec<Snowflake> {
        v.iter().copied().map(Snowflake).collect()
    }

    #[test]
    fn parses_folders() {
        let folders = guild_folders(&json!({"guild_folders": [
            {"id": null, "name": null, "color": null, "guild_ids": ["1"]},
            {"id": 77, "name": "Friends", "color": 5793266, "guild_ids": ["2", "3"]},
            {"id": "78", "name": "", "guild_ids": ["4"]}
        ]}))
        .unwrap();
        assert_eq!(folders[0], GuildFolder { id: None, name: None, color: None, guild_ids: ids(&[1]) });
        assert_eq!(
            (folders[1].id, folders[1].name.as_deref(), folders[1].color),
            (Some(77), Some("Friends"), Some(0x5865F2))
        );
        assert_eq!((folders[2].id, folders[2].name.as_deref()), (Some(78), None));
        assert!(guild_folders(&json!({"theme": "dark"})).is_none(), "update without folders");
    }

    #[test]
    fn layout_orders_like_discord() {
        let folders = vec![
            GuildFolder { id: None, name: None, color: None, guild_ids: ids(&[3]) },
            GuildFolder { id: Some(9), name: Some("F".into()), color: None, guild_ids: ids(&[1, 4, 99]) },
            GuildFolder { id: Some(10), name: None, color: None, guild_ids: ids(&[98]) },
            GuildFolder { id: None, name: None, color: None, guild_ids: ids(&[2]) },
        ];
        let layout = rail_layout(&folders, &ids(&[1, 2, 3, 4, 5]));
        assert_eq!(
            layout,
            vec![
                RailEntry::Guild(Snowflake(5)),
                RailEntry::Guild(Snowflake(3)),
                RailEntry::Folder { id: 9, name: Some("F".into()), color: None, guilds: ids(&[1, 4]) },
                RailEntry::Guild(Snowflake(2)),
            ]
        );
    }

    #[test]
    fn media_settings_patch() {
        let mut media = MediaSettings::default();
        let patch = MediaPatch::from_settings(&json!({"gif_auto_play": false, "theme": "dark"}));
        assert!(!patch.is_empty());
        media.apply(patch);
        assert_eq!(media, MediaSettings { gif_auto_play: false, ..MediaSettings::default() });
        media.apply(MediaPatch::from_settings(&json!({"render_reactions": false})));
        assert!(!media.render_reactions && !media.gif_auto_play, "each setting alone");
        assert!(MediaPatch::from_settings(&json!({"guild_folders": []})).is_empty());
    }

    #[test]
    fn json_reads_animation_settings() {
        let patch = MediaPatch::from_settings(&json!({"animate_emoji": false, "animate_stickers": 2}));
        assert_eq!(patch.animate_emoji, Some(false));
        assert_eq!(patch.animate_stickers, Some(StickerAnimation::Never));
        let patch = MediaPatch::from_settings(&json!({"animate_stickers": 9}));
        assert_eq!(patch.animate_stickers, None, "a value the client does not know");
    }

    #[test]
    fn statuses_end_when_they_say() {
        use std::time::Duration;
        let custom = |expires_at_ms| {
            Some(CustomStatus { text: "brb".into(), expires_at_ms, created_at_ms: 1, ..CustomStatus::default() })
        };
        let status =
            |status, expires_at_ms, custom| StatusSettings { status, expires_at_ms, created_at_ms: None, custom };

        let later = status(UserStatus::Idle, 5_000, custom(9_000)).expiries(1_000);
        assert_eq!(later, Expiries { status: Some(Duration::from_secs(4)), custom: Some(Duration::from_secs(8)) });

        // Already over: now, not never.
        let over = status(UserStatus::DoNotDisturb, 500, custom(900)).expiries(1_000);
        assert_eq!(over, Expiries { status: Some(Duration::ZERO), custom: Some(Duration::ZERO) });

        // Never is 0, and Online ends in nothing whatever the field holds.
        let never = status(UserStatus::Idle, 0, custom(0)).expiries(1_000);
        assert_eq!(never, Expiries::default());
        assert_eq!(status(UserStatus::Online, 5_000, None).expiries(1_000), Expiries::default());
    }

    #[test]
    fn custom_status_clears_after_what_was_picked() {
        let (now, tomorrow) = (1_000_000u64, 86_400_000u64);
        let ends = |choice: ClearAfter| choice.expires_at_ms(now, tomorrow);
        assert_eq!(ends(ClearAfter::Today), now + tomorrow);
        assert_eq!(ends(ClearAfter::Hours4), now + 14_400_000);
        assert_eq!(ends(ClearAfter::Minutes30), now + 1_800_000);
        assert_eq!(ends(ClearAfter::DontClear), 0);
        let labels: Vec<_> = ClearAfter::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels, ["Today", "4 hours", "1 hour", "30 minutes", "Don't clear"]);
    }

    #[test]
    fn the_dialog_starts_on_what_covers_the_current_expiry() {
        let now = 10_000_000u64;
        let initial = |at, same_day| ClearAfter::initial(at, now, same_day);
        assert_eq!(initial(None, true), ClearAfter::Today, "no custom status");
        assert_eq!(initial(Some(0), true), ClearAfter::DontClear);
        assert_eq!(initial(Some(now + 600_000), true), ClearAfter::Minutes30);
        assert_eq!(initial(Some(now + 3_000_000), true), ClearAfter::Hour1);
        assert_eq!(initial(Some(now + 7_200_000), true), ClearAfter::Hours4);
        assert_eq!(initial(Some(now + 20_000_000), true), ClearAfter::Today, "more than four hours");
        assert_eq!(initial(Some(now + 600_000), false), ClearAfter::Today, "ends on another date");
        assert_eq!(initial(Some(now - 5), true), ClearAfter::Minutes30, "already over");
    }

    #[test]
    fn the_upcoming_event_toggle_is_for_the_variants_that_show_it() {
        let assign = |variant: i64| {
            crate::experiments::Experiments::from_ready(&json!({ "apex_experiments": { "assignments": { "1": { "10": {
                "assignments": [[crate::experiments::murmur3("2026-04-upcoming-server-event"), variant, 0, 1]]
            } } } } }))
        };
        let me = Snowflake(10);
        assert!(!upcoming_event_toggle(&assign(0), me), "the control group");
        assert!([1, 2, 3].into_iter().all(|v| upcoming_event_toggle(&assign(v), me)));
        assert!(!upcoming_event_toggle(&assign(4), me));
        assert!(!upcoming_event_toggle(&Default::default(), me), "not assigned: the default config hides it");
    }

    #[test]
    fn the_server_privacy_switches_show_what_the_client_does() {
        let ids = |v: &[u64]| v.iter().copied().map(Snowflake).collect::<Vec<_>>();
        let free = Gates::default();
        let mut privacy = Privacy::default();

        // A new account: everything allowed, nothing disabled.
        assert_eq!(privacy.direct_messages(&free, None), Row { on: true, disabled: false });
        assert_eq!(privacy.message_requests(&free, None), Row { on: true, disabled: false });

        // Messages restricted by default: DMs off, and message requests off with them and not to change.
        privacy.servers_restricted = Some(true);
        assert_eq!(privacy.direct_messages(&free, None), Row { on: false, disabled: false });
        assert_eq!(privacy.message_requests(&free, None), Row { on: false, disabled: true });
        // The older setting answers where the new was never set, which the new overrides.
        privacy.servers_restricted = None;
        privacy.servers_restricted_before = true;
        assert!(!privacy.direct_messages(&free, None).on);
        privacy.servers_restricted = Some(false);
        assert!(privacy.direct_messages(&free, None).on);

        // Message requests set off for everyone.
        privacy.message_requests_restricted = Some(true);
        assert_eq!(privacy.message_requests(&free, None), Row { on: false, disabled: false });

        // A parent holds DMs and shows message requests by their own setting.
        let parent = Gates { parent_linked: true, ..Gates::default() };
        assert_eq!(privacy.direct_messages(&parent, None), Row { on: true, disabled: true });
        assert_eq!(privacy.message_requests(&parent, None), Row { on: false, disabled: true });

        // A teen's region forces message requests on.
        let forced = Gates { message_requests_forced: true, ..Gates::default() };
        assert_eq!(privacy.message_requests(&forced, None), Row { on: true, disabled: true });

        // One server: its own lists.
        let mut privacy = Privacy {
            restricted_servers: ids(&[5]),
            message_request_restricted_servers: ids(&[6]),
            ..Privacy::default()
        };
        let one = |server, p: &Privacy| {
            (p.direct_messages(&free, Some(Snowflake(server))), p.message_requests(&free, Some(Snowflake(server))))
        };
        assert_eq!(one(5, &privacy), (Row { on: false, disabled: false }, Row { on: false, disabled: true }));
        assert_eq!(one(6, &privacy), (Row { on: true, disabled: false }, Row { on: false, disabled: false }));
        assert_eq!(one(7, &privacy), (Row { on: true, disabled: false }, Row { on: true, disabled: false }));
        privacy.message_requests_restricted = Some(true);
        assert!(one(7, &privacy).1.on, "a server's row ignores the default");
    }

    #[test]
    fn the_direct_messages_row_says_what_the_client_says() {
        let text = |only, server| direct_messages_text(only, server);
        assert_eq!(text(false, None), ("Direct Messages", "Allow DMs from other server members"));
        assert_eq!(
            text(true, None),
            ("Direct Messages and Friend Requests", "Allow DMs and friend requests from other server members")
        );
        // A server's row takes the title of the combined switch too.
        assert_eq!(
            text(true, Some(false)),
            ("Direct Messages and Friend Requests", "Allow DMs and friend requests from other members in this server")
        );
        assert_eq!(text(false, Some(false)).1, "Allow DMs from other members in this server");
        assert_eq!(text(true, Some(true)).1, "Allow DMs from other members in this hub", "a hub says so over the rest");
        assert_eq!(message_requests_note(true), "Filter messages from hub members you may not know.");
    }

    #[test]
    fn the_activity_rows_follow_their_lists_and_the_default_follows_the_region() {
        let privacy = Privacy {
            activity_restricted_servers: ids(&[5]),
            activity_joining_restricted_servers: ids(&[6]),
            ..Privacy::default()
        };
        assert_eq!(privacy.activity_status(Snowflake(5)), Row { on: false, disabled: false });
        assert_eq!(privacy.activity_status(Snowflake(6)), Row { on: true, disabled: false });
        assert_eq!(privacy.activity_joining(Snowflake(6)), Row { on: false, disabled: false });
        assert_eq!(privacy.activity_joining(Snowflake(5)), Row { on: true, disabled: false });

        let teen = Gates { activity_status_by_default: true, ..Gates::default() };
        assert_eq!(privacy.activity_default(&Gates::default()), ActivityDefault::Off);
        assert_eq!(privacy.activity_default(&teen), ActivityDefault::LargeServers);
        let chosen = Privacy { activity_default: Some(ActivityDefault::On), ..Privacy::default() };
        assert_eq!(chosen.activity_default(&teen), ActivityDefault::On, "what was set wins");
    }

    #[test]
    fn friend_requests_switch_like_the_client_does() {
        let all = FriendSources(14);
        assert!(all.everyone() && all.mutual_friends() && all.mutual_servers(), "everyone is all three");
        // Everyone off keeps both of the others.
        let rest = FriendSources::with_everyone(false);
        assert_eq!(rest, FriendSources(6));
        assert!(!rest.everyone() && rest.mutual_friends() && rest.mutual_servers());
        assert_eq!(FriendSources::with_everyone(true), all);
        // Turning one of the others off takes "no relation" with it, so everyone goes too.
        assert_eq!(all.with_mutual_friends(false), FriendSources(4));
        assert_eq!(all.with_mutual_servers(false), FriendSources(2));
        assert_eq!(FriendSources(4).with_mutual_friends(true), FriendSources(6));
        assert_eq!(FriendSources(0).with_mutual_servers(true), FriendSources(4));
        assert!(FriendSources(4).only_server_members());
        assert!(!all.only_server_members() && !FriendSources(2).only_server_members());
    }

    #[test]
    fn the_servers_a_restriction_names_are_ones_we_are_in() {
        let ids = |v: &[u64]| v.iter().copied().map(Snowflake).collect::<Vec<_>>();
        assert_eq!(live_servers(&ids(&[1, 2, 2, 9]), &ids(&[1, 2, 3]), 0), ids(&[1, 2]), "9 is a server we left");
        assert_eq!(live_servers(&ids(&[1, 9]), &ids(&[1]), 1), ids(&[1, 9]), "an unavailable server may be 9");
        assert_eq!(with_server(&ids(&[1, 2]), Snowflake(3), true), ids(&[1, 2, 3]));
        assert_eq!(with_server(&ids(&[1, 2, 3]), Snowflake(2), false), ids(&[1, 3]));
        assert_eq!(with_server(&ids(&[1, 2, 3]), Snowflake(2), true), ids(&[1, 2, 3]), "a set holds it once, in place");
    }

    #[test]
    fn json_patch_holds_what_is_mentioned() {
        let patch = SettingsPatch::from_json(&json!({"developer_mode": true, "theme": "dark"}));
        assert_eq!(patch, SettingsPatch { developer_mode: Some(true), ..SettingsPatch::default() });
        let patch = SettingsPatch::from_json(&json!({"locale": "fr", "theme": "dark"}));
        assert_eq!(patch.localization, Some(Some("fr".to_owned())), "READY's JSON names the language too");
        let patch = SettingsPatch::from_json(&json!({"message_display_compact": false, "gif_auto_play": false}));
        assert_eq!(patch.message_display_compact, Some(false));
        assert_eq!(patch.media.gif_auto_play, Some(false));
        assert!(SettingsPatch::from_json(&json!({"theme": "dark"})).is_empty());
    }

    #[test]
    fn a_patch_changes_only_what_it_mentions() {
        let mut settings = Settings { developer_mode: true, ..Settings::default() };
        settings.apply(SettingsPatch { message_display_compact: Some(true), ..SettingsPatch::default() });
        assert!(settings.developer_mode && settings.message_display_compact);
        settings.apply(SettingsPatch { guild_folders: Some(vec![]), ..SettingsPatch::default() });
        assert!(settings.developer_mode, "mentioning folders leaves the rest");
    }

    #[test]
    fn no_folders_keeps_guild_order() {
        assert_eq!(
            rail_layout(&[], &ids(&[2, 1])),
            vec![RailEntry::Guild(Snowflake(2)), RailEntry::Guild(Snowflake(1))]
        );
    }
}
