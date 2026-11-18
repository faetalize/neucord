//! Emoji packs: sets of emoji Discord gives to Nitro, which the picker lists
//! for the ones an account pinned (`EmojiPackStore` in 956396, its fetch in
//! 934499).
//!
//! The whole feature is behind the user experiment `2026-09-default-emojis`
//! (490035): with it off the store holds no usable emoji, the picker shows
//! no packs, and `:name:` offers none of them. The packs are fetched with
//! `GET /emoji-packs` when the picker opens; a 404 means the account has none
//! (they aren't enabled for it), which is an empty list, not a failure. Which
//! packs are pinned is the account's `PinnedEmojiPacks` setting
//! ([`crate::frecency`]).

use serde_json::Value;

use crate::emoji::GuildEmoji;
use crate::experiments::Experiments;
use crate::model::Snowflake;

/// The experiment that turns packs on: variant 1 has `enabled`.
pub const EXPERIMENT: &str = "2026-09-default-emojis";

/// Whether packs are on for the account (`getConfig({location: "emoji_pack_store"}).enabled`).
pub fn enabled(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, EXPERIMENT).is_some_and(|a| a.variant == 1)
}

/// A pack's emoji: a custom emoji that belongs to the pack, not to a server.
#[derive(Clone, Debug, PartialEq)]
pub struct PackEmoji {
    pub id: Snowflake,
    pub name: String,
    pub animated: bool,
    /// Sent as sent, so a payload without it counts as not available, as the
    /// server emoji's flag does.
    pub available: bool,
    pub managed: bool,
    pub roles: Vec<Snowflake>,
}

impl PackEmoji {
    /// The emoji as the library holds a custom one. A pack's belongs to no
    /// server, so its `guild_id` is 0, which no channel's server is.
    pub fn guild_emoji(&self) -> GuildEmoji {
        GuildEmoji {
            id: self.id,
            guild_id: Snowflake(0),
            name: self.name.clone(),
            animated: self.animated,
            available: self.available,
            managed: self.managed,
            roles: self.roles.clone(),
        }
    }
}

/// A pack.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub id: Snowflake,
    pub name: String,
    pub description: Option<String>,
    /// Discord's say on whether the account may use it now; an unavailable
    /// pack is listed but its emoji can't be picked.
    pub available: bool,
    pub representative_emoji: Option<Snowflake>,
    pub emojis: Vec<PackEmoji>,
}

impl Pack {
    fn from_value(value: &Value) -> Option<Pack> {
        let id = |v: &Value| v.as_str()?.parse().ok().map(Snowflake);
        Some(Pack {
            id: id(&value["id"])?,
            name: value["name"].as_str()?.to_owned(),
            description: value["description"].as_str().map(str::to_owned),
            available: value["available"] == true,
            representative_emoji: id(&value["representative_emoji_id"]),
            emojis: value["emojis"]
                .as_array()?
                .iter()
                .filter_map(|e| {
                    Some(PackEmoji {
                        id: id(&e["id"])?,
                        name: e["name"].as_str()?.to_owned(),
                        animated: e["animated"] == true,
                        available: e["available"] == true,
                        managed: e["managed"] == true,
                        roles: e["roles"].as_array().into_iter().flatten().filter_map(id).collect(),
                    })
                })
                .collect(),
        })
    }

    /// The emoji we may use: only an available pack's available ones.
    fn usable(&self) -> impl Iterator<Item = &PackEmoji> {
        self.emojis.iter().filter(|_| self.available).filter(|e| e.available)
    }
}

/// Where the fetch stands (`fetchState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fetch {
    #[default]
    NotFetched,
    Fetching,
    Fetched,
    /// It failed for a reason other than a 404, and isn't tried again this session.
    Failed,
}

/// The packs Discord listed for the account.
#[derive(Debug, Default)]
pub struct EmojiPacks {
    packs: Vec<Pack>,
    fetch: Fetch,
}

impl EmojiPacks {
    pub fn fetch(&self) -> Fetch {
        self.fetch
    }

    /// The fetch is to start now, which the client does once per session
    /// (`Fp` in 934499 asks only while not fetched).
    pub fn start_fetch(&mut self) -> bool {
        match self.fetch {
            Fetch::NotFetched => {
                self.fetch = Fetch::Fetching;
                true
            }
            _ => false,
        }
    }

    /// The answer of `GET /emoji-packs`: a list of packs, or null where
    /// Discord answered 404 (packs aren't enabled for the account, so it has
    /// none). The first of an id is the one kept.
    pub fn fetched(&mut self, answer: &Value) {
        let mut packs: Vec<Pack> = Vec::new();
        for pack in answer.as_array().into_iter().flatten().filter_map(Pack::from_value) {
            if !packs.iter().any(|p| p.id == pack.id) {
                packs.push(pack);
            }
        }
        self.packs = packs;
        self.fetch = Fetch::Fetched;
    }

    pub fn failed(&mut self) {
        self.fetch = Fetch::Failed;
    }

    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    pub fn pack(&self, id: Snowflake) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id == id)
    }

    /// The packs of `pinned`, in that order, as far as they are known.
    pub fn pinned(&self, pinned: &[u64]) -> Vec<&Pack> {
        pinned.iter().filter_map(|id| self.pack(Snowflake(*id))).collect()
    }

    /// The emoji `:name:` and the picker's search offer from packs
    /// (`getIndexablePackEmojis`): every available emoji of every available
    /// pack, with the pack it is of, and none while packs are off.
    pub fn indexable(&self, enabled: bool) -> Vec<(&Pack, &PackEmoji)> {
        match enabled {
            true => self.packs.iter().flat_map(|p| p.usable().map(move |e| (p, e))).collect(),
            false => Vec::new(),
        }
    }
}

/// The pinned packs after `pack` is pinned (`Ls` in 934499), or `None` when
/// nothing changes: it is pinned, or it isn't available. Once the packs are
/// fetched, pinned packs that have gone unavailable are dropped with it.
pub fn pinned_with(packs: &EmojiPacks, pinned: &[u64], pack: Snowflake) -> Option<Vec<u64>> {
    if !packs.pack(pack)?.available || pinned.contains(&pack.0) {
        return None;
    }
    let mut ids: Vec<u64> = match packs.fetch() {
        Fetch::Fetched => {
            pinned.iter().copied().filter(|id| packs.pack(Snowflake(*id)).is_some_and(|p| p.available)).collect()
        }
        _ => pinned.to_vec(),
    };
    ids.push(pack.0);
    Some(ids)
}

/// The pinned packs after `pack` is unpinned (`M5`), or `None` when it wasn't pinned.
pub fn pinned_without(pinned: &[u64], pack: Snowflake) -> Option<Vec<u64>> {
    pinned.contains(&pack.0).then(|| pinned.iter().copied().filter(|id| *id != pack.0).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn emoji(id: &str, name: &str, available: bool) -> Value {
        json!({ "id": id, "name": name, "animated": false, "require_colons": true, "available": available, "managed": false, "roles": [] })
    }

    fn answer() -> Value {
        json!([
            { "id": "10", "name": "Cats", "description": "meow", "available": true, "representative_emoji_id": "100",
              "emojis": [emoji("100", "cat", true), emoji("101", "kitten", false)] },
            { "id": "20", "name": "Dogs", "available": false,
              "emojis": [emoji("200", "dog", true)] },
            { "id": "10", "name": "Cats again", "available": true, "emojis": [emoji("100", "cat", true)] },
        ])
    }

    fn fetched() -> EmojiPacks {
        let mut packs = EmojiPacks::default();
        assert!(packs.start_fetch());
        packs.fetched(&answer());
        packs
    }

    #[test]
    fn packs_read_as_listed_and_the_first_of_an_id_stays() {
        let packs = fetched();
        assert_eq!(packs.fetch(), Fetch::Fetched);
        assert_eq!(packs.packs().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Cats", "Dogs"]);
        let cats = packs.pack(Snowflake(10)).unwrap();
        assert_eq!(cats.description.as_deref(), Some("meow"));
        assert_eq!(cats.representative_emoji, Some(Snowflake(100)));
        assert_eq!(packs.pack(Snowflake(20)).unwrap().description, None);
    }

    #[test]
    fn it_is_fetched_once_and_a_404_is_an_empty_list() {
        let mut packs = EmojiPacks::default();
        assert!(packs.start_fetch());
        assert!(!packs.start_fetch(), "one is out");
        packs.fetched(&Value::Null);
        assert_eq!(packs.fetch(), Fetch::Fetched);
        assert!(!packs.start_fetch(), "and not again");
        assert!(packs.packs().is_empty());

        let mut failed = EmojiPacks::default();
        failed.start_fetch();
        failed.failed();
        assert_eq!(failed.fetch(), Fetch::Failed);
        assert!(!failed.start_fetch(), "a failure isn't tried again");
    }

    #[test]
    fn only_available_emoji_of_available_packs_are_indexed_and_only_while_on() {
        let packs = fetched();
        let names = |on| packs.indexable(on).iter().map(|(_, e)| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(true), ["cat"], "kitten is unavailable and dog's pack is");
        assert!(names(false).is_empty(), "packs off");
    }

    #[test]
    fn a_pinned_pack_is_listed_in_the_order_pinned_and_unknown_ones_are_skipped() {
        let packs = fetched();
        assert_eq!(packs.pinned(&[20, 99, 10]).iter().map(|p| p.id.0).collect::<Vec<_>>(), [20, 10]);
    }

    #[test]
    fn pinning_adds_to_the_end_only_an_available_pack_and_drops_the_gone_ones() {
        let packs = fetched();
        assert_eq!(pinned_with(&packs, &[], Snowflake(10)), Some(vec![10]));
        assert_eq!(pinned_with(&packs, &[10], Snowflake(10)), None, "already");
        assert_eq!(pinned_with(&packs, &[], Snowflake(20)), None, "not available");
        assert_eq!(pinned_with(&packs, &[], Snowflake(99)), None, "unknown");
        // Once fetched, a pinned pack that isn't available any more goes with the new one.
        let mut two = fetched();
        two.packs.push(Pack { id: Snowflake(30), available: true, emojis: vec![], ..two.packs[0].clone() });
        assert_eq!(pinned_with(&two, &[20, 5], Snowflake(30)), Some(vec![30]));
        // Before it is, nothing is judged.
        assert_eq!(
            pinned_with(&EmojiPacks { fetch: Fetch::Fetching, ..two }, &[20], Snowflake(30)),
            Some(vec![20, 30])
        );
    }

    #[test]
    fn unpinning_removes_it_or_changes_nothing() {
        assert_eq!(pinned_without(&[1, 2, 3], Snowflake(2)), Some(vec![1, 3]));
        assert_eq!(pinned_without(&[1, 3], Snowflake(2)), None);
    }

    #[test]
    fn the_experiment_is_variant_one() {
        let assigned = |variant: i64| {
            Experiments::from_ready(&json!({"apex_experiments": {"assignments": {"1": {"7": {"assignments": [
                [crate::experiments::murmur3(EXPERIMENT), variant, 0, 1]
            ]}}}}}))
        };
        let me = Snowflake(7);
        assert!(!enabled(&Experiments::default(), me));
        assert!(!enabled(&assigned(0), me), "control");
        assert!(enabled(&assigned(1), me));
        assert!(!enabled(&assigned(1), Snowflake(8)), "someone else's");
    }
}
