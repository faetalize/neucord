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
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            reactions: ReactionNotifications::Enabled,
            friend_online: true,
            friend_anniversary: true,
            profile_updates: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationsPatch {
    pub reactions: Option<ReactionNotifications>,
    pub friend_online: Option<bool>,
    pub friend_anniversary: Option<bool>,
    pub profile_updates: Option<bool>,
}

impl Notifications {
    fn apply(&mut self, patch: NotificationsPatch) {
        self.reactions = patch.reactions.unwrap_or(self.reactions);
        self.friend_online = patch.friend_online.unwrap_or(self.friend_online);
        self.friend_anniversary = patch.friend_anniversary.unwrap_or(self.friend_anniversary);
        self.profile_updates = patch.profile_updates.unwrap_or(self.profile_updates);
    }
}

/// Who may join the games the account plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Privacy {
    pub friends_join_game: bool,
    pub voice_participants_join_game: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy { friends_join_game: true, voice_participants_join_game: true }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrivacyPatch {
    pub friends_join_game: Option<bool>,
    pub voice_participants_join_game: Option<bool>,
}

impl Privacy {
    fn apply(&mut self, patch: PrivacyPatch) {
        self.friends_join_game = patch.friends_join_game.unwrap_or(self.friends_join_game);
        self.voice_participants_join_game =
            patch.voice_participants_join_game.unwrap_or(self.voice_participants_join_game);
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
    /// The account's status, read from the protobuf as the client does
    /// (`SelfPresenceStore` takes it from that setting, 894768).
    pub status: StatusSettings,
    /// The language the account is set to (`localization.locale`); `None`
    /// until someone picks one.
    pub locale: Option<String>,
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
    }

    /// Take `patch` as all of the settings, as a replacing protobuf is: what
    /// it leaves out is back to its default.
    pub fn replace(&mut self, patch: SettingsPatch) {
        *self = Settings::default();
        self.apply(patch);
    }

    /// Our status is Do Not Disturb, which silences every notification.
    pub fn do_not_disturb(&self) -> bool {
        self.status.status == UserStatus::DoNotDisturb
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
    fn do_not_disturb_is_the_status_the_protobuf_carries() {
        let status = |status| StatusSettings { status, ..StatusSettings::default() };
        let mut settings = Settings::default();
        assert!(!settings.do_not_disturb());
        settings.apply(SettingsPatch { status: Some(status(UserStatus::DoNotDisturb)), ..SettingsPatch::default() });
        assert!(settings.do_not_disturb());
        settings.apply(SettingsPatch { status: Some(status(UserStatus::Idle)), ..SettingsPatch::default() });
        assert!(!settings.do_not_disturb());
        let json = SettingsPatch::from_json(&json!({"status": "dnd"}));
        assert!(json.is_empty(), "the JSON status is not read: the client takes it from the protobuf");
    }

    #[test]
    fn a_replacing_patch_takes_all_of_the_settings() {
        let dnd = StatusSettings { status: UserStatus::DoNotDisturb, ..StatusSettings::default() };
        let mut settings = Settings { status: dnd.clone(), developer_mode: true, ..Settings::default() };
        settings.replace(SettingsPatch { message_display_compact: Some(true), ..SettingsPatch::default() });
        assert!(settings.message_display_compact, "what it says");
        assert!(!settings.developer_mode, "what it leaves out is back to its default");
        assert!(!settings.do_not_disturb(), "no status in it is no status set: Online");
        settings.replace(SettingsPatch { status: Some(dnd), ..SettingsPatch::default() });
        assert!(settings.do_not_disturb());
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
