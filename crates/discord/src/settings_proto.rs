//! `PreloadedUserSettings`, the protobuf form of the account's settings:
//! READY's `user_settings_proto` and USER_SETTINGS_PROTO_UPDATE carry it as
//! base64. Only the fields [`SettingsPatch`] holds are read, and the ones
//! [`Setting`] names are written; the client's schema (module
//! `PreloadedUserSettings`, tracked by discord-watch as `protos`) numbers
//! them.
//!
//! Booleans mostly arrive as `BoolValue` wrappers, so a setting the user
//! never changed is simply absent, and a patch leaves it alone.

use std::time::Duration;

use crate::Snowflake;
use crate::emoji::Tone;
use crate::protobuf::{Message, Value, bool_value, string_value, uint_value, uint64_value};
use crate::sensitive::{DmFilter, Preferences, Redaction, Redactions};
use crate::settings::{
    ActivityDefault, CustomStatus, FriendSources, GuildFolder, MediaPatch, NotificationsPatch, PrivacyPatch,
    ProfileVisibility, ReactionNotifications, SettingsPatch, StatusSettings, StickerAnimation, UserStatus,
};

// PreloadedUserSettings.
const VERSIONS: u32 = 1;
const TEXT_AND_IMAGES: u32 = 6;
const VOICE_AND_VIDEO: u32 = 5;
const NOTIFICATIONS: u32 = 7;
const PRIVACY: u32 = 8;
const STATUS: u32 = 11;
const LOCALIZATION: u32 = 12;
const APPEARANCE: u32 = 13;
const GUILD_FOLDERS: u32 = 14;

// Versions.
const DATA_VERSION: u32 = 3;

// VoiceAndVideoSettings.
const AFK_TIMEOUT: u32 = 6;
const STREAM_NOTIFICATIONS_ENABLED: u32 = 7;

// TextAndImagesSettings.
const DIVERSITY_SURROGATE: u32 = 1;
const SHOW_COMMAND_SUGGESTIONS: u32 = 8;
const INLINE_ATTACHMENT_MEDIA: u32 = 9;
const INLINE_EMBED_MEDIA: u32 = 10;
const GIF_AUTO_PLAY: u32 = 11;
const RENDER_EMBEDS: u32 = 12;
const RENDER_REACTIONS: u32 = 13;
const ANIMATE_EMOJI: u32 = 14;
const ANIMATE_STICKERS: u32 = 15;
const ENABLE_TTS_COMMAND: u32 = 16;
const MESSAGE_DISPLAY_COMPACT: u32 = 17;
const CONVERT_EMOTICONS: u32 = 21;
const VIEW_NSFW_COMMANDS: u32 = 23;
const EXPLICIT_CONTENT_FILTER: u32 = 19;
const EXPLICIT_CONTENT_SETTINGS: u32 = 29;
const GORE_CONTENT_SETTINGS: u32 = 32;
const IS_CROSS_DM_SEARCH_ENABLED: u32 = 36;

// ExplicitContentSettings and GoreContentSettings.
const IN_GUILDS: u32 = 1;
const IN_FRIEND_DMS: u32 = 2;
const IN_NON_FRIEND_DMS: u32 = 3;

// NotificationSettings.
const REACTION_NOTIFICATIONS: u32 = 7;
const FRIEND_ONLINE_NOTIFICATIONS: u32 = 12;
const FRIEND_ANNIVERSARY_NOTIFICATIONS: u32 = 14;
const PROFILE_UPDATES_NOTIFICATIONS: u32 = 16;
const UPCOMING_SERVER_EVENT_NOTIFICATIONS: u32 = 22;
const QUIET_MODE: u32 = 5;
const NOTIFY_FRIENDS_COME_ONLINE: u32 = 25;
const NOTIFY_SERVER_MEMBERS_GO_LIVE: u32 = 26;
const NOTIFY_FRIENDS_CUSTOM_STATUS: u32 = 27;
const FOCUS_MODE_EXPIRES_AT_MS: u32 = 6;

// PrivacySettings.
const ALLOW_ACTIVITY_PARTY_FRIENDS: u32 = 1;
const ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL: u32 = 2;
const RESTRICTED_GUILD_IDS: u32 = 3;
const DEFAULT_GUILDS_RESTRICTED: u32 = 4;
const FRIEND_SOURCE_FLAGS: u32 = 11;
const ACTIVITY_RESTRICTED_GUILD_IDS: u32 = 13;
const ACTIVITY_JOINING_RESTRICTED_GUILD_IDS: u32 = 15;
const MESSAGE_REQUEST_RESTRICTED_GUILD_IDS: u32 = 16;
const DEFAULT_MESSAGE_REQUEST_RESTRICTED: u32 = 17;
const HIDE_FRIEND_REQUEST_NOTES: u32 = 33;
const DEFAULT_GUILDS_RESTRICTED_V2: u32 = 27;
const DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2: u32 = 29;
const PROFILE_VISIBILITY: u32 = 32;

// StatusSettings.
const STATUS_STATUS: u32 = 1;
const STATUS_CUSTOM: u32 = 2;
const STATUS_SHOW_CURRENT_GAME: u32 = 3;
const STATUS_EXPIRES_AT_MS: u32 = 4;
const STATUS_CREATED_AT_MS: u32 = 5;

// CustomStatus.
const CUSTOM_TEXT: u32 = 1;
const CUSTOM_EMOJI_ID: u32 = 2;
const CUSTOM_EMOJI_NAME: u32 = 3;
const CUSTOM_EXPIRES_AT_MS: u32 = 4;
const CUSTOM_CREATED_AT_MS: u32 = 5;

// LocalizationSettings.
const LOCALE: u32 = 1;

// AppearanceSettings.
const DEVELOPER_MODE: u32 = 2;

// GuildFolders and GuildFolder.
const FOLDERS: u32 = 1;
const GUILD_POSITIONS: u32 = 2;
const FOLDER_GUILD_IDS: u32 = 1;
const FOLDER_ID: u32 = 2;
const FOLDER_NAME: u32 = 3;
const FOLDER_COLOR: u32 = 4;

impl SettingsPatch {
    /// The settings in a base64 `PreloadedUserSettings`; `None` if it does
    /// not decode.
    pub fn from_proto(encoded: &str) -> Option<Self> {
        Self::from_proto_bytes(&decode(encoded))
    }

    pub fn from_proto_bytes(bytes: &[u8]) -> Option<Self> {
        let settings = Message::parse(bytes)?;
        let text = settings.message(TEXT_AND_IMAGES);
        let flag = |number| text.as_ref().and_then(|t| t.bool_value(number));
        let notifications = settings.message(NOTIFICATIONS);
        let notified = |number| notifications.as_ref().and_then(|n| n.bool_value(number));
        let privacy = settings.message(PRIVACY);
        let private = |number| privacy.as_ref().and_then(|p| p.bool_value(number));
        let voice = settings.message(VOICE_AND_VIDEO);
        Some(SettingsPatch {
            guild_folders: settings.message(GUILD_FOLDERS).map(|folders| guild_folders(&folders)),
            media: MediaPatch {
                render_embeds: flag(RENDER_EMBEDS),
                inline_embed_media: flag(INLINE_EMBED_MEDIA),
                inline_attachment_media: flag(INLINE_ATTACHMENT_MEDIA),
                gif_auto_play: flag(GIF_AUTO_PLAY),
                render_reactions: flag(RENDER_REACTIONS),
                animate_emoji: flag(ANIMATE_EMOJI),
                convert_emoticons: flag(CONVERT_EMOTICONS),
                animate_stickers: text
                    .as_ref()
                    .and_then(|t| t.int_value(ANIMATE_STICKERS))
                    .and_then(StickerAnimation::from_raw),
            },
            notifications: NotificationsPatch {
                // A plain enum, so notifications without it say the default.
                reactions: notifications
                    .as_ref()
                    .and_then(|n| ReactionNotifications::from_raw(n.varint(REACTION_NOTIFICATIONS).unwrap_or(0))),
                friend_online: notified(FRIEND_ONLINE_NOTIFICATIONS),
                friend_anniversary: notified(FRIEND_ANNIVERSARY_NOTIFICATIONS),
                profile_updates: notified(PROFILE_UPDATES_NOTIFICATIONS),
                go_live: voice.as_ref().and_then(|v| v.bool_value(STREAM_NOTIFICATIONS_ENABLED)),
                upcoming_event: notified(UPCOMING_SERVER_EVENT_NOTIFICATIONS),
                quiet_mode: notified(QUIET_MODE),
                share_online: notified(NOTIFY_FRIENDS_COME_ONLINE),
                share_custom_status: notified(NOTIFY_FRIENDS_CUSTOM_STATUS),
                share_go_live: notified(NOTIFY_SERVER_MEMBERS_GO_LIVE),
                focus_expires_at_ms: notifications.as_ref().map(|n| n.fixed64(FOCUS_MODE_EXPIRES_AT_MS).unwrap_or(0)),
            },
            privacy: PrivacyPatch {
                friends_join_game: private(ALLOW_ACTIVITY_PARTY_FRIENDS),
                voice_participants_join_game: private(ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL),
                friend_sources: privacy
                    .as_ref()
                    .and_then(|p| p.int_value(FRIEND_SOURCE_FLAGS))
                    .map(|flags| FriendSources(flags as u32)),
                servers_restricted: private(DEFAULT_GUILDS_RESTRICTED_V2),
                // A plain `bool`: left out when false.
                servers_restricted_before: privacy.as_ref().map(|p| p.bool(DEFAULT_GUILDS_RESTRICTED).unwrap_or(false)),
                restricted_servers: privacy.as_ref().map(|p| servers(p, RESTRICTED_GUILD_IDS)),
                message_requests_restricted: private(DEFAULT_MESSAGE_REQUEST_RESTRICTED),
                hide_friend_request_notes: private(HIDE_FRIEND_REQUEST_NOTES),
                message_request_restricted_servers: privacy
                    .as_ref()
                    .map(|p| servers(p, MESSAGE_REQUEST_RESTRICTED_GUILD_IDS)),
                // A plain enum: absent is `UNSET`, which is the default.
                profile_visibility: privacy
                    .as_ref()
                    .and_then(|p| ProfileVisibility::from_raw(p.varint(PROFILE_VISIBILITY).unwrap_or(0))),
                // A plain enum too.
                activity_default: privacy
                    .as_ref()
                    .map(|p| ActivityDefault::from_raw(p.varint(DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2).unwrap_or(0))),
                activity_restricted_servers: privacy.as_ref().map(|p| servers(p, ACTIVITY_RESTRICTED_GUILD_IDS)),
                activity_joining_restricted_servers: privacy
                    .as_ref()
                    .map(|p| servers(p, ACTIVITY_JOINING_RESTRICTED_GUILD_IDS)),
            },
            afk_timeout: voice.as_ref().and_then(|v| v.int_value(AFK_TIMEOUT)).map(|seconds| seconds as u32),
            show_current_game: settings.message(STATUS).and_then(|s| s.bool_value(STATUS_SHOW_CURRENT_GAME)),
            // A plain `bool`: `false` is not sent, so appearance without it is off.
            developer_mode: settings.message(APPEARANCE).map(|a| a.bool(DEVELOPER_MODE).unwrap_or(false)),
            message_display_compact: flag(MESSAGE_DISPLAY_COMPACT),
            status: settings.message(STATUS).map(|status| status_settings(&status)),
            localization: settings.message(LOCALIZATION).map(|l| l.string_value(LOCALE).map(String::from)),
            skin_tone: text.as_ref().and_then(|t| t.string_value(DIVERSITY_SURROGATE)).map(Tone::from_modifier),
            nsfw_commands_in_dms: flag(VIEW_NSFW_COMMANDS),
            enable_tts_command: flag(ENABLE_TTS_COMMAND),
            show_command_suggestions: flag(SHOW_COMMAND_SUGGESTIONS),
            cross_dm_search: flag(IS_CROSS_DM_SEARCH_ENABLED),
            sensitive: text.as_ref().map(|t| preferences(t)),
        })
    }
}

/// How `text_and_images` covers sensitive media. The redactions are plain
/// enums, so one left out is unset; the old filter is a wrapper, absent
/// until chosen.
fn preferences(text: &Message) -> Preferences {
    let redactions = |number| {
        let message = text.message(number);
        let at = |field| Redaction::from_raw(message.as_ref().and_then(|m| m.varint(field)).unwrap_or(0));
        Redactions { guilds: at(IN_GUILDS), friend_dm: at(IN_FRIEND_DMS), non_friend_dm: at(IN_NON_FRIEND_DMS) }
    };
    Preferences {
        explicit: redactions(EXPLICIT_CONTENT_SETTINGS),
        gore: redactions(GORE_CONTENT_SETTINGS),
        dm_filter: text.int_value(EXPLICIT_CONTENT_FILTER).map_or_else(DmFilter::default, DmFilter::from_raw),
    }
}

/// A repeated `fixed64` of server ids.
fn servers(message: &Message, number: u32) -> Vec<Snowflake> {
    message.fixed64s(number).into_iter().map(Snowflake).collect()
}

/// The `status` message. A status the client's picker cannot set reads as
/// Online (see [`UserStatus::from_wire`]); a custom status with nothing in
/// it shows nothing, so it is none.
fn status_settings(message: &Message) -> StatusSettings {
    let custom = message.message(STATUS_CUSTOM).map(|c| CustomStatus {
        text: c.string(CUSTOM_TEXT).unwrap_or_default().to_owned(),
        emoji_id: c.fixed64(CUSTOM_EMOJI_ID).filter(|id| *id != 0).map(Snowflake),
        emoji_name: c.string(CUSTOM_EMOJI_NAME).unwrap_or_default().to_owned(),
        expires_at_ms: c.fixed64(CUSTOM_EXPIRES_AT_MS).unwrap_or(0),
        created_at_ms: c.fixed64(CUSTOM_CREATED_AT_MS).unwrap_or(0),
    });
    StatusSettings {
        status: message.string_value(STATUS_STATUS).map(UserStatus::from_wire).unwrap_or_default(),
        expires_at_ms: message.fixed64(STATUS_EXPIRES_AT_MS).unwrap_or(0),
        created_at_ms: message.int_value(STATUS_CREATED_AT_MS),
        custom: custom.filter(|c| !c.is_empty()),
    }
}

/// `CustomStatus` as `internalBinaryWrite` writes it: a field that holds its
/// default is left out.
fn custom_status_bytes(custom: &CustomStatus) -> Vec<u8> {
    let fields = [
        (CUSTOM_TEXT, (!custom.text.is_empty()).then(|| Value::Bytes(custom.text.clone().into_bytes()))),
        (CUSTOM_EMOJI_ID, custom.emoji_id.filter(|id| id.0 != 0).map(|id| Value::Fixed64(id.0))),
        (
            CUSTOM_EMOJI_NAME,
            (!custom.emoji_name.is_empty()).then(|| Value::Bytes(custom.emoji_name.clone().into_bytes())),
        ),
        (CUSTOM_EXPIRES_AT_MS, (custom.expires_at_ms != 0).then_some(Value::Fixed64(custom.expires_at_ms))),
        (CUSTOM_CREATED_AT_MS, (custom.created_at_ms != 0).then_some(Value::Fixed64(custom.created_at_ms))),
    ];
    fields.into_iter().fold(Vec::new(), |out, (number, value)| match Message::parse(&out) {
        Some(message) => message.with(number, value),
        None => out,
    })
}

/// The official client lays servers out from `folders` alone, and falls back
/// to `guild_positions` (a bare order) only when there are none.
fn guild_folders(message: &Message) -> Vec<GuildFolder> {
    let folders: Vec<GuildFolder> = message
        .messages(FOLDERS)
        .iter()
        .map(|f| GuildFolder {
            id: f.int_value(FOLDER_ID),
            name: f.string_value(FOLDER_NAME).filter(|n| !n.is_empty()).map(String::from),
            color: f.int_value(FOLDER_COLOR).map(|c| c as u32),
            guild_ids: f.fixed64s(FOLDER_GUILD_IDS).into_iter().map(Snowflake).collect(),
        })
        .collect();
    if !folders.is_empty() {
        return folders;
    }
    message
        .fixed64s(GUILD_POSITIONS)
        .into_iter()
        .map(|id| GuildFolder { id: None, name: None, color: None, guild_ids: vec![Snowflake(id)] })
        .collect()
}

/// The bytes of a base64 `PreloadedUserSettings` (empty if it is not base64).
pub fn decode(encoded: &str) -> Vec<u8> {
    glib::base64_decode(encoded).to_vec()
}

pub fn encode(proto: &[u8]) -> String {
    glib::base64_encode(proto).to_string()
}

/// The `data_version` the server counted this copy of the settings at
/// (`versions.dataVersion ?? 0`).
pub fn data_version(proto: &[u8]) -> u64 {
    Message::parse(proto).and_then(|p| p.message(VERSIONS)).and_then(|v| v.varint(DATA_VERSION)).unwrap_or(0)
}

/// A setting the settings window writes, with the value it is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setting {
    MessageDisplayCompact(bool),
    RenderEmbeds(bool),
    InlineEmbedMedia(bool),
    InlineAttachmentMedia(bool),
    RenderReactions(bool),
    GifAutoPlay(bool),
    AnimateEmoji(bool),
    /// The message box menu's Suggestions ▸ Slash Commands (194400).
    ShowCommandSuggestions(bool),
    /// The search's cog: Search this DM or Search all DMs (252715).
    CrossDmSearch(bool),
    AnimateStickers(StickerAnimation),
    DeveloperMode(bool),
    ReactionNotifications(ReactionNotifications),
    FriendOnlineNotifications(bool),
    /// Activity Privacy's "Share when I come online", "...update my custom
    /// status" and "...start streaming", and "Share my activity".
    ShareOnline(bool),
    ShareCustomStatus(bool),
    ShareGoLive(bool),
    ShowCurrentGame(bool),
    FriendAnniversaryNotifications(bool),
    ProfileUpdatesNotifications(bool),
    FriendsJoinGame(bool),
    VoiceParticipantsJoinGame(bool),
    /// Privacy & Safety ▸ Friend Requests ▸ Show personalized messages, the
    /// other way round: `hide_friend_request_notes` (`Zd` in 253932).
    HideFriendRequestNotes(bool),
    /// The status picker (`827827`): the status, when it goes back to
    /// Online (0 for never) and the time now. The status keeps its creation
    /// time when it stays what it was.
    Status {
        status: UserStatus,
        expires_at_ms: u64,
        now_ms: u64,
    },
    /// The custom status dialog (`431273`); none clears it.
    CustomStatus(Option<CustomStatus>),
    /// `updateLocale` (817281).
    Locale(String),
    /// The emoji picker's skin tone (`dK`, 554375); none is the default.
    SkinTone(Option<Tone>),
    GoLiveNotifications(bool),
    /// Focus Mode on or off (`ES` in 970931): the quiet mode, and when it
    /// ends, 0 for never. Off has no end.
    FocusMode {
        on: bool,
        expires_at_ms: u64,
    },
    UpcomingEventNotifications(bool),
    FriendSourceFlags(FriendSources),
    /// Direct messages from servers' members. `ServersRestricted` is the
    /// default, with the servers it is also applied to: none leaves them as
    /// they are, an empty list lets every server through. `ServerRestricted`
    /// is one server.
    DefaultServersRestricted {
        restricted: bool,
        apply_to: Option<Vec<Snowflake>>,
    },
    RestrictedServers(Vec<Snowflake>),
    /// The same for message requests.
    DefaultMessageRequestsRestricted {
        restricted: bool,
        apply_to: Option<Vec<Snowflake>>,
    },
    MessageRequestRestrictedServers(Vec<Snowflake>),
    ProfileVisibility(ProfileVisibility),
    /// "Share my activity" for the servers listed as not shared.
    ActivityRestrictedServers(Vec<Snowflake>),
    /// "Activity joining" the same way.
    ActivityJoiningRestrictedServers(Vec<Snowflake>),
    /// What the dialog after a Profile Privacy change confirms (`I`,
    /// 413201): the default, and the servers it brings along.
    ActivityDefault {
        default: ActivityDefault,
        restricted_servers: Vec<Snowflake>,
    },
}

/// How long the client waits before saving a setting (`UserSettingsDelay`):
/// none for most; the skin tone is `FREQUENT_USER_ACTION`.
const FREQUENT_USER_ACTION: Duration = Duration::from_secs(10);

/// A repeated `fixed64`, packed as the client writes it; none if empty.
fn servers_value(ids: &[Snowflake]) -> Option<Value> {
    (!ids.is_empty()).then(|| Value::Bytes(ids.iter().flat_map(|id| id.0.to_le_bytes()).collect()))
}

impl Setting {
    /// Where the setting lives, as (message of `PreloadedUserSettings`, field
    /// in it) and the value as the client writes it: a wrapper holds even a
    /// default, a plain `bool` or enum leaves its default out. Some settings
    /// change several fields, or depend on what is there: `proto` is the
    /// whole `PreloadedUserSettings` as it is now.
    fn place(self, proto: &Message) -> (u32, Vec<(u32, Option<Value>)>) {
        let plain = |on: bool| on.then_some(Value::Varint(1));
        let one = |group, field, value| (group, vec![(field, value)]);
        match self {
            Setting::MessageDisplayCompact(on) => one(TEXT_AND_IMAGES, MESSAGE_DISPLAY_COMPACT, Some(bool_value(on))),
            Setting::RenderEmbeds(on) => one(TEXT_AND_IMAGES, RENDER_EMBEDS, Some(bool_value(on))),
            Setting::InlineEmbedMedia(on) => one(TEXT_AND_IMAGES, INLINE_EMBED_MEDIA, Some(bool_value(on))),
            Setting::InlineAttachmentMedia(on) => one(TEXT_AND_IMAGES, INLINE_ATTACHMENT_MEDIA, Some(bool_value(on))),
            Setting::RenderReactions(on) => one(TEXT_AND_IMAGES, RENDER_REACTIONS, Some(bool_value(on))),
            Setting::GifAutoPlay(on) => one(TEXT_AND_IMAGES, GIF_AUTO_PLAY, Some(bool_value(on))),
            Setting::ShowCommandSuggestions(on) => one(TEXT_AND_IMAGES, SHOW_COMMAND_SUGGESTIONS, Some(bool_value(on))),
            Setting::CrossDmSearch(on) => one(TEXT_AND_IMAGES, IS_CROSS_DM_SEARCH_ENABLED, Some(bool_value(on))),
            Setting::AnimateEmoji(on) => one(TEXT_AND_IMAGES, ANIMATE_EMOJI, Some(bool_value(on))),
            Setting::AnimateStickers(how) => one(TEXT_AND_IMAGES, ANIMATE_STICKERS, Some(uint_value(how.raw() as u32))),
            Setting::DeveloperMode(on) => one(APPEARANCE, DEVELOPER_MODE, plain(on)),
            Setting::ReactionNotifications(which) => {
                one(NOTIFICATIONS, REACTION_NOTIFICATIONS, (which.raw() != 0).then_some(Value::Varint(which.raw())))
            }
            Setting::FriendOnlineNotifications(on) => {
                one(NOTIFICATIONS, FRIEND_ONLINE_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::ShareOnline(on) => one(NOTIFICATIONS, NOTIFY_FRIENDS_COME_ONLINE, Some(bool_value(on))),
            Setting::ShareCustomStatus(on) => one(NOTIFICATIONS, NOTIFY_FRIENDS_CUSTOM_STATUS, Some(bool_value(on))),
            Setting::ShareGoLive(on) => one(NOTIFICATIONS, NOTIFY_SERVER_MEMBERS_GO_LIVE, Some(bool_value(on))),
            Setting::ShowCurrentGame(on) => one(STATUS, STATUS_SHOW_CURRENT_GAME, Some(bool_value(on))),
            Setting::FriendAnniversaryNotifications(on) => {
                one(NOTIFICATIONS, FRIEND_ANNIVERSARY_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::ProfileUpdatesNotifications(on) => {
                one(NOTIFICATIONS, PROFILE_UPDATES_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::FriendsJoinGame(on) => one(PRIVACY, ALLOW_ACTIVITY_PARTY_FRIENDS, Some(bool_value(on))),
            Setting::HideFriendRequestNotes(on) => one(PRIVACY, HIDE_FRIEND_REQUEST_NOTES, Some(bool_value(on))),
            Setting::VoiceParticipantsJoinGame(on) => {
                one(PRIVACY, ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL, Some(bool_value(on)))
            }
            Setting::Status { status, expires_at_ms, now_ms } => {
                let current = proto.message(STATUS).unwrap_or_default();
                let before = current.string_value(STATUS_STATUS).map(UserStatus::from_wire).unwrap_or_default();
                let keeps_creation = before == status && current.message(STATUS_CREATED_AT_MS).is_some();
                let mut edits = vec![
                    (STATUS_STATUS, Some(string_value(status.wire()))),
                    (STATUS_EXPIRES_AT_MS, (expires_at_ms != 0).then_some(Value::Fixed64(expires_at_ms))),
                ];
                if !keeps_creation {
                    edits.push((STATUS_CREATED_AT_MS, Some(uint64_value(now_ms))));
                }
                (STATUS, edits)
            }
            Setting::CustomStatus(custom) => {
                let value = custom.filter(|c| !c.is_empty()).map(|c| Value::Bytes(custom_status_bytes(&c)));
                one(STATUS, STATUS_CUSTOM, value)
            }
            Setting::Locale(locale) => one(LOCALIZATION, LOCALE, Some(string_value(&locale))),
            Setting::SkinTone(tone) => {
                one(TEXT_AND_IMAGES, DIVERSITY_SURROGATE, Some(string_value(tone.map_or("", Tone::modifier))))
            }
            Setting::FocusMode { on, expires_at_ms } => (
                NOTIFICATIONS,
                vec![
                    (QUIET_MODE, Some(bool_value(on))),
                    (FOCUS_MODE_EXPIRES_AT_MS, (on && expires_at_ms != 0).then_some(Value::Fixed64(expires_at_ms))),
                ],
            ),
            Setting::GoLiveNotifications(on) => {
                one(VOICE_AND_VIDEO, STREAM_NOTIFICATIONS_ENABLED, Some(bool_value(on)))
            }
            Setting::UpcomingEventNotifications(on) => {
                one(NOTIFICATIONS, UPCOMING_SERVER_EVENT_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::FriendSourceFlags(sources) => one(PRIVACY, FRIEND_SOURCE_FLAGS, Some(uint_value(sources.0))),
            Setting::DefaultServersRestricted { restricted, apply_to } => {
                let mut edits = vec![(DEFAULT_GUILDS_RESTRICTED_V2, Some(bool_value(restricted)))];
                if let Some(servers) = apply_to {
                    edits.push((RESTRICTED_GUILD_IDS, servers_value(&servers)));
                }
                (PRIVACY, edits)
            }
            Setting::RestrictedServers(servers) => one(PRIVACY, RESTRICTED_GUILD_IDS, servers_value(&servers)),
            Setting::DefaultMessageRequestsRestricted { restricted, apply_to } => {
                let mut edits = vec![(DEFAULT_MESSAGE_REQUEST_RESTRICTED, Some(bool_value(restricted)))];
                if let Some(servers) = apply_to {
                    edits.push((MESSAGE_REQUEST_RESTRICTED_GUILD_IDS, servers_value(&servers)));
                }
                (PRIVACY, edits)
            }
            Setting::MessageRequestRestrictedServers(servers) => {
                one(PRIVACY, MESSAGE_REQUEST_RESTRICTED_GUILD_IDS, servers_value(&servers))
            }
            Setting::ProfileVisibility(visibility) => {
                one(PRIVACY, PROFILE_VISIBILITY, Some(Value::Varint(visibility.raw())))
            }
            Setting::ActivityRestrictedServers(servers) => {
                one(PRIVACY, ACTIVITY_RESTRICTED_GUILD_IDS, servers_value(&servers))
            }
            Setting::ActivityJoiningRestrictedServers(servers) => {
                one(PRIVACY, ACTIVITY_JOINING_RESTRICTED_GUILD_IDS, servers_value(&servers))
            }
            Setting::ActivityDefault { default, restricted_servers } => (
                PRIVACY,
                vec![
                    (DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2, Some(Value::Varint(default.raw()))),
                    (ACTIVITY_RESTRICTED_GUILD_IDS, servers_value(&restricted_servers)),
                ],
            ),
        }
    }

    /// How long after the change the client saves it.
    pub fn delay(&self) -> Duration {
        match self {
            Setting::SkinTone(_) => FREQUENT_USER_ACTION,
            _ => Duration::ZERO,
        }
    }

    /// The message this setting lives in, as `proto` (a
    /// `PreloadedUserSettings`) has it with this setting changed, and its
    /// field number there. The client sends that whole message, not the one
    /// setting, and keeps the fields it does not know. `None` if `proto` does
    /// not decode.
    pub fn written(self, proto: &[u8]) -> Option<(u32, Vec<u8>)> {
        let proto = Message::parse(proto)?;
        let (group, edits) = self.place(&proto);
        let mut written = proto.bytes(group).unwrap_or_default().to_vec();
        for (field, value) in edits {
            written = Message::parse(&written)?.with(field, value);
        }
        Some((group, written))
    }
}

/// `proto` with the messages in `groups` (field number → message) replacing
/// its own, as a partial update does. `None` if either does not decode.
pub fn replace_groups(proto: &[u8], groups: &[(u32, &[u8])]) -> Option<Vec<u8>> {
    let mut proto = proto.to_vec();
    for (number, group) in groups {
        proto = Message::parse(&proto)?.with(*number, Some(Value::Bytes(group.to_vec())));
    }
    Some(proto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protobuf::tests::{bool_wrapper, bytes_field, varint_bytes, varint_field};

    fn fixed64s(ids: &[u64]) -> Vec<u8> {
        ids.iter().flat_map(|id| id.to_le_bytes()).collect()
    }

    fn folder(ids: &[u64], id: Option<u64>, name: Option<&str>, color: Option<u64>) -> Vec<u8> {
        let wrapped = |number, inner: Vec<u8>| bytes_field(number, &inner);
        [
            bytes_field(FOLDER_GUILD_IDS, &fixed64s(ids)),
            id.map(|id| wrapped(FOLDER_ID, varint_field(1, id))).unwrap_or_default(),
            name.map(|n| wrapped(FOLDER_NAME, bytes_field(1, n.as_bytes()))).unwrap_or_default(),
            color.map(|c| wrapped(FOLDER_COLOR, varint_field(1, c))).unwrap_or_default(),
        ]
        .concat()
    }

    fn patch(proto: &[u8]) -> SettingsPatch {
        SettingsPatch::from_proto_bytes(proto).expect("decodes")
    }

    fn fixed64_field(number: u32, v: u64) -> Vec<u8> {
        [varint_bytes(u64::from(number) << 3 | 1), v.to_le_bytes().to_vec()].concat()
    }

    fn string_wrapper(number: u32, v: &str) -> Vec<u8> {
        bytes_field(number, &if v.is_empty() { Vec::new() } else { bytes_field(1, v.as_bytes()) })
    }

    #[test]
    fn reads_chat_settings() {
        let text = [
            bool_wrapper(GIF_AUTO_PLAY, false),
            bool_wrapper(RENDER_REACTIONS, true),
            bool_wrapper(CONVERT_EMOTICONS, false),
            bool_wrapper(MESSAGE_DISPLAY_COMPACT, true),
        ]
        .concat();
        let patch = patch(&bytes_field(TEXT_AND_IMAGES, &text));
        assert_eq!(patch.media.gif_auto_play, Some(false), "a wrapper holding false is set, not absent");
        assert_eq!(patch.media.render_reactions, Some(true));
        assert_eq!(patch.media.convert_emoticons, Some(false));
        assert_eq!(patch.media.render_embeds, None, "never changed, so not sent");
        assert_eq!(patch.message_display_compact, Some(true));
        assert_eq!(patch.developer_mode, None);
        assert_eq!(patch.guild_folders, None);
    }

    #[test]
    fn reads_developer_mode_from_appearance() {
        let patch = patch(&bytes_field(APPEARANCE, &varint_field(DEVELOPER_MODE, 1)));
        assert_eq!(patch.developer_mode, Some(true));
    }

    /// Read from the protobuf only, which READY's JSON also carries but the
    /// client never reads.
    #[test]
    fn age_restricted_commands_in_dms() {
        let patch = patch(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(VIEW_NSFW_COMMANDS, true)));
        assert_eq!(patch.nsfw_commands_in_dms, Some(true));
        assert_eq!(self::patch(&bytes_field(TEXT_AND_IMAGES, &[])).nsfw_commands_in_dms, None);
        let mut settings = crate::settings::Settings { nsfw_commands_in_dms: true, ..Default::default() };
        settings.replace(self::patch(&bytes_field(TEXT_AND_IMAGES, &[])));
        assert!(!settings.nsfw_commands_in_dms, "absent is off");
        let json = SettingsPatch::from_json(&serde_json::json!({"view_nsfw_commands": true}));
        assert_eq!(json.nsfw_commands_in_dms, None);
    }

    /// Redactions are plain enums, so one left out is unset; the old DM filter
    /// is a `UInt32Value`, non-friends (253932's default) until chosen.
    #[test]
    fn reads_which_sensitive_media_is_covered_where() {
        use crate::sensitive::{DmFilter, Redaction::*, Redactions};
        let redactions = |guilds, friend, non_friend| {
            [varint_field(1, guilds), varint_field(2, friend), varint_field(3, non_friend)].concat()
        };
        let text = [
            bytes_field(EXPLICIT_CONTENT_SETTINGS, &redactions(1, 2, 3)),
            bytes_field(GORE_CONTENT_SETTINGS, &redactions(0, 3, 2)),
            bytes_field(EXPLICIT_CONTENT_FILTER, &varint_field(1, 2)),
        ]
        .concat();
        let sensitive = patch(&bytes_field(TEXT_AND_IMAGES, &text)).sensitive.expect("text_and_images is there");
        assert_eq!(sensitive.explicit, Redactions { guilds: Show, friend_dm: Blur, non_friend_dm: Block });
        assert_eq!(sensitive.gore, Redactions { guilds: Unset, friend_dm: Block, non_friend_dm: Blur });
        assert_eq!(sensitive.dm_filter, DmFilter::FriendsAndNonFriends);
        let bare = patch(&bytes_field(TEXT_AND_IMAGES, &[])).sensitive.expect("text_and_images is there");
        assert_eq!(bare.explicit, Redactions::default());
        assert_eq!(bare.dm_filter, DmFilter::NonFriends);
        let disabled = bytes_field(EXPLICIT_CONTENT_FILTER, &[]);
        let sensitive = patch(&bytes_field(TEXT_AND_IMAGES, &disabled)).sensitive.expect("text_and_images is there");
        assert_eq!(sensitive.dm_filter, DmFilter::Disabled, "a wrapper holding 0 is chosen");
        assert_eq!(patch(&bytes_field(PRIVACY, &[])).sensitive, None);
        let mut settings = crate::settings::Settings::default();
        settings.replace(self::patch(&bytes_field(TEXT_AND_IMAGES, &text)));
        settings.replace(self::patch(&bytes_field(TEXT_AND_IMAGES, &[])));
        assert_eq!(settings.sensitive.explicit, Redactions::default(), "what a replacing proto leaves out is unset");
    }

    /// A `BoolValue` that most protos leave out, which means on (253932's
    /// default); READY's JSON copy isn't read.
    #[test]
    fn reads_the_tts_command_setting() {
        let off = patch(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(ENABLE_TTS_COMMAND, false)));
        assert_eq!(off.enable_tts_command, Some(false));
        let on = patch(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(ENABLE_TTS_COMMAND, true)));
        assert_eq!(on.enable_tts_command, Some(true));
        assert_eq!(patch(&bytes_field(TEXT_AND_IMAGES, &[])).enable_tts_command, None);
        let mut settings = crate::settings::Settings::default();
        assert!(settings.tts_command(), "on by default");
        settings.replace(off);
        assert!(!settings.tts_command());
        settings.replace(patch(&bytes_field(TEXT_AND_IMAGES, &[])));
        assert!(settings.tts_command(), "absent is on");
        assert_eq!(
            SettingsPatch::from_json(&serde_json::json!({"enable_tts_command": false})).enable_tts_command,
            None
        );
    }

    /// A `BoolValue` too, on when left out (`showCommandSuggestions` in
    /// 253932).
    #[test]
    fn reads_the_command_suggestions_setting() {
        let off = patch(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(SHOW_COMMAND_SUGGESTIONS, false)));
        assert_eq!(off.show_command_suggestions, Some(false));
        let mut settings = crate::settings::Settings::default();
        assert!(settings.command_suggestions(), "on by default");
        settings.replace(off);
        assert!(!settings.command_suggestions());
        settings.replace(patch(&bytes_field(TEXT_AND_IMAGES, &[])));
        assert!(settings.command_suggestions(), "absent is on");
        let (group, written) = Setting::ShowCommandSuggestions(false).written(&[]).unwrap();
        assert_eq!((group, written), (TEXT_AND_IMAGES, bool_wrapper(SHOW_COMMAND_SUGGESTIONS, false)));
    }

    #[test]
    fn searching_every_dm_is_a_wrapper_off_until_it_is_set() {
        let on = patch(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(IS_CROSS_DM_SEARCH_ENABLED, true)));
        assert_eq!(on.cross_dm_search, Some(true));
        let mut settings = crate::settings::Settings::default();
        assert!(!settings.cross_dm_search(), "off by default");
        settings.apply(on);
        assert!(settings.cross_dm_search());
        let (group, written) = Setting::CrossDmSearch(false).written(&[]).unwrap();
        assert_eq!((group, written), (TEXT_AND_IMAGES, bool_wrapper(IS_CROSS_DM_SEARCH_ENABLED, false)));
    }

    /// Developer Mode is a plain `bool`, not a wrapper: proto3 leaves `false`
    /// out, so appearance without it is how another client turns it off.
    #[test]
    fn appearance_without_developer_mode_turns_it_off() {
        let patch = patch(&bytes_field(APPEARANCE, &[]));
        assert_eq!(patch.developer_mode, Some(false));
        let patch = self::patch(&bytes_field(TEXT_AND_IMAGES, &[]));
        assert_eq!(patch.developer_mode, None, "no appearance, nothing said");
    }

    #[test]
    fn reads_folders_and_loose_servers() {
        let folders = [
            bytes_field(FOLDERS, &folder(&[1], None, None, None)),
            bytes_field(FOLDERS, &folder(&[2, 3], Some(77), Some("Friends"), Some(5793266))),
            bytes_field(FOLDERS, &folder(&[4], Some(78), Some(""), None)),
        ]
        .concat();
        let found = patch(&bytes_field(GUILD_FOLDERS, &folders)).guild_folders.unwrap();
        assert_eq!(found[0], GuildFolder { id: None, name: None, color: None, guild_ids: vec![Snowflake(1)] });
        assert_eq!(
            (found[1].id, found[1].name.as_deref(), found[1].color),
            (Some(77), Some("Friends"), Some(0x5865F2))
        );
        assert_eq!(found[1].guild_ids, vec![Snowflake(2), Snowflake(3)]);
        assert_eq!((found[2].id, found[2].name.as_deref()), (Some(78), None), "an empty name is no name");
    }

    #[test]
    fn positions_stand_in_when_there_are_no_folders() {
        let folders = bytes_field(GUILD_POSITIONS, &fixed64s(&[9, 8]));
        let found = patch(&bytes_field(GUILD_FOLDERS, &folders)).guild_folders.unwrap();
        let ids: Vec<_> = found.iter().map(|f| (f.id, f.guild_ids.clone())).collect();
        assert_eq!(ids, vec![(None, vec![Snowflake(9)]), (None, vec![Snowflake(8)])]);
        let empty = patch(&bytes_field(GUILD_FOLDERS, &[])).guild_folders;
        assert_eq!(empty, Some(vec![]), "folders present but empty clears them");
    }

    #[test]
    fn skips_what_it_does_not_know() {
        let other = [
            varint_field(99, 5),
            bytes_field(7, &[0xff, 0xff]),
            bytes_field(TEXT_AND_IMAGES, &[bool_wrapper(RENDER_EMBEDS, false), varint_field(60, 1)].concat()),
            [varint_bytes(1 << 3 | 1), 0u64.to_le_bytes().to_vec()].concat(),
        ]
        .concat();
        assert_eq!(patch(&other).media.render_embeds, Some(false));
    }

    #[test]
    fn reads_animation_notification_and_privacy_settings() {
        let text = [bool_wrapper(ANIMATE_EMOJI, false), bytes_field(ANIMATE_STICKERS, &varint_field(1, 2))].concat();
        let notifications = [
            varint_field(REACTION_NOTIFICATIONS, 1),
            bool_wrapper(FRIEND_ONLINE_NOTIFICATIONS, false),
            bool_wrapper(PROFILE_UPDATES_NOTIFICATIONS, true),
            bool_wrapper(QUIET_MODE, true),
        ]
        .concat();
        let proto = [
            bytes_field(TEXT_AND_IMAGES, &text),
            bytes_field(NOTIFICATIONS, &notifications),
            bytes_field(PRIVACY, &bool_wrapper(ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL, false)),
        ]
        .concat();
        let patch = patch(&proto);
        assert_eq!(patch.media.animate_emoji, Some(false));
        assert_eq!(patch.media.animate_stickers, Some(StickerAnimation::Never));
        assert_eq!(patch.notifications.reactions, Some(ReactionNotifications::OnlyDms));
        assert_eq!(patch.notifications.friend_online, Some(false));
        assert_eq!(patch.notifications.profile_updates, Some(true));
        assert_eq!(patch.notifications.friend_anniversary, None);
        assert_eq!(patch.notifications.quiet_mode, Some(true));
        assert_eq!(patch.privacy.voice_participants_join_game, Some(false));
        assert_eq!(patch.privacy.friends_join_game, None);

        // The default of a plain enum is not sent.
        let patch = self::patch(&bytes_field(NOTIFICATIONS, &[]));
        assert_eq!(patch.notifications.reactions, Some(ReactionNotifications::Enabled));
    }

    /// What `written` makes is what the client's `updateAsync` makes: the
    /// whole message, every other field in it as it was.
    #[test]
    fn a_written_setting_carries_its_whole_message() {
        let text = [bool_wrapper(RENDER_EMBEDS, true), bool_wrapper(GIF_AUTO_PLAY, true), varint_field(60, 1)].concat();
        let proto = [varint_field(1, 7), bytes_field(TEXT_AND_IMAGES, &text), bytes_field(GUILD_FOLDERS, &[])].concat();

        let (group, written) = Setting::GifAutoPlay(false).written(&proto).unwrap();
        assert_eq!(group, TEXT_AND_IMAGES);
        let expected = [bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, true), varint_field(60, 1)];
        assert_eq!(written, expected.concat(), "a false wrapper is empty but there, the unknown field stays");

        // Nothing there yet: the message starts with the one setting.
        let (group, written) = Setting::DeveloperMode(true).written(&proto).unwrap();
        assert_eq!((group, written), (APPEARANCE, varint_field(DEVELOPER_MODE, 1)));
        let (_, written) = Setting::DeveloperMode(false).written(&proto).unwrap();
        assert!(written.is_empty(), "false leaves the bool out");
        let (_, written) = Setting::ReactionNotifications(ReactionNotifications::Disabled).written(&[]).unwrap();
        assert_eq!(written, varint_field(REACTION_NOTIFICATIONS, 2));
        let (_, written) = Setting::AnimateStickers(StickerAnimation::OnInteraction).written(&[]).unwrap();
        assert_eq!(written, bytes_field(ANIMATE_STICKERS, &varint_field(1, 1)));
        assert!(Setting::RenderEmbeds(true).written(&[0x08]).is_none(), "not a message");
    }

    #[test]
    fn every_written_setting_reads_back() {
        use Setting::*;
        let all = [
            MessageDisplayCompact(true),
            RenderEmbeds(false),
            InlineEmbedMedia(false),
            InlineAttachmentMedia(false),
            RenderReactions(false),
            GifAutoPlay(false),
            AnimateEmoji(false),
            AnimateStickers(StickerAnimation::Never),
            ShowCommandSuggestions(false),
            CrossDmSearch(true),
            DeveloperMode(true),
            ReactionNotifications(self::ReactionNotifications::OnlyDms),
            FriendOnlineNotifications(false),
            ShareOnline(false),
            ShareCustomStatus(false),
            ShareGoLive(false),
            ShowCurrentGame(false),
            FriendAnniversaryNotifications(false),
            ProfileUpdatesNotifications(false),
            FriendsJoinGame(false),
            VoiceParticipantsJoinGame(false),
            HideFriendRequestNotes(true),
            Status { status: UserStatus::Idle, expires_at_ms: 50, now_ms: 40 },
            CustomStatus(Some(self::CustomStatus {
                text: "hi".into(),
                emoji_id: Some(Snowflake(9)),
                emoji_name: "wave".into(),
                expires_at_ms: 60,
                created_at_ms: 41,
            })),
            Locale("fr".into()),
            SkinTone(Some(Tone::MediumDark)),
            GoLiveNotifications(false),
            FocusMode { on: true, expires_at_ms: 1_700_000_900_000 },
            UpcomingEventNotifications(false),
            FriendSourceFlags(FriendSources(6)),
            DefaultServersRestricted { restricted: true, apply_to: Some(vec![Snowflake(4), Snowflake(8)]) },
            DefaultMessageRequestsRestricted { restricted: true, apply_to: Some(vec![Snowflake(8)]) },
            ProfileVisibility(self::ProfileVisibility::FriendsOnly),
        ];
        let mut proto = Vec::new();
        for setting in all {
            let (group, written) = setting.written(&proto).unwrap();
            proto = replace_groups(&proto, &[(group, &written)]).unwrap();
        }
        let mut settings = crate::settings::Settings::default();
        settings.apply(patch(&proto));
        use crate::settings::*;
        assert_eq!(
            settings,
            Settings {
                media: MediaSettings {
                    render_embeds: false,
                    inline_embed_media: false,
                    inline_attachment_media: false,
                    gif_auto_play: false,
                    render_reactions: false,
                    animate_emoji: false,
                    animate_stickers: StickerAnimation::Never,
                    convert_emoticons: true,
                },
                developer_mode: true,
                message_display_compact: true,
                notifications: Notifications {
                    reactions: self::ReactionNotifications::OnlyDms,
                    friend_online: false,
                    friend_anniversary: false,
                    profile_updates: false,
                    go_live: false,
                    upcoming_event: false,
                    quiet_mode: true,
                    focus_expires_at_ms: 1_700_000_900_000,
                    share_online: false,
                    share_custom_status: false,
                    share_go_live: false,
                },
                privacy: Privacy {
                    friends_join_game: false,
                    voice_participants_join_game: false,
                    friend_sources: FriendSources(6),
                    servers_restricted: Some(true),
                    restricted_servers: vec![Snowflake(4), Snowflake(8)],
                    message_requests_restricted: Some(true),
                    message_request_restricted_servers: vec![Snowflake(8)],
                    hide_friend_request_notes: Some(true),
                    profile_visibility: self::ProfileVisibility::FriendsOnly,
                    ..Privacy::default()
                },
                status: StatusSettings {
                    status: UserStatus::Idle,
                    expires_at_ms: 50,
                    created_at_ms: Some(40),
                    custom: Some(self::CustomStatus {
                        text: "hi".into(),
                        emoji_id: Some(Snowflake(9)),
                        emoji_name: "wave".into(),
                        expires_at_ms: 60,
                        created_at_ms: 41,
                    }),
                },
                locale: Some("fr".into()),
                skin_tone: Some(Tone::MediumDark),
                show_command_suggestions: Some(false),
                cross_dm_search: Some(true),
                show_current_game: Some(false),
                ..Settings::default()
            }
        );
    }

    #[test]
    fn reads_status_custom_status_and_locale() {
        let custom = [
            bytes_field(CUSTOM_TEXT, b"out for lunch"),
            fixed64_field(CUSTOM_EMOJI_ID, 77),
            bytes_field(CUSTOM_EMOJI_NAME, b"taco"),
            fixed64_field(CUSTOM_EXPIRES_AT_MS, 1_700_000_900_000),
            fixed64_field(CUSTOM_CREATED_AT_MS, 1_700_000_000_000),
        ]
        .concat();
        let status = [
            string_wrapper(STATUS_STATUS, "dnd"),
            bytes_field(STATUS_CUSTOM, &custom),
            fixed64_field(STATUS_EXPIRES_AT_MS, 1_700_000_500_000),
            bytes_field(STATUS_CREATED_AT_MS, &varint_field(1, 1_700_000_000_000)),
        ]
        .concat();
        let proto = [bytes_field(STATUS, &status), bytes_field(LOCALIZATION, &string_wrapper(LOCALE, "fr"))].concat();
        let patch = patch(&proto);
        let status = patch.status.unwrap();
        assert_eq!(status.status, UserStatus::DoNotDisturb);
        assert_eq!((status.expires_at_ms, status.created_at_ms), (1_700_000_500_000, Some(1_700_000_000_000)));
        assert_eq!(
            status.custom,
            Some(CustomStatus {
                text: "out for lunch".into(),
                emoji_id: Some(Snowflake(77)),
                emoji_name: "taco".into(),
                expires_at_ms: 1_700_000_900_000,
                created_at_ms: 1_700_000_000_000,
            })
        );
        assert_eq!(patch.localization, Some(Some("fr".to_owned())));
    }

    #[test]
    fn a_status_the_picker_cannot_set_is_online() {
        for wire in ["unknown", "offline", "", "away"] {
            let patch = patch(&bytes_field(STATUS, &string_wrapper(STATUS_STATUS, wire)));
            assert_eq!(patch.status.unwrap().status, UserStatus::Online, "{wire:?}");
        }
        // The `status` message without its status (all defaults) is still said.
        let patch = patch(&bytes_field(STATUS, &[]));
        assert_eq!(patch.status, Some(StatusSettings::default()));
        assert_eq!(self::patch(&[]).status, None, "no status message, nothing said");
        let empty = bytes_field(STATUS, &bytes_field(STATUS_CUSTOM, &[]));
        assert_eq!(self::patch(&empty).status.unwrap().custom, None, "a custom status with nothing in it");
    }

    #[test]
    fn sharing_activity_sits_in_the_status_and_leaves_it_as_it_is() {
        let status = [string_wrapper(STATUS_STATUS, "idle"), fixed64_field(STATUS_EXPIRES_AT_MS, 5)].concat();
        let proto = bytes_field(STATUS, &status);
        let (group, written) = Setting::ShowCurrentGame(false).written(&proto).unwrap();
        assert_eq!(group, STATUS);
        let in_order = [
            string_wrapper(STATUS_STATUS, "idle"),
            bool_wrapper(STATUS_SHOW_CURRENT_GAME, false),
            fixed64_field(STATUS_EXPIRES_AT_MS, 5),
        ];
        assert_eq!(written, in_order.concat(), "written by field number, the others as they were");
        // And the status picker, writing the status after, leaves it.
        let proto = bytes_field(STATUS, &written);
        let (_, saved) =
            Setting::Status { status: UserStatus::Online, expires_at_ms: 0, now_ms: 7 }.written(&proto).unwrap();
        let read = patch(&bytes_field(STATUS, &saved));
        assert_eq!(read.show_current_game, Some(false));
        assert_eq!(patch(&bytes_field(STATUS, &status)).show_current_game, None, "unset is on");
        assert!(crate::settings::Settings::default().shares_activity());
    }

    #[test]
    fn the_notifications_about_us_are_fields_25_to_27() {
        let proto = [
            bool_wrapper(NOTIFY_FRIENDS_COME_ONLINE, false),
            bool_wrapper(NOTIFY_SERVER_MEMBERS_GO_LIVE, false),
            bool_wrapper(NOTIFY_FRIENDS_CUSTOM_STATUS, true),
        ]
        .concat();
        let read = patch(&bytes_field(NOTIFICATIONS, &proto)).notifications;
        assert_eq!(
            (read.share_online, read.share_go_live, read.share_custom_status),
            (Some(false), Some(false), Some(true))
        );
        assert_eq!(patch(&bytes_field(NOTIFICATIONS, &[])).notifications.share_online, None);
        assert_eq!(
            (NOTIFY_FRIENDS_COME_ONLINE, NOTIFY_SERVER_MEMBERS_GO_LIVE, NOTIFY_FRIENDS_CUSTOM_STATUS),
            (25, 26, 27)
        );
        let defaults = crate::settings::Notifications::default();
        assert!(defaults.share_online && defaults.share_custom_status && defaults.share_go_live, "on until turned off");
    }

    #[test]
    fn focus_mode_writes_its_flag_and_when_it_ends() {
        let proto = bytes_field(NOTIFICATIONS, &varint_field(60, 1));
        let (group, on) = Setting::FocusMode { on: true, expires_at_ms: 77 }.written(&proto).unwrap();
        assert_eq!(group, NOTIFICATIONS);
        assert_eq!(
            on,
            [bool_wrapper(QUIET_MODE, true), fixed64_field(FOCUS_MODE_EXPIRES_AT_MS, 77), varint_field(60, 1)].concat(),
            "the wrapper holds its true, the end is a fixed64, the unknown field stays"
        );
        // Off holds its false, and no end, whatever was asked.
        let proto = bytes_field(NOTIFICATIONS, &on);
        let (_, off) = Setting::FocusMode { on: false, expires_at_ms: 77 }.written(&proto).unwrap();
        assert_eq!(off, [bool_wrapper(QUIET_MODE, false), varint_field(60, 1)].concat());
        // For ever has no end either.
        let (_, forever) = Setting::FocusMode { on: true, expires_at_ms: 0 }.written(&proto).unwrap();
        assert_eq!(forever, [bool_wrapper(QUIET_MODE, true), varint_field(60, 1)].concat());
        let read = patch(&bytes_field(NOTIFICATIONS, &on)).notifications;
        assert_eq!((read.quiet_mode, read.focus_expires_at_ms), (Some(true), Some(77)));
    }

    #[test]
    fn the_status_picker_saves_the_whole_status_message() {
        let proto = bytes_field(STATUS, &[string_wrapper(STATUS_STATUS, "online"), varint_field(60, 1)].concat());

        // A new status: it expires, and its creation time is now.
        let setting =
            Setting::Status { status: UserStatus::Idle, expires_at_ms: 1_700_000_900_000, now_ms: 1_700_000_000_000 };
        let (group, written) = setting.written(&proto).unwrap();
        assert_eq!(group, STATUS);
        let expected = [
            string_wrapper(STATUS_STATUS, "idle"),
            fixed64_field(STATUS_EXPIRES_AT_MS, 1_700_000_900_000),
            bytes_field(STATUS_CREATED_AT_MS, &varint_field(1, 1_700_000_000_000)),
            varint_field(60, 1),
        ];
        assert_eq!(written, expected.concat(), "the unknown field stays, the expiry is a fixed64");

        // The same status again keeps the time it was set at and drops an expiry of never.
        let kept = [
            string_wrapper(STATUS_STATUS, "dnd"),
            fixed64_field(STATUS_EXPIRES_AT_MS, 5),
            bytes_field(STATUS_CREATED_AT_MS, &varint_field(1, 9)),
        ]
        .concat();
        let proto = bytes_field(STATUS, &kept);
        let again = Setting::Status { status: UserStatus::DoNotDisturb, expires_at_ms: 0, now_ms: 1_700_000_000_000 };
        let (_, written) = again.written(&proto).unwrap();
        let expected = [string_wrapper(STATUS_STATUS, "dnd"), bytes_field(STATUS_CREATED_AT_MS, &varint_field(1, 9))];
        assert_eq!(written, expected.concat());

        // Another status starts over, and one that was never set has no creation time to keep.
        let online = Setting::Status { status: UserStatus::Online, expires_at_ms: 0, now_ms: 7 };
        let (_, written) = online.clone().written(&proto).unwrap();
        let expected =
            [string_wrapper(STATUS_STATUS, "online"), bytes_field(STATUS_CREATED_AT_MS, &varint_field(1, 7))];
        assert_eq!(written, expected.concat());
        let (_, written) = online.written(&[]).unwrap();
        assert_eq!(written, expected.concat());
    }

    #[test]
    fn a_custom_status_is_written_without_its_defaults_and_cleared_by_none() {
        let status = [string_wrapper(STATUS_STATUS, "dnd"), bytes_field(STATUS_CUSTOM, b"\x0a\x03old")].concat();
        let proto = bytes_field(STATUS, &status);
        let custom = CustomStatus {
            text: "hi".into(),
            emoji_id: None,
            emoji_name: "👋".into(),
            expires_at_ms: 0,
            created_at_ms: 3,
        };
        let (_, written) = Setting::CustomStatus(Some(custom)).written(&proto).unwrap();
        let wanted = [
            bytes_field(CUSTOM_TEXT, b"hi"),
            bytes_field(CUSTOM_EMOJI_NAME, "👋".as_bytes()),
            fixed64_field(CUSTOM_CREATED_AT_MS, 3),
        ];
        assert_eq!(
            written,
            [string_wrapper(STATUS_STATUS, "dnd"), bytes_field(STATUS_CUSTOM, &wanted.concat())].concat()
        );

        for cleared in [None, Some(CustomStatus::default())] {
            let (_, written) = Setting::CustomStatus(cleared).written(&proto).unwrap();
            assert_eq!(written, string_wrapper(STATUS_STATUS, "dnd"), "the status stays");
        }
    }

    #[test]
    fn reads_the_afk_timeout_a_wrapper_with_nothing_in_it_is_zero() {
        let uint_wrapper =
            |seconds: u64| bytes_field(AFK_TIMEOUT, &if seconds == 0 { Vec::new() } else { varint_field(1, seconds) });
        let timeout = |wrapper: Vec<u8>| patch(&bytes_field(VOICE_AND_VIDEO, &wrapper)).afk_timeout;
        assert_eq!(timeout(uint_wrapper(300)), Some(300));
        assert_eq!(timeout(uint_wrapper(0)), Some(0), "set to 0: always away");
        assert_eq!(timeout(bool_wrapper(STREAM_NOTIFICATIONS_ENABLED, true)), None, "never set");
        let mut settings = crate::settings::Settings::default();
        assert_eq!(settings.afk_timeout(), 60, "a minute until the account says otherwise");
        settings.apply(SettingsPatch { afk_timeout: Some(0), ..SettingsPatch::default() });
        assert_eq!(settings.afk_timeout(), 0);
    }

    #[test]
    fn reads_privacy_skin_tone_and_the_rest_of_notifications() {
        let privacy = [
            bytes_field(FRIEND_SOURCE_FLAGS, &varint_field(1, 6)),
            bool_wrapper(DEFAULT_GUILDS_RESTRICTED_V2, true),
            bytes_field(RESTRICTED_GUILD_IDS, &fixed64s(&[5, 6])),
            bool_wrapper(DEFAULT_MESSAGE_REQUEST_RESTRICTED, false),
            bytes_field(MESSAGE_REQUEST_RESTRICTED_GUILD_IDS, &fixed64s(&[7])),
            varint_field(PROFILE_VISIBILITY, 1),
        ]
        .concat();
        let proto = [
            bytes_field(TEXT_AND_IMAGES, &string_wrapper(DIVERSITY_SURROGATE, "🏽")),
            bytes_field(VOICE_AND_VIDEO, &bool_wrapper(STREAM_NOTIFICATIONS_ENABLED, false)),
            bytes_field(NOTIFICATIONS, &bool_wrapper(UPCOMING_SERVER_EVENT_NOTIFICATIONS, false)),
            bytes_field(PRIVACY, &privacy),
        ]
        .concat();
        let patch = patch(&proto);
        assert_eq!(patch.skin_tone, Some(Some(Tone::Medium)));
        assert_eq!(patch.notifications.go_live, Some(false));
        assert_eq!(patch.notifications.upcoming_event, Some(false));
        let privacy = patch.privacy;
        assert_eq!(privacy.friend_sources, Some(FriendSources(6)));
        assert_eq!(privacy.servers_restricted, Some(true));
        assert_eq!(privacy.restricted_servers, Some(vec![Snowflake(5), Snowflake(6)]));
        assert_eq!(privacy.message_requests_restricted, Some(false));
        assert_eq!(privacy.message_request_restricted_servers, Some(vec![Snowflake(7)]));
        assert_eq!(privacy.profile_visibility, Some(ProfileVisibility::FriendsOnly));

        // An empty tone is the default; privacy without its lists, plain fields and enum says they are empty or off.
        assert_eq!(
            self::patch(&bytes_field(TEXT_AND_IMAGES, &string_wrapper(DIVERSITY_SURROGATE, ""))).skin_tone,
            Some(None)
        );
        let empty = self::patch(&bytes_field(PRIVACY, &[])).privacy;
        assert_eq!(empty.restricted_servers, Some(vec![]));
        assert_eq!(empty.servers_restricted_before, Some(false));
        assert_eq!(empty.profile_visibility, Some(ProfileVisibility::FriendsAndAllServers), "UNSET");
        assert_eq!((empty.friend_sources, empty.servers_restricted), (None, None), "wrappers never set stay unsaid");
    }

    #[test]
    fn writes_privacy_the_way_the_client_does() {
        let proto =
            bytes_field(PRIVACY, &[bool_wrapper(ALLOW_ACTIVITY_PARTY_FRIENDS, true), varint_field(60, 1)].concat());
        let written = |setting: Setting| setting.written(&proto).unwrap().1;

        // The default alone, and with every server: the list is a packed fixed64.
        let alone = Setting::DefaultServersRestricted { restricted: true, apply_to: None };
        assert_eq!(
            written(alone),
            [
                bool_wrapper(ALLOW_ACTIVITY_PARTY_FRIENDS, true),
                bool_wrapper(DEFAULT_GUILDS_RESTRICTED_V2, true),
                varint_field(60, 1)
            ]
            .concat()
        );
        let all =
            Setting::DefaultServersRestricted { restricted: true, apply_to: Some(vec![Snowflake(5), Snowflake(6)]) };
        let expected = [
            bool_wrapper(ALLOW_ACTIVITY_PARTY_FRIENDS, true),
            bytes_field(RESTRICTED_GUILD_IDS, &fixed64s(&[5, 6])),
            bool_wrapper(DEFAULT_GUILDS_RESTRICTED_V2, true),
            varint_field(60, 1),
        ];
        assert_eq!(written(all), expected.concat());
        // Allowing again empties the list, which leaves it out.
        let none = Setting::DefaultServersRestricted { restricted: false, apply_to: Some(vec![]) };
        let expected = [
            bool_wrapper(ALLOW_ACTIVITY_PARTY_FRIENDS, true),
            bool_wrapper(DEFAULT_GUILDS_RESTRICTED_V2, false),
            varint_field(60, 1),
        ];
        assert_eq!(written(none), expected.concat(), "a false wrapper is empty but there");

        let flags = written(Setting::FriendSourceFlags(FriendSources(6)));
        assert_eq!(Message::parse(&flags).unwrap().int_value(FRIEND_SOURCE_FLAGS), Some(6));
        let visibility = written(Setting::ProfileVisibility(ProfileVisibility::FriendsAndSmallServers));
        assert_eq!(Message::parse(&visibility).unwrap().varint(PROFILE_VISIBILITY), Some(2));
        let requests =
            Setting::DefaultMessageRequestsRestricted { restricted: true, apply_to: Some(vec![Snowflake(9)]) };
        let requests = written(requests);
        let requests = Message::parse(&requests).unwrap();
        assert_eq!(requests.bool_value(DEFAULT_MESSAGE_REQUEST_RESTRICTED), Some(true));
        assert_eq!(requests.fixed64s(MESSAGE_REQUEST_RESTRICTED_GUILD_IDS), vec![9]);
        let servers = Setting::RestrictedServers(vec![Snowflake(3)]);
        assert_eq!(Message::parse(&written(servers)).unwrap().fixed64s(RESTRICTED_GUILD_IDS), vec![3]);

        let (group, tone) = Setting::SkinTone(Some(Tone::Dark)).written(&[]).unwrap();
        assert_eq!((group, tone), (TEXT_AND_IMAGES, string_wrapper(DIVERSITY_SURROGATE, "🏿")));
        let (_, default) = Setting::SkinTone(None).written(&[]).unwrap();
        assert_eq!(default, string_wrapper(DIVERSITY_SURROGATE, ""), "the default is an empty string, there");
    }

    #[test]
    fn reads_and_writes_the_activity_privacy_the_way_the_client_does() {
        let privacy = [
            bytes_field(ACTIVITY_RESTRICTED_GUILD_IDS, &fixed64s(&[3, 4])),
            bytes_field(ACTIVITY_JOINING_RESTRICTED_GUILD_IDS, &fixed64s(&[5])),
            varint_field(DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2, 2),
        ]
        .concat();
        let read = patch(&bytes_field(PRIVACY, &privacy)).privacy;
        assert_eq!(read.activity_restricted_servers, Some(vec![Snowflake(3), Snowflake(4)]));
        assert_eq!(read.activity_joining_restricted_servers, Some(vec![Snowflake(5)]));
        assert_eq!(read.activity_default, Some(Some(ActivityDefault::LargeServers)));
        let unset = patch(&bytes_field(PRIVACY, &[])).privacy;
        assert_eq!(unset.activity_default, Some(None), "UNSET is none: the region's default stands");
        assert_eq!(unset.activity_restricted_servers, Some(vec![]));
        assert_eq!(patch(&[]).privacy.activity_default, None, "no privacy, nothing said");

        let proto = bytes_field(PRIVACY, &varint_field(60, 1));
        let written = |setting: Setting| setting.written(&proto).unwrap().1;
        let joining = written(Setting::ActivityJoiningRestrictedServers(vec![Snowflake(8)]));
        assert_eq!(Message::parse(&joining).unwrap().fixed64s(ACTIVITY_JOINING_RESTRICTED_GUILD_IDS), vec![8]);
        let status = written(Setting::ActivityRestrictedServers(vec![Snowflake(9)]));
        assert_eq!(Message::parse(&status).unwrap().fixed64s(ACTIVITY_RESTRICTED_GUILD_IDS), vec![9]);
        // What the dialog confirms goes in one message, with what else is there kept.
        let both = written(Setting::ActivityDefault {
            default: ActivityDefault::On,
            restricted_servers: vec![Snowflake(1), Snowflake(2)],
        });
        assert_eq!(
            both,
            [
                bytes_field(ACTIVITY_RESTRICTED_GUILD_IDS, &fixed64s(&[1, 2])),
                varint_field(DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2, 3),
                varint_field(60, 1)
            ]
            .concat()
        );
        let emptied = written(Setting::ActivityDefault { default: ActivityDefault::Off, restricted_servers: vec![] });
        assert_eq!(
            emptied,
            [varint_field(DEFAULT_GUILDS_ACTIVITY_RESTRICTED_V2, 1), varint_field(60, 1)].concat(),
            "an emptied list is left out"
        );
    }

    #[test]
    fn the_language_is_a_string_wrapper() {
        let (group, written) = Setting::Locale("pt-BR".into()).written(&[]).unwrap();
        assert_eq!((group, written), (LOCALIZATION, string_wrapper(LOCALE, "pt-BR")));
    }

    #[test]
    fn replacing_messages_leaves_the_rest() {
        let proto = [varint_field(1, 7), bytes_field(TEXT_AND_IMAGES, b"old"), bytes_field(PRIVACY, b"p")].concat();
        let replaced = replace_groups(&proto, &[(TEXT_AND_IMAGES, b"new"), (APPEARANCE, b"a")]).unwrap();
        let expected = [varint_field(1, 7), bytes_field(TEXT_AND_IMAGES, b"new"), bytes_field(PRIVACY, b"p")];
        assert_eq!(replaced, [expected.concat(), bytes_field(APPEARANCE, b"a")].concat());
        assert_eq!(data_version(&[bytes_field(VERSIONS, &varint_field(DATA_VERSION, 12))].concat()), 12);
        assert_eq!(data_version(&proto), 0, "no versions");
    }

    #[test]
    fn base64_in_garbage_out() {
        let encoded = glib::base64_encode(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(RENDER_EMBEDS, false)));
        assert_eq!(SettingsPatch::from_proto(&encoded).unwrap().media.render_embeds, Some(false));
        assert!(SettingsPatch::from_proto(&glib::base64_encode(&[0x08])).is_none(), "cut short");
        assert!(SettingsPatch::from_proto("").unwrap().is_empty(), "no settings at all");
    }
}
