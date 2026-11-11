//! The account's client settings that Neucord honours: server folders
//! (which also define the order of servers in the rail), how media and
//! embeds show, Developer Mode, and compact display.
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
}

impl Default for MediaSettings {
    fn default() -> Self {
        MediaSettings {
            render_embeds: true,
            inline_embed_media: true,
            inline_attachment_media: true,
            gif_auto_play: true,
            render_reactions: true,
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
}

/// The settings a payload mentions; the rest are `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsPatch {
    pub guild_folders: Option<Vec<GuildFolder>>,
    pub media: MediaPatch,
    pub developer_mode: Option<bool>,
    pub message_display_compact: Option<bool>,
}

impl SettingsPatch {
    pub fn from_json(settings: &Value) -> Self {
        SettingsPatch {
            guild_folders: guild_folders(settings),
            media: MediaPatch::from_settings(settings),
            developer_mode: settings.get("developer_mode").and_then(Value::as_bool),
            message_display_compact: settings.get("message_display_compact").and_then(Value::as_bool),
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
    fn json_patch_holds_what_is_mentioned() {
        let patch = SettingsPatch::from_json(&json!({"developer_mode": true, "theme": "dark"}));
        assert_eq!(patch, SettingsPatch { developer_mode: Some(true), ..SettingsPatch::default() });
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
