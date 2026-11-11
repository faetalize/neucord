//! `PreloadedUserSettings`, the protobuf form of the account's settings:
//! READY's `user_settings_proto` and USER_SETTINGS_PROTO_UPDATE carry it as
//! base64. Only the fields [`SettingsPatch`] holds are read, and the ones
//! [`Setting`] names are written; the client's schema (module
//! `PreloadedUserSettings`, tracked by discord-watch as `protos`) numbers
//! them.
//!
//! Booleans mostly arrive as `BoolValue` wrappers, so a setting the user
//! never changed is simply absent, and a patch leaves it alone.

use crate::Snowflake;
use crate::protobuf::{Message, Value, bool_value, uint_value};
use crate::settings::{
    GuildFolder, MediaPatch, NotificationsPatch, PrivacyPatch, ReactionNotifications, SettingsPatch, StickerAnimation,
};

// PreloadedUserSettings.
const VERSIONS: u32 = 1;
const TEXT_AND_IMAGES: u32 = 6;
const NOTIFICATIONS: u32 = 7;
const PRIVACY: u32 = 8;
const APPEARANCE: u32 = 13;
const GUILD_FOLDERS: u32 = 14;

// Versions.
const DATA_VERSION: u32 = 3;

// TextAndImagesSettings.
const INLINE_ATTACHMENT_MEDIA: u32 = 9;
const INLINE_EMBED_MEDIA: u32 = 10;
const GIF_AUTO_PLAY: u32 = 11;
const RENDER_EMBEDS: u32 = 12;
const RENDER_REACTIONS: u32 = 13;
const ANIMATE_EMOJI: u32 = 14;
const ANIMATE_STICKERS: u32 = 15;
const MESSAGE_DISPLAY_COMPACT: u32 = 17;

// NotificationSettings.
const REACTION_NOTIFICATIONS: u32 = 7;
const FRIEND_ONLINE_NOTIFICATIONS: u32 = 12;
const FRIEND_ANNIVERSARY_NOTIFICATIONS: u32 = 14;
const PROFILE_UPDATES_NOTIFICATIONS: u32 = 16;

// PrivacySettings.
const ALLOW_ACTIVITY_PARTY_FRIENDS: u32 = 1;
const ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL: u32 = 2;

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
        Some(SettingsPatch {
            guild_folders: settings.message(GUILD_FOLDERS).map(|folders| guild_folders(&folders)),
            media: MediaPatch {
                render_embeds: flag(RENDER_EMBEDS),
                inline_embed_media: flag(INLINE_EMBED_MEDIA),
                inline_attachment_media: flag(INLINE_ATTACHMENT_MEDIA),
                gif_auto_play: flag(GIF_AUTO_PLAY),
                render_reactions: flag(RENDER_REACTIONS),
                animate_emoji: flag(ANIMATE_EMOJI),
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
            },
            privacy: PrivacyPatch {
                friends_join_game: private(ALLOW_ACTIVITY_PARTY_FRIENDS),
                voice_participants_join_game: private(ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL),
            },
            // A plain `bool`: `false` is not sent, so appearance without it is off.
            developer_mode: settings.message(APPEARANCE).map(|a| a.bool(DEVELOPER_MODE).unwrap_or(false)),
            message_display_compact: flag(MESSAGE_DISPLAY_COMPACT),
        })
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    MessageDisplayCompact(bool),
    RenderEmbeds(bool),
    InlineEmbedMedia(bool),
    InlineAttachmentMedia(bool),
    RenderReactions(bool),
    GifAutoPlay(bool),
    AnimateEmoji(bool),
    AnimateStickers(StickerAnimation),
    DeveloperMode(bool),
    ReactionNotifications(ReactionNotifications),
    FriendOnlineNotifications(bool),
    FriendAnniversaryNotifications(bool),
    ProfileUpdatesNotifications(bool),
    FriendsJoinGame(bool),
    VoiceParticipantsJoinGame(bool),
}

impl Setting {
    /// Where the setting lives, as (message of `PreloadedUserSettings`, field
    /// in it) and the value as the client writes it: a wrapper holds even a
    /// default, a plain `bool` or enum leaves its default out.
    fn place(self) -> (u32, u32, Option<Value>) {
        let plain = |on: bool| on.then_some(Value::Varint(1));
        match self {
            Setting::MessageDisplayCompact(on) => (TEXT_AND_IMAGES, MESSAGE_DISPLAY_COMPACT, Some(bool_value(on))),
            Setting::RenderEmbeds(on) => (TEXT_AND_IMAGES, RENDER_EMBEDS, Some(bool_value(on))),
            Setting::InlineEmbedMedia(on) => (TEXT_AND_IMAGES, INLINE_EMBED_MEDIA, Some(bool_value(on))),
            Setting::InlineAttachmentMedia(on) => (TEXT_AND_IMAGES, INLINE_ATTACHMENT_MEDIA, Some(bool_value(on))),
            Setting::RenderReactions(on) => (TEXT_AND_IMAGES, RENDER_REACTIONS, Some(bool_value(on))),
            Setting::GifAutoPlay(on) => (TEXT_AND_IMAGES, GIF_AUTO_PLAY, Some(bool_value(on))),
            Setting::AnimateEmoji(on) => (TEXT_AND_IMAGES, ANIMATE_EMOJI, Some(bool_value(on))),
            Setting::AnimateStickers(how) => (TEXT_AND_IMAGES, ANIMATE_STICKERS, Some(uint_value(how.raw() as u32))),
            Setting::DeveloperMode(on) => (APPEARANCE, DEVELOPER_MODE, plain(on)),
            Setting::ReactionNotifications(which) => {
                (NOTIFICATIONS, REACTION_NOTIFICATIONS, (which.raw() != 0).then_some(Value::Varint(which.raw())))
            }
            Setting::FriendOnlineNotifications(on) => {
                (NOTIFICATIONS, FRIEND_ONLINE_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::FriendAnniversaryNotifications(on) => {
                (NOTIFICATIONS, FRIEND_ANNIVERSARY_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::ProfileUpdatesNotifications(on) => {
                (NOTIFICATIONS, PROFILE_UPDATES_NOTIFICATIONS, Some(bool_value(on)))
            }
            Setting::FriendsJoinGame(on) => (PRIVACY, ALLOW_ACTIVITY_PARTY_FRIENDS, Some(bool_value(on))),
            Setting::VoiceParticipantsJoinGame(on) => {
                (PRIVACY, ALLOW_ACTIVITY_PARTY_VOICE_CHANNEL, Some(bool_value(on)))
            }
        }
    }

    /// The message this setting lives in, as `proto` (a
    /// `PreloadedUserSettings`) has it with this setting changed, and its
    /// field number there. The client sends that whole message, not the one
    /// setting, and keeps the fields it does not know. `None` if `proto` does
    /// not decode.
    pub fn written(self, proto: &[u8]) -> Option<(u32, Vec<u8>)> {
        let (group, field, value) = self.place();
        let current = Message::parse(proto)?.bytes(group).unwrap_or_default();
        Some((group, Message::parse(current)?.with(field, value)))
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

    #[test]
    fn reads_chat_settings() {
        let text = [
            bool_wrapper(GIF_AUTO_PLAY, false),
            bool_wrapper(RENDER_REACTIONS, true),
            bool_wrapper(MESSAGE_DISPLAY_COMPACT, true),
        ]
        .concat();
        let patch = patch(&bytes_field(TEXT_AND_IMAGES, &text));
        assert_eq!(patch.media.gif_auto_play, Some(false), "a wrapper holding false is set, not absent");
        assert_eq!(patch.media.render_reactions, Some(true));
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
            DeveloperMode(true),
            ReactionNotifications(self::ReactionNotifications::OnlyDms),
            FriendOnlineNotifications(false),
            FriendAnniversaryNotifications(false),
            ProfileUpdatesNotifications(false),
            FriendsJoinGame(false),
            VoiceParticipantsJoinGame(false),
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
                },
                developer_mode: true,
                message_display_compact: true,
                notifications: Notifications {
                    reactions: self::ReactionNotifications::OnlyDms,
                    friend_online: false,
                    friend_anniversary: false,
                    profile_updates: false,
                },
                privacy: Privacy { friends_join_game: false, voice_participants_join_game: false },
                ..Settings::default()
            }
        );
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
