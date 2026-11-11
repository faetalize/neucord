//! `PreloadedUserSettings`, the protobuf form of the account's settings:
//! READY's `user_settings_proto` and USER_SETTINGS_PROTO_UPDATE carry it as
//! base64. Only the fields [`SettingsPatch`] holds are read; the client's
//! schema (module `PreloadedUserSettings`, tracked by discord-watch as
//! `protos`) numbers them.
//!
//! Booleans arrive as `BoolValue` wrappers, so a setting the user never
//! changed is simply absent, and a patch leaves it alone.

use crate::Snowflake;
use crate::protobuf::Message;
use crate::settings::{GuildFolder, MediaPatch, SettingsPatch};

// PreloadedUserSettings.
const TEXT_AND_IMAGES: u32 = 6;
const APPEARANCE: u32 = 13;
const GUILD_FOLDERS: u32 = 14;

// TextAndImagesSettings.
const INLINE_ATTACHMENT_MEDIA: u32 = 9;
const INLINE_EMBED_MEDIA: u32 = 10;
const GIF_AUTO_PLAY: u32 = 11;
const RENDER_EMBEDS: u32 = 12;
const RENDER_REACTIONS: u32 = 13;
const MESSAGE_DISPLAY_COMPACT: u32 = 17;

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
        let bytes = glib::base64_decode(encoded);
        Self::from_proto_bytes(&bytes)
    }

    fn from_proto_bytes(bytes: &[u8]) -> Option<Self> {
        let settings = Message::parse(bytes)?;
        let text = settings.message(TEXT_AND_IMAGES);
        let flag = |number| text.as_ref().and_then(|t| t.bool_value(number));
        Some(SettingsPatch {
            guild_folders: settings.message(GUILD_FOLDERS).map(|folders| guild_folders(&folders)),
            media: MediaPatch {
                render_embeds: flag(RENDER_EMBEDS),
                inline_embed_media: flag(INLINE_EMBED_MEDIA),
                inline_attachment_media: flag(INLINE_ATTACHMENT_MEDIA),
                gif_auto_play: flag(GIF_AUTO_PLAY),
                render_reactions: flag(RENDER_REACTIONS),
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
    fn base64_in_garbage_out() {
        let encoded = glib::base64_encode(&bytes_field(TEXT_AND_IMAGES, &bool_wrapper(RENDER_EMBEDS, false)));
        assert_eq!(SettingsPatch::from_proto(&encoded).unwrap().media.render_embeds, Some(false));
        assert!(SettingsPatch::from_proto(&glib::base64_encode(&[0x08])).is_none(), "cut short");
        assert!(SettingsPatch::from_proto("").unwrap().is_empty(), "no settings at all");
    }
}
