//! The account's favourite emoji, and the emoji it uses most: the second
//! settings proto the client keeps, `FrecencyUserSettings` (`type` 2 of
//! `/users/@me/settings-proto/{type}`; module 803805 has the schema).
//!
//! The client loads it the first time something needs it (the emoji picker
//! opening, `loadIfNecessary`) and saves changes to it as it does for the
//! other proto, which [`crate::settings_sync::SettingsSync`] does for both.
//! A favourite is saved at once; what the client records of how often each
//! emoji is used it sends in the background, with other usage it keeps, and
//! Neucord sends none of that.

use std::time::Duration;

use crate::protobuf::{Message, Value};
use crate::settings_sync::Kind;

// FrecencyUserSettings.
const FAVORITE_EMOJIS: u32 = 5;
const EMOJI_FRECENCY: u32 = 6;

// FavoriteEmojis.
const EMOJIS: u32 = 1;

// EmojiFrecency's map entries, and FrecencyItem.
const ENTRY_KEY: u32 = 1;
const ENTRY_VALUE: u32 = 2;
const TOTAL_USES: u32 = 1;
const RECENT_USES: u32 = 2;

/// How many favourite emoji there can be (554375).
pub const MAX_FAVORITE_EMOJI: usize = 250;

/// The most recent uses the client scores an emoji by.
const MAX_SAMPLES: usize = 10;
/// How many frequently used emoji the picker lists (`numFrequentlyItems` in
/// `eu`, 121681).
const FREQUENTLY_USED: usize = 42;

/// How often and when an emoji was used: a `FrecencyItem`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub total_uses: u64,
    /// Milliseconds since 1970, in the order sent.
    pub recent_uses: Vec<u64>,
}

/// The settings a payload mentions; the rest are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrecencyPatch {
    /// `favorite_emojis.emojis`: a custom emoji's id, or a Unicode one's
    /// name, in the order the account favourited them.
    pub favorite_emojis: Option<Vec<String>>,
    /// `emoji_frecency.emojis`, in the order sent, by the same keys.
    pub emoji_usage: Option<Vec<(String, Usage)>>,
}

impl FrecencyPatch {
    pub fn from_proto_bytes(bytes: &[u8]) -> Option<Self> {
        let settings = Message::parse(bytes)?;
        let favorite_emojis = settings.message(FAVORITE_EMOJIS).map(|favorites| {
            let strings = favorites.strings(EMOJIS);
            strings.into_iter().map(str::to_owned).collect()
        });
        let emoji_usage = settings.message(EMOJI_FRECENCY).map(|frecency| {
            frecency
                .messages(1)
                .into_iter()
                .filter_map(|entry| {
                    let item = entry.message(ENTRY_VALUE).unwrap_or_default();
                    let usage = Usage {
                        total_uses: item.varint(TOTAL_USES).unwrap_or(0),
                        recent_uses: item.varints(RECENT_USES),
                    };
                    Some((entry.string(ENTRY_KEY)?.to_owned(), usage))
                })
                .collect()
        });
        Some(FrecencyPatch { favorite_emojis, emoji_usage })
    }
}

/// What the account has, as [`FrecencyPatch`]es said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frecency {
    pub favorite_emojis: Vec<String>,
    pub emoji_usage: Vec<(String, Usage)>,
}

impl Frecency {
    pub fn apply(&mut self, patch: FrecencyPatch) {
        if let Some(emojis) = patch.favorite_emojis {
            self.favorite_emojis = emojis;
        }
        if let Some(usage) = patch.emoji_usage {
            self.emoji_usage = usage;
        }
    }
}

/// A change to the favourites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setting {
    /// The whole list, which the client writes whole.
    FavoriteEmojis(Vec<String>),
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

/// The keys of the emoji used most, best first, as the client orders
/// `emojiFrecency` (`compute` in 283047): each is scored by how long ago its
/// most recent uses were, scaled by how often it was used, and the best 42
/// that `known` finds are listed. `now_ms` is the time to score by.
pub fn frequently_used(usage: &[(String, Usage)], now_ms: u64, known: impl Fn(&str) -> bool) -> Vec<String> {
    // How much a use counts for, by the whole days since (`computeWeight`).
    let weight = |days: i64| match days {
        ..=3 => 100,
        4..=15 => 70,
        16..=30 => 50,
        31..=45 => 30,
        46..=80 => 10,
        _ => 1,
    };
    let mut scored: Vec<(&str, i64)> = usage
        .iter()
        .filter(|(_, u)| !u.recent_uses.is_empty())
        .filter(|(key, _)| known(key))
        .map(|(key, u)| {
            let score: i64 = u
                .recent_uses
                .iter()
                .take(MAX_SAMPLES)
                .map(|at| weight((now_ms as i64 - *at as i64) / Duration::from_secs(86_400).as_millis() as i64))
                .sum();
            // `Math.ceil(totalUses * (score / numOfRecentUses))`.
            let frecency = (u.total_uses as f64 * (score as f64 / u.recent_uses.len() as f64)).ceil() as i64;
            (key.as_str(), frecency)
        })
        .collect();
    // Stable, so equals stay in the order they were sent.
    scored.sort_by_key(|(_, frecency)| std::cmp::Reverse(*frecency));
    scored.into_iter().take(FREQUENTLY_USED).map(|(key, _)| key.to_owned()).collect()
}

/// `FavoriteEmojis`: a repeated string is a field for each, one after another.
fn favorites_message(emojis: &[String]) -> Vec<u8> {
    let empty = Message::parse(&[]).unwrap_or_default();
    emojis.iter().flat_map(|emoji| empty.with(EMOJIS, Some(Value::Bytes(emoji.clone().into_bytes())))).collect()
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
        frecency.apply(patch);
        frecency.apply(FrecencyPatch::default());
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
    fn frequently_used_by_recent_weight_and_how_often() {
        const DAY: u64 = 86_400_000;
        let now = 1_000_000 * DAY;
        let usage = vec![
            ("old".to_owned(), Usage { total_uses: 50, recent_uses: vec![now - 100 * DAY] }),
            ("often".to_owned(), Usage { total_uses: 10, recent_uses: vec![now - DAY, now - 2 * DAY] }),
            ("gone".to_owned(), Usage { total_uses: 9, recent_uses: vec![] }),
            ("new".to_owned(), Usage { total_uses: 3, recent_uses: vec![now] }),
            ("unknown".to_owned(), Usage { total_uses: 99, recent_uses: vec![now] }),
        ];
        // often: 10 × (100 + 100) / 2 = 1000; new: 3 × 100 = 300; old: 50 × 1 = 50.
        let known = |key: &str| key != "unknown";
        assert_eq!(frequently_used(&usage, now, known), ["often", "new", "old"]);

        // Older uses weigh less, by the day counts the client has.
        let at = |days: u64| vec![("e".to_owned(), Usage { total_uses: 1, recent_uses: vec![now - days * DAY] })];
        let weight = |days| {
            // One use, once: its frecency is its weight, which orders it against a fixed rival.
            let rival = ("r".to_owned(), Usage { total_uses: 1, recent_uses: vec![now - 4 * DAY] });
            let mut both = at(days);
            both.push(rival);
            frequently_used(&both, now, |_| true)[0].clone()
        };
        assert_eq!(weight(3), "e", "3 days is 100, over the rival's 70");
        assert_eq!(weight(5), "e", "5 days is 70 too, a tie: the order sent holds");
        assert_eq!(weight(16), "r", "16 days is 50");
        // Only the first ten uses count, the number of them still divides.
        let many = vec![("m".to_owned(), Usage { total_uses: 2, recent_uses: vec![now; 12] })];
        assert_eq!(frequently_used(&many, now, |_| true), ["m"]);
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
