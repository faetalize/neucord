//! Stickers: a server's own and Discord's standard packs, which of them the
//! picker offers and how a search ranks them. Whether we may send one is
//! `limits.rs`'s ([`Limits::sticker`]); this is the rest of what the client
//! does with them.

use std::collections::HashMap;

use serde_json::Value;

use crate::emoji;
use crate::limits::{Limits, StickerUnavailable};
use crate::model::{Guild, Snowflake, Sticker};

/// How long the standard packs count as loaded (`hasLoadedStickerPacks` in
/// the client's `StickersPackStore`), in milliseconds.
const PACKS_FRESH_MS: u64 = 60 * 60 * 1000;

/// Where a sticker comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A server's own. `available` is false once the server lost the boosts
    /// it took.
    Guild { guild_id: Snowflake, available: bool },
    /// One of Discord's standard packs.
    Pack { pack_id: Snowflake },
}

/// A sticker of a server or a pack, with what a search reads of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub sticker: Sticker,
    pub origin: Origin,
    /// A server sticker's tags: the name of the emoji it stands for
    /// (`smile`), which the client also searches as that emoji.
    pub tags: Option<String>,
}

impl Listed {
    fn from_value(origin: impl Fn(&Value) -> Option<Origin>, value: &Value) -> Option<Listed> {
        Some(Listed {
            sticker: Sticker {
                id: value["id"].as_str()?.parse().ok().map(Snowflake)?,
                name: value["name"].as_str()?.to_owned(),
                format_type: value["format_type"].as_u64().and_then(|f| u8::try_from(f).ok()).unwrap_or(0),
            },
            origin: origin(value)?,
            tags: value["tags"].as_str().map(str::to_owned),
        })
    }

    /// A server's `stickers`: a list, or the full sync the client's own
    /// store reads (`{"op": "full_sync", "items": [...]}`). An `update` op
    /// only edits a list we'd have kept from an earlier session, and we
    /// keep none. `available` is read as sent, as the client does.
    pub fn guild_list(guild_id: Snowflake, value: &Value) -> Option<Vec<Listed>> {
        let items = match value {
            Value::Array(items) => items,
            Value::Object(_) if value["op"] == "full_sync" => value["items"].as_array()?,
            _ => return None,
        };
        let origin = |v: &Value| Some(Origin::Guild { guild_id, available: v["available"] == true });
        Some(items.iter().filter_map(|v| Listed::from_value(origin, v)).collect())
    }
}

/// One of Discord's standard packs.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub id: Snowflake,
    pub name: String,
    pub stickers: Vec<Listed>,
}

impl Pack {
    /// What `/sticker-packs` answers: `{"sticker_packs": [...]}`.
    pub fn list(value: &Value) -> Vec<Pack> {
        let id = |v: &Value| v.as_str()?.parse().ok().map(Snowflake);
        value["sticker_packs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pack| {
                let pack_id = id(&pack["id"])?;
                let origin = |_: &Value| Some(Origin::Pack { pack_id });
                Some(Pack {
                    id: pack_id,
                    name: pack["name"].as_str()?.to_owned(),
                    stickers: pack["stickers"]
                        .as_array()?
                        .iter()
                        .filter_map(|s| Listed::from_value(origin, s))
                        .collect(),
                })
            })
            .collect()
    }
}

/// The standard packs, and whether to fetch them. The client asks when the
/// picker's sticker view opens, unless a fetch is running or one finished
/// within the hour; a fetch that failed is never run again, as its flag
/// stays set (`isFetchingStickerPacks` in `StickersPackStore`).
#[derive(Debug, Default)]
pub struct Packs {
    packs: Vec<Pack>,
    fetching: bool,
    loaded_at: Option<u64>,
}

impl Packs {
    /// Whether the picker opening at `now_ms` (any monotonic clock) fetches.
    pub fn wants_fetch(&self, now_ms: u64) -> bool {
        !self.fetching && !self.fresh(now_ms)
    }

    fn fresh(&self, now_ms: u64) -> bool {
        self.loaded_at.is_some_and(|at| at + PACKS_FRESH_MS > now_ms)
    }

    pub fn fetch_started(&mut self) {
        self.fetching = true;
    }

    pub fn fetched(&mut self, packs: Vec<Pack>, now_ms: u64) {
        self.packs = packs;
        self.fetching = false;
        self.loaded_at = Some(now_ms);
    }

    pub fn all(&self) -> &[Pack] {
        &self.packs
    }
}

/// Where stickers are being sent: the open channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// The channel's server; none in a DM.
    pub guild: Option<Snowflake>,
    /// Use External Emojis in the channel, which the picker asks for before
    /// it lists other servers' stickers (`pD` in the client). A DM needs
    /// none.
    pub external_emojis: bool,
    /// Use External Stickers in the channel, which sending another server's
    /// sticker takes. A DM needs none.
    pub external_stickers: bool,
}

/// A sticker of the picker or a search, and whether we may send it here.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub listed: Listed,
    pub status: Result<(), StickerUnavailable>,
}

/// The stickers of one server or pack.
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub stickers: Vec<Entry>,
}

/// What a search found: what we may send, and what Nitro would unlock.
#[derive(Debug, Default)]
pub struct Found {
    pub unlocked: Vec<Entry>,
    pub locked: Vec<Entry>,
}

/// The stickers the open channel's account has.
pub struct Library {
    pub servers: Vec<Section>,
    pub packs: Vec<Section>,
    /// For the guild names the search reads.
    guild_names: Vec<(Snowflake, String)>,
    pack_names: Vec<(Snowflake, String)>,
}

impl Library {
    /// `guilds` in the order the client lists servers: the channel's own
    /// first, then the rest as the rail has them. Servers with no stickers
    /// are left out; where we lack Use External Emojis only the channel's
    /// own server stays, as in the client's picker.
    pub fn new<'a>(
        guilds: impl IntoIterator<Item = &'a Guild>,
        packs: &[Pack],
        limits: &Limits,
        place: &Place,
    ) -> Library {
        let entry = |listed: &Listed| Entry { status: limits.sticker(&listed.origin, place), listed: listed.clone() };
        let mut guild_names = Vec::new();
        let servers = guilds
            .into_iter()
            .filter(|guild| place.guild.is_none() || place.external_emojis || place.guild == Some(guild.id))
            .filter(|guild| !guild.stickers.is_empty())
            .map(|guild| {
                guild_names.push((guild.id, guild.name.clone()));
                Section { name: guild.name.clone(), stickers: guild.stickers.iter().map(entry).collect() }
            })
            .collect();
        let pack_names = packs.iter().map(|p| (p.id, p.name.clone())).collect();
        let packs =
            packs.iter().map(|p| Section { name: p.name.clone(), stickers: p.stickers.iter().map(entry).collect() });
        Library { servers, packs: packs.filter(|p| !p.stickers.is_empty()).collect(), guild_names, pack_names }
    }

    /// The sticker with `id`, if the library has it.
    pub fn find(&self, id: Snowflake) -> Option<&Entry> {
        self.all().find(|e| e.listed.sticker.id == id)
    }

    fn all(&self) -> impl Iterator<Item = &Entry> {
        self.servers.iter().chain(&self.packs).flat_map(|s| &s.stickers)
    }

    fn source_name(&self, origin: Origin) -> Option<&str> {
        match origin {
            Origin::Guild { guild_id, .. } => self.guild_names.iter().find(|(id, _)| *id == guild_id),
            Origin::Pack { pack_id } => self.pack_names.iter().find(|(id, _)| *id == pack_id),
        }
        .map(|(_, name)| name.as_str())
    }

    /// Stickers matching `query`, best first, as `queryStickers` (997625)
    /// ranks them: what we may send comes first and what Nitro would unlock
    /// after; the rest isn't offered.
    pub fn search(&self, query: &str, scores: &HashMap<String, f64>) -> Found {
        let mut found = Found::default();
        for (_, entry, _) in self.scored(query, scores) {
            match entry.status {
                Ok(()) => found.unlocked.push(entry.clone()),
                Err(StickerUnavailable::Nitro) => found.locked.push(entry.clone()),
                Err(_) => {}
            }
        }
        found
    }

    /// Every sticker `query` matches, best first, with its score and the
    /// value that won (the client's comparator). Each of a sticker's values
    /// scores by how well the query fits it (whole 10, its start 7, anywhere
    /// in it 5; a tag or the emoji it names don't match in the middle) times
    /// a weight (its name 11, its server's or pack's name 8, the emoji its
    /// tag names 6, its tag 1), and the best counts, the first on a tie. A
    /// sticker that was used is scaled by its score over a hundred (`scores`,
    /// by sticker id, as [`crate::usage::History::scores`] has them), which
    /// can also lower it. Only the query is folded (`sS`, 23339), and only
    /// for its start and anywhere: the values are matched as stored, which
    /// for a pack's name is as named (514983), so a name with capitals never
    /// matches whole.
    fn scored(&self, query: &str, scores: &HashMap<String, f64>) -> Vec<(f64, &Entry, String)> {
        let whole = query.to_lowercase();
        let folded = crate::switcher::fold(&whole);
        let mut scored = Vec::new();
        for entry in self.all() {
            let sticker = &entry.listed.sticker;
            // In the stores' order: 41237 for a server's, 514983 for a pack's.
            let mut values = vec![(Match::Name, sticker.name.trim().to_lowercase(), 11)];
            match (entry.listed.origin, &entry.listed.tags) {
                // A server sticker's tag and server name only count with a tag.
                (Origin::Guild { .. }, Some(tags)) => {
                    let tags = tags.trim().to_lowercase();
                    let emoji = emoji::unicode(&tags).map(|e| e.glyph.to_owned());
                    values.push((Match::Other, tags, 1));
                    if let Some(source) = self.source_name(entry.listed.origin).map(|s| s.trim().to_lowercase())
                        && !source.is_empty()
                    {
                        values.push((Match::Name, source, 8));
                    }
                    if let Some(emoji) = emoji {
                        values.push((Match::Other, emoji, 6));
                    }
                }
                (Origin::Guild { .. }, None) => {}
                (Origin::Pack { .. }, _) => {
                    if let Some(source) = self.source_name(entry.listed.origin) {
                        values.push((Match::Name, source.to_owned(), 8));
                    }
                }
            }
            let mut best: Option<(u32, String)> = None;
            for (kind, value, weight) in values {
                let fit = weight * kind.fit(&value, &whole, &folded);
                if fit > best.as_ref().map_or(0, |(score, _)| *score) {
                    best = Some((fit, value));
                }
            }
            let Some((fit, value)) = best else { continue };
            let score = match scores.get(&sticker.id.0.to_string()) {
                Some(used) => f64::from(fit) * used / 100.0,
                None => f64::from(fit),
            };
            if score > 0.0 {
                scored.push((score, entry, value));
            }
        }
        // A stable sort, so equals keep the library's order.
        scored.sort_by(|(a, ..), (b, ..)| b.total_cmp(a));
        scored
    }
}

/// Whether a value may match in the middle of it: the client allows that
/// for a sticker's, a server's and a pack's name only.
#[derive(Clone, Copy)]
enum Match {
    Name,
    Other,
}

impl Match {
    /// `whole` is the lowercased query, `folded` the same folded; the
    /// client's start and anywhere regexes ignore case.
    fn fit(self, value: &str, whole: &str, folded: &str) -> u32 {
        let lower = value.to_lowercase();
        if value == whole {
            10
        } else if lower.starts_with(folded) {
            7
        } else if matches!(self, Match::Name) && lower.contains(folded) {
            5
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::experiments::Experiments;
    use crate::limits::Nitro;

    fn guild(id: u64, name: &str, stickers: Value) -> Guild {
        serde_json::from_value(json!({"id": id.to_string(), "name": name, "stickers": stickers})).unwrap()
    }

    fn open(guild_id: Option<u64>, emojis: bool, stickers: bool) -> Place {
        Place { guild: guild_id.map(Snowflake), external_emojis: emojis, external_stickers: stickers }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.listed.sticker.name.as_str()).collect()
    }

    #[test]
    fn a_server_lists_its_stickers_as_a_list_or_a_full_sync() {
        let items = json!([{"id": "5", "name": "wave", "format_type": 1, "tags": "wave", "available": true}]);
        let sync = json!({"op": "full_sync", "items": items});
        for value in [&items, &sync] {
            let listed = Listed::guild_list(Snowflake(1), value).expect("stickers");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].origin, Origin::Guild { guild_id: Snowflake(1), available: true });
            assert_eq!((listed[0].sticker.format_type, listed[0].tags.as_deref()), (1, Some("wave")));
        }
        assert_eq!(Listed::guild_list(Snowflake(1), &json!({"op": "update"})), None, "only a full sync is a list");
        // As the client reads it, a sticker sent without `available` has lost its boosts.
        let bare = Listed::guild_list(Snowflake(1), &json!([{"id": "5", "name": "a"}])).unwrap();
        assert_eq!(bare[0].origin, Origin::Guild { guild_id: Snowflake(1), available: false });
    }

    #[test]
    fn standard_packs_are_read_from_the_answer() {
        let answer = json!({"sticker_packs": [
            {"id": "70", "name": "Pals", "stickers": [{"id": "71", "name": "hi", "format_type": 3, "pack_id": "70"}]},
            {"name": "no id", "stickers": []},
        ]});
        let packs = Pack::list(&answer);
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].stickers[0].origin, Origin::Pack { pack_id: Snowflake(70) });
        assert_eq!(packs[0].stickers[0].sticker.format_type, 3);
        assert!(Pack::list(&json!({})).is_empty());
    }

    #[test]
    fn packs_are_fetched_when_the_picker_opens_but_not_while_fresh_or_after_a_failure() {
        let mut packs = Packs::default();
        assert!(packs.wants_fetch(0), "never loaded");
        packs.fetch_started();
        assert!(!packs.wants_fetch(10), "one is running");
        packs.fetched(Vec::new(), 1_000);
        assert!(!packs.wants_fetch(1_000 + PACKS_FRESH_MS - 1));
        assert!(packs.wants_fetch(1_000 + PACKS_FRESH_MS), "an hour old");
        // The client's flag stays set when a fetch fails, so it never asks again.
        packs.fetch_started();
        assert!(!packs.wants_fetch(10 * PACKS_FRESH_MS));
    }

    fn library(place: Place, nitro: bool) -> Library {
        let experiments = Experiments::default();
        // Bit 3 is Stickers Everywhere.
        let nitro = Nitro::from_user(&json!({"perks": {"active_perks_bitmask": [if nitro { 1u64 << 3 } else { 0 }]}}));
        let limits = Limits { nitro: &nitro, experiments: &experiments, me: Snowflake(7) };
        let here = guild(1, "Home", json!([{"id": "10", "name": "wave", "tags": "wave", "available": true}]));
        let there = guild(
            2,
            "Away",
            json!([
                {"id": "20", "name": "waving", "tags": "smile", "available": true},
                {"id": "21", "name": "lost", "available": false},
            ]),
        );
        let packs = Pack::list(&json!({"sticker_packs": [
            {"id": "70", "name": "Wavy", "stickers": [{"id": "71", "name": "hello", "pack_id": "70"}]},
        ]}));
        Library::new([&here, &there], &packs, &limits, &place)
    }

    #[test]
    fn the_picker_hides_other_servers_without_use_external_emojis_whatever_stickers_say() {
        let lib = library(open(Some(1), false, true), true);
        assert_eq!(lib.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Home"]);
        assert_eq!(lib.packs.len(), 1, "packs stay");
        let lib = library(open(Some(1), true, false), true);
        assert_eq!(lib.servers.len(), 2);
        assert_eq!(lib.servers[1].stickers[0].status, Err(StickerUnavailable::NotHere));
        assert_eq!(library(open(None, true, true), false).servers.len(), 2, "a DM lists every server's");
    }

    #[test]
    fn search_ranks_by_name_then_tag_and_offers_what_nitro_unlocks_apart() {
        let lib = library(open(Some(1), true, true), false);
        let found = lib.search("wav", &HashMap::new());
        // wave (own, 7*11), Wavy's hello (pack name prefix, 7*8), waving (needs Nitro).
        assert_eq!(names(&found.unlocked), ["wave", "hello"]);
        assert_eq!(names(&found.locked), ["waving"]);
        // A tag matches from its start but never in the middle, and names its emoji.
        assert_eq!(names(&lib.search("smi", &HashMap::new()).locked), ["waving"]);
        assert_eq!(names(&lib.search("mile", &HashMap::new()).locked), Vec::<&str>::new());
        assert_eq!(names(&lib.search("😄", &HashMap::new()).locked), ["waving"]);
        // A sticker whose server lost its boosts isn't offered at all.
        assert!(
            lib.search("lost", &HashMap::new()).unlocked.is_empty()
                && lib.search("lost", &HashMap::new()).locked.is_empty()
        );
    }

    #[test]
    fn a_sticker_that_was_used_is_scaled_by_its_score_over_a_hundred() {
        let lib = library(open(Some(1), true, true), false);
        let id =
            |name: &str| lib.all().find(|e| e.listed.sticker.name == name).unwrap().listed.sticker.id.0.to_string();
        // wave 77 and hello 56 without use.
        let used = HashMap::from([(id("hello"), 200.0)]);
        assert_eq!(names(&lib.search("wav", &used).unlocked), ["hello", "wave"], "112 over 77");
        let little = HashMap::from([(id("wave"), 50.0)]);
        assert_eq!(
            names(&lib.search("wav", &little).unlocked),
            ["hello", "wave"],
            "a low score lowers it: 38 under 56"
        );
        assert!(lib.find(Snowflake(0)).is_none());
        assert_eq!(
            lib.find(lib.all().next().unwrap().listed.sticker.id).map(|e| e.listed.sticker.id.0.to_string()),
            Some(id("wave"))
        );
    }

    #[test]
    fn a_servers_name_finds_its_tagged_stickers_only() {
        let lib = library(open(Some(1), true, true), true);
        // `Away` names waving (tagged) but not lost (untagged, and unavailable anyway).
        assert_eq!(names(&lib.search("away", &HashMap::new()).unlocked), ["waving"]);
        assert_eq!(names(&lib.search("home", &HashMap::new()).unlocked), ["wave"]);
    }

    fn plain_library(guilds: &[&Guild], packs: &[Pack]) -> Library {
        let experiments = Experiments::default();
        let nitro = Nitro::from_user(&json!({}));
        let limits = Limits { nitro: &nitro, experiments: &experiments, me: Snowflake(7) };
        Library::new(guilds.iter().copied(), packs, &limits, &open(Some(1), true, true))
    }

    #[test]
    fn a_query_with_accents_matches_as_the_client_folds_it() {
        let home = guild(
            1,
            "Home",
            json!([
                {"id": "10", "name": "cafe latte", "available": true},
                {"id": "11", "name": "café", "available": true},
                {"id": "12", "name": "Crème brûlée", "available": true},
            ]),
        );
        let lib = plain_library(&[&home], &[]);
        // The query alone is folded for its start and anywhere in a name:
        // "café" scores 10 on itself and 7 on "cafe latte".
        assert_eq!(names(&lib.search("café", &HashMap::new()).unlocked), ["café", "cafe latte"]);
        assert_eq!(names(&lib.search("CAFÉ", &HashMap::new()).unlocked), ["café", "cafe latte"]);
        // The names aren't folded, so an accented name matches only whole or
        // before its accents, as in the client.
        for query in ["creme", "crème", "brûl"] {
            assert_eq!(names(&lib.search(query, &HashMap::new()).unlocked), Vec::<&str>::new(), "{query}");
        }
        assert_eq!(names(&lib.search("cr", &HashMap::new()).unlocked), ["Crème brûlée"]);
    }

    #[test]
    fn a_pack_name_with_capitals_matches_only_from_its_start() {
        let home = guild(1, "Home", json!([{"id": "10", "name": "wavy thing", "available": true}]));
        let packs = Pack::list(&json!({"sticker_packs": [
            {"id": "70", "name": "Wavy", "stickers": [{"id": "71", "name": "hello", "pack_id": "70"}]},
            {"id": "80", "name": "calm", "stickers": [{"id": "81", "name": "rest", "pack_id": "80"}]},
        ]}));
        let lib = plain_library(&[&home], &packs);
        // "Wavy" isn't the lowercased query, so hello scores 7*8 = 56 under wavy thing's 7*11 = 77.
        assert_eq!(names(&lib.search("wavy", &HashMap::new()).unlocked), ["wavy thing", "hello"]);
        // A pack named in lower case still matches whole: 10*8 = 80.
        assert_eq!(names(&lib.search("calm", &HashMap::new()).unlocked), ["rest"]);
        // Matching from its start or anywhere ignores case.
        assert_eq!(names(&lib.search("avy", &HashMap::new()).unlocked), ["wavy thing", "hello"]);
    }
}
