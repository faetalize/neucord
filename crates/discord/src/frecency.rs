//! The account's favourite emoji, and the emoji it uses most: the second
//! settings proto the client keeps, `FrecencyUserSettings` (`type` 2 of
//! `/users/@me/settings-proto/{type}`; module 803805 has the schema).
//!
//! The client loads it the first time something needs it (the emoji picker
//! opening, `loadIfNecessary`) and saves changes to it as it does for the
//! other proto, which [`crate::settings_sync::SettingsSync`] does for both.
//! A favourite is saved at once; how often things are used is recorded by
//! [`crate::usage`] and goes in the background, with whatever else is saved.

use std::time::Duration;

use crate::protobuf::{Message, Value, build};
use crate::settings_sync::Kind;
use crate::usage::{Stores, Usage};

// FrecencyUserSettings.
const FAVORITE_GIFS: u32 = 2;
const FAVORITE_STICKERS: u32 = 3;
const STICKER_FRECENCY: u32 = 4;
const FAVORITE_EMOJIS: u32 = 5;
const EMOJI_FRECENCY: u32 = 6;
const GUILD_AND_CHANNEL_FRECENCY: u32 = 12;
const EMOJI_REACTION_FRECENCY: u32 = 13;

// FavoriteEmojis, FavoriteStickers.
const EMOJIS: u32 = 1;
const STICKER_IDS: u32 = 1;

// FavoriteGIFs and FavoriteGIF.
const GIFS: u32 = 1;
const HIDE_TOOLTIP: u32 = 2;
const GIF_FORMAT: u32 = 1;
const GIF_SRC: u32 = 2;
const GIF_WIDTH: u32 = 3;
const GIF_HEIGHT: u32 = 4;
const GIF_ORDER: u32 = 5;

// A frecency's map entries, and FrecencyItem.
const ENTRY_KEY: u32 = 1;
const ENTRY_VALUE: u32 = 2;
const TOTAL_USES: u32 = 1;
const RECENT_USES: u32 = 2;

/// How many favourite emoji there can be (554375).
pub const MAX_FAVORITE_EMOJI: usize = 250;

/// What the client lists as used most for an account with no use of emoji,
/// and of reactions, until it has some (`eT`, 121681). Listing one counts as
/// a use, so a name twice is two.
const SEEDED_EMOJI: [&str; 10] =
    ["thumbsup", "eyes", "laughing", "watermelon", "fork_and_knife", "yum", "weary", "tired_face", "poop", "100"];
const SEEDED_REACTIONS: [&str; 13] = [
    "100",
    "100",
    "thumbsup",
    "thumbsup",
    "thumbsdown",
    "thumbsdown",
    "heart",
    "point_up",
    "eyes",
    "weary",
    "laughing",
    "white_check_mark",
    "x",
];

/// A favourite GIF: a `FavoriteGIF`, filed under the GIF's link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteGif {
    pub url: String,
    pub format: GifFormat,
    pub src: String,
    pub width: u32,
    pub height: u32,
    pub order: u32,
}

/// `GIFType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GifFormat {
    None,
    Image,
    Video,
}

impl GifFormat {
    fn from_raw(raw: u64) -> GifFormat {
        match raw {
            1 => GifFormat::Image,
            2 => GifFormat::Video,
            _ => GifFormat::None,
        }
    }
}

/// The settings a payload mentions; the rest are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrecencyPatch {
    /// `favorite_emojis.emojis`: a custom emoji's id, or a Unicode one's
    /// name, in the order the account favourited them.
    pub favorite_emojis: Option<Vec<String>>,
    /// `favorite_stickers.sticker_ids`.
    pub favorite_stickers: Option<Vec<u64>>,
    /// `favorite_gifs`: the GIFs, and whether the hint to favourite one is
    /// hidden.
    pub favorite_gifs: Option<(Vec<FavoriteGif>, bool)>,
    /// `emoji_frecency.emojis`, in the order sent, by the same keys.
    pub emoji_usage: Option<Vec<(String, Usage)>>,
    /// `emoji_reaction_frecency.emojis`.
    pub reaction_usage: Option<Vec<(String, Usage)>>,
    /// `sticker_frecency.stickers`, by sticker id.
    pub sticker_usage: Option<Vec<(String, Usage)>>,
    /// `guild_and_channel_frecency.guild_and_channels`, by id.
    pub channel_usage: Option<Vec<(String, Usage)>>,
}

/// The entries of a frecency message: a map of `FrecencyItem`s whose keys
/// are strings, or ids when `ids` says.
fn usage_of(frecency: Message<'_>, ids: bool) -> Vec<(String, Usage)> {
    frecency
        .messages(1)
        .into_iter()
        .filter_map(|entry| {
            let item = entry.message(ENTRY_VALUE).unwrap_or_default();
            let usage =
                Usage { total_uses: item.varint(TOTAL_USES).unwrap_or(0), recent_uses: item.varints(RECENT_USES) };
            let key = if ids { entry.fixed64(ENTRY_KEY)?.to_string() } else { entry.string(ENTRY_KEY)?.to_owned() };
            Some((key, usage))
        })
        .collect()
}

impl FrecencyPatch {
    pub fn from_proto_bytes(bytes: &[u8]) -> Option<Self> {
        let settings = Message::parse(bytes)?;
        let favorite_emojis = settings.message(FAVORITE_EMOJIS).map(|favorites| {
            let strings = favorites.strings(EMOJIS);
            strings.into_iter().map(str::to_owned).collect()
        });
        let favorite_stickers = settings.message(FAVORITE_STICKERS).map(|stickers| stickers.fixed64s(STICKER_IDS));
        let favorite_gifs = settings.message(FAVORITE_GIFS).map(|gifs| {
            let list = gifs
                .messages(GIFS)
                .into_iter()
                .filter_map(|entry| {
                    let gif = entry.message(ENTRY_VALUE).unwrap_or_default();
                    Some(FavoriteGif {
                        url: entry.string(ENTRY_KEY)?.to_owned(),
                        format: GifFormat::from_raw(gif.varint(GIF_FORMAT).unwrap_or(0)),
                        src: gif.string(GIF_SRC).unwrap_or_default().to_owned(),
                        width: gif.varint(GIF_WIDTH).unwrap_or(0) as u32,
                        height: gif.varint(GIF_HEIGHT).unwrap_or(0) as u32,
                        order: gif.varint(GIF_ORDER).unwrap_or(0) as u32,
                    })
                })
                .collect();
            (list, gifs.bool(HIDE_TOOLTIP).unwrap_or(false))
        });
        Some(FrecencyPatch {
            favorite_emojis,
            favorite_stickers,
            favorite_gifs,
            emoji_usage: settings.message(EMOJI_FRECENCY).map(|m| usage_of(m, false)),
            reaction_usage: settings.message(EMOJI_REACTION_FRECENCY).map(|m| usage_of(m, false)),
            sticker_usage: settings.message(STICKER_FRECENCY).map(|m| usage_of(m, true)),
            channel_usage: settings.message(GUILD_AND_CHANNEL_FRECENCY).map(|m| usage_of(m, true)),
        })
    }
}

/// What the account has, as [`FrecencyPatch`]es said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frecency {
    pub favorite_emojis: Vec<String>,
    pub favorite_stickers: Vec<u64>,
    pub favorite_gifs: Vec<FavoriteGif>,
    pub hide_gif_tooltip: bool,
    /// What is used, and what of it is not saved yet.
    pub stores: Stores,
}

impl Frecency {
    /// The settings were loaded, or sent whole: what they say replaces
    /// what is held, and what they leave out is empty. `now_ms` is when the
    /// seeded uses count from.
    pub fn load(&mut self, patch: FrecencyPatch, now_ms: u64) {
        self.favorite_emojis.clear();
        self.favorite_stickers.clear();
        self.favorite_gifs.clear();
        self.hide_gif_tooltip = false;
        let patch = FrecencyPatch {
            emoji_usage: Some(patch.emoji_usage.clone().unwrap_or_default()),
            reaction_usage: Some(patch.reaction_usage.clone().unwrap_or_default()),
            sticker_usage: Some(patch.sticker_usage.clone().unwrap_or_default()),
            channel_usage: Some(patch.channel_usage.clone().unwrap_or_default()),
            ..patch
        };
        self.apply(patch, now_ms);
    }

    /// What a patch says replaces what is held; the rest is left.
    pub fn apply(&mut self, patch: FrecencyPatch, now_ms: u64) {
        if let Some(emojis) = patch.favorite_emojis {
            self.favorite_emojis = emojis;
        }
        if let Some(stickers) = patch.favorite_stickers {
            self.favorite_stickers = stickers;
        }
        if let Some((gifs, hide_tooltip)) = patch.favorite_gifs {
            self.favorite_gifs = gifs;
            self.hide_gif_tooltip = hide_tooltip;
        }
        if let Some(usage) = patch.emoji_usage {
            self.stores.emoji.reload(&usage, &SEEDED_EMOJI, now_ms);
        }
        if let Some(usage) = patch.reaction_usage {
            self.stores.reactions.reload(&usage, &SEEDED_REACTIONS, now_ms);
        }
        if let Some(usage) = patch.sticker_usage {
            self.stores.stickers.reload(&usage, &[], now_ms);
        }
        if let Some(usage) = patch.channel_usage {
            self.stores.channels.reload(&usage, &[], now_ms);
        }
    }
}

/// A change to the favourites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setting {
    /// The whole list, which the client writes whole.
    FavoriteEmojis(Vec<String>),
    FavoriteStickers(Vec<u64>),
    /// The whole message: the GIFs and whether the hint is hidden.
    FavoriteGifs(Vec<FavoriteGif>, bool),
}

/// Why an emoji could not be favourited.
#[derive(Debug, PartialEq, Eq)]
pub struct TooMany;

/// The favourites after `key` is favourited or not (`V4` and `Sw` in
/// 554375), or `None` when that changes nothing: it already is, or isn't.
/// The client also drops favourites it can no longer find and repeats
/// (`N`); repeats go here, the rest stays until it saves one.
pub fn toggled(favorites: &[String], key: &str, favorite: bool) -> Result<Option<Vec<String>>, TooMany> {
    let mut unique: Vec<String> = Vec::new();
    for emoji in favorites {
        if !unique.contains(emoji) {
            unique.push(emoji.clone());
        }
    }
    let present = unique.iter().any(|e| e == key);
    match (favorite, present) {
        (false, false) | (true, true) => Ok(None),
        (false, true) => Ok(Some(unique.into_iter().filter(|e| e != key).collect())),
        (true, false) if unique.len() >= MAX_FAVORITE_EMOJI => Err(TooMany),
        (true, false) => {
            unique.push(key.to_owned());
            Ok(Some(unique))
        }
    }
}

/// `FavoriteEmojis`: a repeated string is a field for each, one after another.
fn favorites_message(emojis: &[String]) -> Vec<u8> {
    let empty = Message::parse(&[]).unwrap_or_default();
    emojis.iter().flat_map(|emoji| empty.with(EMOJIS, Some(Value::Bytes(emoji.clone().into_bytes())))).collect()
}

/// `FavoriteStickers`: the ids as `fixed64`s.
fn stickers_message(ids: &[u64]) -> Vec<u8> {
    ids.iter().flat_map(|id| build(&[(STICKER_IDS, Value::Fixed64(*id))])).collect()
}

/// `FavoriteGIFs`, as the client's `toBinary` writes it: a map entry for each
/// GIF, then the hint's flag when it is set.
fn gifs_message(gifs: &[FavoriteGif], hide_tooltip: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for gif in gifs {
        let format = match gif.format {
            GifFormat::None => 0u64,
            GifFormat::Image => 1,
            GifFormat::Video => 2,
        };
        // A field left at its default is not written.
        let mut fields = Vec::new();
        if format != 0 {
            fields.push((GIF_FORMAT, Value::Varint(format)));
        }
        if !gif.src.is_empty() {
            fields.push((GIF_SRC, Value::Bytes(gif.src.clone().into_bytes())));
        }
        for (number, value) in [(GIF_WIDTH, gif.width), (GIF_HEIGHT, gif.height), (GIF_ORDER, gif.order)] {
            if value != 0 {
                fields.push((number, Value::Varint(u64::from(value))));
            }
        }
        let item = Value::Bytes(build(&fields));
        let entry = build(&[(ENTRY_KEY, Value::Bytes(gif.url.clone().into_bytes())), (ENTRY_VALUE, item)]);
        out.extend(build(&[(GIFS, Value::Bytes(entry))]));
    }
    if hide_tooltip {
        out.extend(build(&[(HIDE_TOOLTIP, Value::Varint(1))]));
    }
    out
}

/// How big the client lets `FavoriteGIFs` be (`wg`, 497685): past it, no
/// more GIFs can be favourited.
pub const MAX_FAVORITE_GIFS_BYTES: usize = 762_880;

/// The size `gifs` would be saved at.
pub fn favorite_gifs_size(gifs: &[FavoriteGif], hide_tooltip: bool) -> usize {
    gifs_message(gifs, hide_tooltip).len()
}

/// `FrecencyUserSettings`, saved like `PreloadedUserSettings`.
pub struct Frecent;

impl Kind for Frecent {
    type Setting = Setting;
    type Patch = FrecencyPatch;
    const TYPE: u8 = 2;

    fn written(setting: Setting, _proto: &[u8]) -> Option<(u32, Vec<u8>)> {
        match setting {
            Setting::FavoriteEmojis(emojis) => Some((FAVORITE_EMOJIS, favorites_message(&emojis))),
            Setting::FavoriteStickers(ids) => Some((FAVORITE_STICKERS, stickers_message(&ids))),
            Setting::FavoriteGifs(gifs, hide_tooltip) => Some((FAVORITE_GIFS, gifs_message(&gifs, hide_tooltip))),
        }
    }

    fn delay(_: &Setting) -> Duration {
        // `INFREQUENT_USER_ACTION`.
        Duration::ZERO
    }

    fn patch(proto: &[u8]) -> Option<FrecencyPatch> {
        FrecencyPatch::from_proto_bytes(proto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protobuf::tests::{bytes_field, varint_bytes, varint_field};

    fn fixed64_field(number: u32, value: u64) -> Vec<u8> {
        build(&[(number, Value::Fixed64(value))])
    }
    use crate::settings_sync::SettingsSync;

    fn entry(key: &str, total: u64, recent: &[u64]) -> Vec<u8> {
        let packed: Vec<u8> = recent.iter().flat_map(|r| varint_bytes(*r)).collect();
        let item = [varint_field(TOTAL_USES, total), bytes_field(RECENT_USES, &packed)].concat();
        bytes_field(1, &[bytes_field(ENTRY_KEY, key.as_bytes()), bytes_field(ENTRY_VALUE, &item)].concat())
    }

    #[test]
    fn reads_favourites_and_usage() {
        let favorites = [bytes_field(EMOJIS, b"thumbsup"), bytes_field(EMOJIS, b"123")].concat();
        let usage = [entry("heart", 5, &[100, 200]), entry("456", 1, &[300])].concat();
        let proto = [bytes_field(FAVORITE_EMOJIS, &favorites), bytes_field(EMOJI_FRECENCY, &usage)].concat();
        let patch = FrecencyPatch::from_proto_bytes(&proto).unwrap();
        assert_eq!(patch.favorite_emojis, Some(vec!["thumbsup".to_owned(), "123".to_owned()]));
        let usage = patch.emoji_usage.clone().unwrap();
        assert_eq!(usage[0], ("heart".to_owned(), Usage { total_uses: 5, recent_uses: vec![100, 200] }));
        assert_eq!(usage[1].0, "456");
        // Nothing said of either is nothing.
        assert_eq!(FrecencyPatch::from_proto_bytes(&[]).unwrap(), FrecencyPatch::default());
        let mut frecency = Frecency::default();
        frecency.apply(patch, 0);
        frecency.apply(FrecencyPatch::default(), 0);
        assert_eq!(frecency.favorite_emojis.len(), 2, "an update that says nothing leaves them");
    }

    #[test]
    fn favouriting_adds_to_the_end_removes_and_stops_at_250() {
        let list = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(toggled(&list(&["a", "b"]), "c", true), Ok(Some(list(&["a", "b", "c"]))));
        assert_eq!(toggled(&list(&["a", "b"]), "a", true), Ok(None), "already one");
        assert_eq!(toggled(&list(&["a", "b"]), "a", false), Ok(Some(list(&["b"]))));
        assert_eq!(toggled(&list(&["a"]), "z", false), Ok(None), "not one");
        assert_eq!(toggled(&list(&["a", "b", "a"]), "c", true), Ok(Some(list(&["a", "b", "c"]))), "repeats go");
        let full: Vec<String> = (0..250).map(|n| n.to_string()).collect();
        assert_eq!(toggled(&full, "new", true), Err(TooMany));
        assert!(toggled(&full, "5", false).unwrap().is_some(), "a full list can still lose one");
    }

    #[test]
    fn reads_the_other_usage_and_the_favourite_media() {
        let id_entry = |id: u64, total: u64| {
            let item = varint_field(TOTAL_USES, total);
            bytes_field(1, &[fixed64_field(ENTRY_KEY, id), bytes_field(ENTRY_VALUE, &item)].concat())
        };
        let gif = [
            varint_field(GIF_FORMAT, 1),
            bytes_field(GIF_SRC, b"https://media.example/a.webp"),
            varint_field(GIF_WIDTH, 320),
            varint_field(GIF_HEIGHT, 240),
            varint_field(GIF_ORDER, 2),
        ]
        .concat();
        let gifs = [
            bytes_field(
                GIFS,
                &[bytes_field(ENTRY_KEY, b"https://example.test/a"), bytes_field(ENTRY_VALUE, &gif)].concat(),
            ),
            varint_field(HIDE_TOOLTIP, 1),
        ]
        .concat();
        let proto = [
            bytes_field(FAVORITE_GIFS, &gifs),
            bytes_field(FAVORITE_STICKERS, &[fixed64_field(STICKER_IDS, 7), fixed64_field(STICKER_IDS, 9)].concat()),
            bytes_field(STICKER_FRECENCY, &id_entry(7, 3)),
            bytes_field(GUILD_AND_CHANNEL_FRECENCY, &id_entry(11, 2)),
            bytes_field(EMOJI_REACTION_FRECENCY, &entry("heart", 4, &[1])),
        ]
        .concat();
        let patch = FrecencyPatch::from_proto_bytes(&proto).unwrap();
        assert_eq!(patch.favorite_stickers, Some(vec![7, 9]));
        let (gifs, hidden) = patch.favorite_gifs.clone().unwrap();
        assert!(hidden);
        assert_eq!(
            gifs,
            [FavoriteGif {
                url: "https://example.test/a".into(),
                format: GifFormat::Image,
                src: "https://media.example/a.webp".into(),
                width: 320,
                height: 240,
                order: 2
            }]
        );
        assert_eq!(patch.sticker_usage.as_ref().unwrap()[0].0, "7", "an id key is read as a number");
        assert_eq!(
            patch.channel_usage.as_ref().unwrap()[0],
            ("11".to_owned(), Usage { total_uses: 2, recent_uses: vec![] })
        );
        assert_eq!(patch.reaction_usage.as_ref().unwrap()[0].0, "heart");
    }

    #[test]
    fn loading_empties_what_the_settings_leave_out_and_seeds_the_account_with_no_use() {
        let mut frecency = Frecency::default();
        frecency.favorite_emojis = vec!["x".into()];
        frecency.load(FrecencyPatch::default(), 1_000_000_000_000);
        assert!(frecency.favorite_emojis.is_empty());
        let now = 1_000_000_000_000;
        let listed = frecency.stores.emoji.history.frequently(now, |_| true);
        assert_eq!(listed.len(), 10);
        assert_eq!(listed[0], "thumbsup");
        let reactions = frecency.stores.reactions.history.frequently(now, |_| true);
        assert_eq!(&reactions[..2], ["100", "thumbsup"], "used twice, a name that was listed twice");
        assert!(!frecency.stores.has_pending(), "seeded uses are never sent");
    }

    #[test]
    fn the_first_save_of_an_account_with_no_use_carries_the_seeded_ones_too() {
        let now = 1_700_000_000_000;
        let mut frecency = Frecency::default();
        frecency.load(FrecencyPatch::default(), now);
        frecency.stores.emoji.track("x", now);
        let (groups, _) = frecency.stores.before_send(now);
        let emoji = Message::parse(&groups[0].1).unwrap();
        // The client's history holds them, and it saves the history whole.
        assert_eq!(emoji.messages(1).len(), SEEDED_EMOJI.len() + 1);
        // Loaded after the use, there is nothing to seed: the use is pending.
        let mut late = Frecency::default();
        late.stores.emoji.track("x", now);
        late.load(FrecencyPatch::default(), now);
        let (groups, _) = late.stores.before_send(now);
        assert_eq!(Message::parse(&groups[0].1).unwrap().messages(1).len(), 1);
    }

    #[test]
    fn favourite_media_save_as_whole_messages() {
        let mut sync = SettingsSync::<Frecent>::new();
        assert_eq!(sync.connected(bytes_field(1, &varint_field(3, 7))), None);
        let gif = FavoriteGif {
            url: "https://example.test/a".into(),
            format: GifFormat::Image,
            src: "https://media.example/a.webp".into(),
            width: 320,
            height: 240,
            order: 1,
        };
        let edited = sync.edit(Setting::FavoriteGifs(vec![gif.clone()], false)).unwrap();
        assert_eq!(edited.patch.favorite_gifs, Some((vec![gif.clone()], false)));
        let save = sync.save(edited.schedule.unwrap().generation).unwrap();
        let item = [
            varint_field(GIF_FORMAT, 1),
            bytes_field(GIF_SRC, b"https://media.example/a.webp"),
            varint_field(GIF_WIDTH, 320),
            varint_field(GIF_HEIGHT, 240),
            varint_field(GIF_ORDER, 1),
        ]
        .concat();
        let entry = bytes_field(
            GIFS,
            &[bytes_field(ENTRY_KEY, b"https://example.test/a"), bytes_field(ENTRY_VALUE, &item)].concat(),
        );
        assert_eq!(crate::settings_proto::decode(&save.settings), bytes_field(FAVORITE_GIFS, &entry));
        assert_eq!(favorite_gifs_size(&[gif], false), entry.len());

        let edited = sync.edit(Setting::FavoriteStickers(vec![7, 9])).unwrap();
        let save = sync.save(edited.schedule.unwrap().generation).unwrap();
        let stickers = [fixed64_field(STICKER_IDS, 7), fixed64_field(STICKER_IDS, 9)].concat();
        let sent = crate::settings_proto::decode(&save.settings);
        assert!(sent.windows(stickers.len()).any(|w| w == stickers), "ids are fixed64s, one field each");
    }

    #[test]
    fn what_is_used_goes_in_a_save_with_whatever_else_is_saved() {
        let now = 1_700_000_000_000;
        let mut sync = SettingsSync::<Frecent>::new();
        let mut frecency = Frecency::default();
        assert_eq!(sync.touch(), None, "nothing before it is loaded");
        sync.connected(Vec::new());
        let before = vec![("heart".to_owned(), Usage { total_uses: 1, recent_uses: vec![now - 86_400_000] })];
        frecency.load(FrecencyPatch { emoji_usage: Some(before), ..Default::default() }, now);
        frecency.stores.emoji.track("thumbsup", now);
        let schedule = sync.touch().unwrap();
        assert_eq!(schedule.after, Duration::ZERO);
        let (added, sent) = frecency.stores.before_send(now);
        let save = sync.save_with(schedule.generation, added).unwrap();
        assert_eq!(save.kind, 2);
        let proto = crate::settings_proto::decode(&save.settings);
        let message = Message::parse(&proto).unwrap();
        assert!(message.message(EMOJI_FRECENCY).is_some());
        assert!(message.message(EMOJI_REACTION_FRECENCY).is_some(), "emoji and reactions go together");
        assert!(message.message(FAVORITE_EMOJIS).is_none());
        // Answered, the pending use is the account's.
        let saved = crate::settings_sync::Saved { settings: Some(proto), out_of_date: false };
        let applied = sync.saved(&save, &saved).unwrap();
        frecency.stores.saved(sent);
        frecency.load(applied.patch, now);
        assert!(!frecency.stores.has_pending());
        assert_eq!(frecency.stores.emoji.history.frequently(now, |_| true), ["heart", "thumbsup"]);
        // Nothing pending is a save with nothing to send.
        let schedule = sync.touch().unwrap();
        assert!(sync.save_with(schedule.generation, frecency.stores.before_send(now).0).is_none());
    }

    #[test]
    fn a_favourite_is_saved_as_the_message_with_every_emoji_in_it() {
        let mut sync = SettingsSync::<Frecent>::new();
        assert!(sync.edit(Setting::FavoriteEmojis(vec![])).is_none(), "nothing before it is loaded");
        assert_eq!(sync.connected(bytes_field(1, &varint_field(3, 7))), None);
        let favorites: Vec<String> = vec!["thumbsup".into(), "123".into()];
        let edited = sync.edit(Setting::FavoriteEmojis(favorites.clone())).unwrap();
        assert_eq!(edited.patch.favorite_emojis, Some(favorites));
        let save = sync.save(edited.schedule.unwrap().generation).unwrap();
        assert_eq!(save.kind, 2, "the other settings proto");
        let sent = crate::settings_proto::decode(&save.settings);
        let expected =
            bytes_field(FAVORITE_EMOJIS, &[bytes_field(EMOJIS, b"thumbsup"), bytes_field(EMOJIS, b"123")].concat());
        assert_eq!(sent, expected, "the whole message, a field for each");

        // Emptied, the message is still sent, empty.
        let emptied = sync.edit(Setting::FavoriteEmojis(vec![])).unwrap();
        let save = sync.save(emptied.schedule.unwrap().generation).unwrap();
        assert_eq!(crate::settings_proto::decode(&save.settings), bytes_field(FAVORITE_EMOJIS, &[]));
    }
}
