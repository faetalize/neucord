//! How often and how recently the account uses things, which the client
//! keeps as a "frecency" per kind of thing and saves in the settings proto
//! of type 2 (see [`crate::frecency`]).
//!
//! [`History`] is the client's frecency class (283047): a use is counted and
//! its time kept, the last 10 times at most; a score weighs the recent uses
//! by their age; the count of use scaled by that score ranks. A use is also
//! kept as pending (`pendingUsages`, which the client persists) until a save
//! of the proto carries it, and the history is what the account's proto
//! says plus what is pending.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Snowflake;
use crate::commands::{self, Command, Used, command_type};
use crate::protobuf::{Value, build, int32, packed};

const DAY_MS: i64 = 86_400_000;

/// How often and when something was used: a `FrecencyItem`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub total_uses: u64,
    /// Milliseconds since 1970, in the order sent.
    pub recent_uses: Vec<u64>,
}

/// The kinds of use Neucord records: what the client records on surfaces
/// Neucord has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An emoji sent in a message or reacted with, by id, or a Unicode
    /// emoji's name (`EmojiStore`'s `eA`, 121681).
    Emoji,
    /// An emoji reacted with (`ef`).
    Reaction,
    /// A sticker sent (931959).
    Sticker,
    /// A server or channel opened, by id (205761).
    Channel,
    /// A command run, by [`crate::commands::usage_key`]
    /// (`ApplicationCommandFrecencyStore`, 26909).
    Command,
    /// An app whose slash command was run, by id (`ApplicationFrecencyStore`,
    /// 630248).
    Application,
}

/// The most recent uses kept of each thing, and scored.
const MAX_SAMPLES: usize = 10;

impl Kind {
    /// The most uses the client lists (`numFrequentlyItems`).
    fn listed(self) -> usize {
        match self {
            Kind::Emoji | Kind::Reaction => 42,
            Kind::Sticker => 20,
            Kind::Channel | Kind::Command | Kind::Application => 100,
        }
    }

    /// What a use's weight is multiplied by (`computeBonus` over 100): a
    /// command's store has a bonus of 1 where every other has 100, so its
    /// scores are a hundredth of the others'.
    fn bonus(self) -> f64 {
        match self {
            Kind::Command => 1.0 / 100.0,
            _ => 1.0,
        }
    }

    /// The most entries a save carries (`Mr`, 761821, as 416380 calls it).
    fn saved(self) -> usize {
        match self {
            Kind::Command => 500,
            _ => 100,
        }
    }

    /// What a use counts for by the whole days since (`computeWeight`).
    fn weight(self, days: i64) -> i64 {
        match self {
            Kind::Channel => match days {
                0 => 100,
                1 => 70,
                2..=3 => 50,
                4..=6 => 30,
                7.. => 10,
                _ => 1,
            },
            _ => match days {
                ..=3 => 100,
                4..=15 => 70,
                16..=30 => 50,
                31..=45 => 30,
                46..=80 => 10,
                _ => 1,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    total_uses: u64,
    recent_uses: Vec<u64>,
    /// `-1` until computed.
    frecency: i64,
    score: f64,
}

/// What was used and how often: the client's frecency class.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    kind: Kind,
    /// In the order they were first used or sent.
    entries: Vec<(String, Entry)>,
}

impl History {
    pub fn new(kind: Kind) -> Self {
        History { kind, entries: Vec::new() }
    }

    /// A use of `key` at `at`. With `sorted` the times are put in order,
    /// which the client does for the uses it is told the time of
    /// (`track` with a timestamp) and not for a use that is now.
    fn track(&mut self, key: &str, at: u64, sorted: bool) {
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            None => self
                .entries
                .push((key.to_owned(), Entry { total_uses: 1, recent_uses: vec![at], frecency: -1, score: 0.0 })),
            Some((_, entry)) => {
                entry.frecency = -1;
                entry.total_uses += 1;
                entry.recent_uses.push(at);
                if sorted {
                    entry.recent_uses.sort_unstable();
                }
                let extra = entry.recent_uses.len().saturating_sub(MAX_SAMPLES);
                entry.recent_uses.drain(..extra);
            }
        }
    }

    /// Start over from what the account has, then count the uses not saved
    /// yet (`overwriteHistory`).
    fn overwrite(&mut self, saved: &[(String, Usage)], pending: &[(String, u64)]) {
        self.entries = saved
            .iter()
            .map(|(key, usage)| {
                let recent_uses = usage.recent_uses.iter().copied().filter(|&at| at > 0).collect();
                (key.clone(), Entry { total_uses: usage.total_uses, recent_uses, frecency: -1, score: 0.0 })
            })
            .collect();
        for (key, at) in pending {
            self.track(key, *at, true);
        }
    }

    /// Score what has no score, and forget what scores nothing (`compute`).
    fn compute(&mut self, now_ms: u64) {
        let kind = self.kind;
        let bonus = kind.bonus();
        let max_total = self.entries.iter().map(|(_, e)| e.total_uses).max();
        self.entries.retain_mut(|(_, entry)| {
            if entry.frecency != -1 {
                return true;
            }
            // Added up use by use, in floats, as the client does: a hundredth
            // at a time doesn't sum to what the whole would.
            entry.score = entry
                .recent_uses
                .iter()
                .take(MAX_SAMPLES)
                .fold(0.0, |score, &at| score + bonus * kind.weight((now_ms as i64 - at as i64) / DAY_MS) as f64);
            if entry.score <= 0.0 {
                return false;
            }
            entry.frecency = match (kind, max_total) {
                // `Math.trunc(1000 * (total / max * .2 + score / 1000 * .8))`.
                (Kind::Reaction, Some(max)) => {
                    (1000.0 * (entry.total_uses as f64 / max as f64 * 0.2 + entry.score / 1000.0 * 0.8)) as i64
                }
                (Kind::Reaction, None) => 0,
                // `Math.ceil(total * (score / recent uses))`.
                _ => (entry.total_uses as f64 * (entry.score / entry.recent_uses.len() as f64)).ceil() as i64,
            };
            true
        });
    }

    /// The keys used most, best first, that `known` finds: as many as the
    /// kind lists.
    pub fn frequently(&mut self, now_ms: u64, known: impl Fn(&str) -> bool) -> Vec<String> {
        self.compute(now_ms);
        let mut ranked: Vec<&(String, Entry)> = self.entries.iter().filter(|(key, _)| known(key)).collect();
        // Stable, so equals stay in the order they came.
        ranked.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.frecency));
        ranked.into_iter().take(self.kind.listed()).map(|(key, _)| key.clone()).collect()
    }

    /// The score of everything used (`getScore`).
    pub fn scores(&mut self, now_ms: u64) -> HashMap<String, f64> {
        self.compute(now_ms);
        self.entries.iter().map(|(key, entry)| (key.clone(), entry.score)).collect()
    }

    /// What a save carries: at most as many entries as the kind saves, the
    /// ones used last if there are more (`Mr`), as `FrecencyItem`s by key.
    fn saved(&mut self, now_ms: u64) -> Vec<(&str, Vec<u8>)> {
        self.compute(now_ms);
        let saved = self.kind.saved();
        let mut kept: Vec<&(String, Entry)> = self.entries.iter().collect();
        if kept.len() > saved {
            // lodash's `sortBy` puts what has no use last, and the order is then turned around.
            kept.sort_by_key(|(_, e)| e.recent_uses.last().map_or((1, 0), |&at| (0, at)));
            kept.reverse();
            kept.truncate(saved);
        }
        kept.into_iter()
            .map(|(key, entry)| {
                let recent: Vec<u64> = entry.recent_uses.iter().copied().filter(|&at| at > 0).collect();
                let mut fields = Vec::new();
                if entry.total_uses > 0 {
                    fields.push((1, Value::Varint(entry.total_uses)));
                }
                if !recent.is_empty() {
                    fields.push((2, packed(&recent)));
                }
                if entry.frecency != 0 {
                    fields.push((3, int32(entry.frecency as i32)));
                }
                // `Math.round`.
                let score = (entry.score + 0.5).floor() as i32;
                if score != 0 {
                    fields.push((4, int32(score)));
                }
                (key.as_str(), build(&fields))
            })
            .collect()
    }
}

/// A history and the uses not yet saved.
#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    pub history: History,
    /// What was used, and when, in order. The client persists these.
    pending: Vec<(String, u64)>,
}

impl Store {
    pub fn new(kind: Kind) -> Self {
        Store { history: History::new(kind), pending: Vec::new() }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn pending(&self) -> &[(String, u64)] {
        &self.pending
    }

    /// `key` was used.
    pub fn track(&mut self, key: &str, now_ms: u64) {
        self.history.track(key, now_ms, false);
        self.pending.push((key.to_owned(), now_ms));
    }

    /// The account's settings now say `saved`: the history is that and what
    /// is pending, or the `seeded` uses where there is none of either.
    pub fn reload(&mut self, saved: &[(String, Usage)], seeded: &[&str], now_ms: u64) {
        self.history.overwrite(saved, &self.pending);
        if saved.is_empty() && self.pending.is_empty() {
            for key in seeded {
                self.history.track(key, now_ms, false);
            }
        }
    }

    /// A save carrying the first `sent` pending uses was taken. The client
    /// forgets every pending use, which loses one made while it was on its
    /// way.
    fn sent(&mut self, sent: usize) {
        self.pending.drain(..sent.min(self.pending.len()));
    }

    /// The map message a save carries; `ids` says keys are ids, which are
    /// written as `fixed64`, not strings.
    fn message(&mut self, now_ms: u64, ids: bool) -> Vec<u8> {
        let entries = self.history.saved(now_ms);
        let mut out = Vec::new();
        for (key, item) in entries {
            let key = match (ids, key.parse::<u64>()) {
                (true, Ok(id)) => Value::Fixed64(id),
                (true, Err(_)) => continue,
                (false, _) => Value::Bytes(key.as_bytes().to_vec()),
            };
            let entry = build(&[(1, key), (2, Value::Bytes(item))]);
            out.extend(build(&[(1, Value::Bytes(entry))]));
        }
        out
    }
}

/// The uses not yet saved, which survive the app closing so a later session
/// sends them (the client keeps them in its local storage).
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    emoji: Vec<(String, u64)>,
    reactions: Vec<(String, u64)>,
    stickers: Vec<(String, u64)>,
    channels: Vec<(String, u64)>,
    // A file written before commands were counted has none.
    #[serde(default)]
    commands: Vec<(String, u64)>,
    #[serde(default)]
    applications: Vec<(String, u64)>,
}

/// Every kind of use recorded, and the fields of `FrecencyUserSettings` a
/// save puts them in.
#[derive(Clone, Debug, PartialEq)]
pub struct Stores {
    pub emoji: Store,
    pub reactions: Store,
    pub stickers: Store,
    pub channels: Store,
    pub commands: Store,
    pub applications: Store,
}

impl Default for Stores {
    fn default() -> Self {
        Stores {
            emoji: Store::new(Kind::Emoji),
            reactions: Store::new(Kind::Reaction),
            stickers: Store::new(Kind::Sticker),
            channels: Store::new(Kind::Channel),
            commands: Store::new(Kind::Command),
            applications: Store::new(Kind::Application),
        }
    }
}

// FrecencyUserSettings.
const STICKER_FRECENCY: u32 = 4;
const EMOJI_FRECENCY: u32 = 6;
const APPLICATION_COMMAND_FRECENCY: u32 = 7;
const APPLICATION_FRECENCY: u32 = 9;
const GUILD_AND_CHANNEL_FRECENCY: u32 = 12;
const EMOJI_REACTION_FRECENCY: u32 = 13;

/// How many uses of each kind a save carried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    emoji: usize,
    reactions: usize,
    stickers: usize,
    channels: usize,
    commands: usize,
    applications: usize,
}

impl Stores {
    /// Whether any use waits for a save.
    pub fn has_pending(&self) -> bool {
        self.emoji.has_pending()
            || self.reactions.has_pending()
            || self.stickers.has_pending()
            || self.channels.has_pending()
            || self.commands.has_pending()
            || self.applications.has_pending()
    }

    /// Whether a use other than a server or channel opened waits: those
    /// alone don't make the first save load the settings (`R`, 416380).
    pub fn has_pending_beyond_channels(&self) -> bool {
        self.emoji.has_pending()
            || self.reactions.has_pending()
            || self.stickers.has_pending()
            || self.commands.has_pending()
            || self.applications.has_pending()
    }

    /// `command` was run where `guild` is (`APPLICATION_COMMAND_USED`):
    /// the command counts, and for a slash command its app too (630248
    /// counts `CHAT` and `PRIMARY_ENTRY_POINT` commands of an app not
    /// launching, which Neucord never is), a built-in's being `-1`.
    pub fn command_used(&mut self, command: &Command, guild: Option<Snowflake>, now_ms: u64) {
        self.commands.track(&commands::usage_key(command, guild), now_ms);
        if matches!(command.kind, command_type::CHAT | command_type::PRIMARY_ENTRY_POINT) {
            let app = if command.is_built_in() { "-1".to_owned() } else { command.application.0.to_string() };
            self.applications.track(&app, now_ms);
        }
    }

    /// How much each command is used, for ordering them.
    pub fn command_use(&mut self, now_ms: u64) -> Used {
        let history = &mut self.commands.history;
        Used { scores: history.scores(now_ms), top: history.frequently(now_ms, |_| true) }
    }

    /// The messages a save carries for what is pending, by field of
    /// `FrecencyUserSettings`, each holding the whole history of its kind
    /// (the `beforeSendCallbacks` of 416380). Emoji and reaction use go
    /// together, as the client has them. Says how much was pending of each,
    /// for [`Self::saved`].
    pub fn before_send(&mut self, now_ms: u64) -> (Vec<(u32, Vec<u8>)>, Sent) {
        let sent = Sent {
            emoji: self.emoji.pending.len(),
            reactions: self.reactions.pending.len(),
            stickers: self.stickers.pending.len(),
            channels: self.channels.pending.len(),
            commands: self.commands.pending.len(),
            applications: self.applications.pending.len(),
        };
        let mut groups = Vec::new();
        if self.stickers.has_pending() {
            groups.push((STICKER_FRECENCY, self.stickers.message(now_ms, true)));
        }
        if self.emoji.has_pending() || self.reactions.has_pending() {
            groups.push((EMOJI_FRECENCY, self.emoji.message(now_ms, false)));
            groups.push((EMOJI_REACTION_FRECENCY, self.reactions.message(now_ms, false)));
        }
        if self.channels.has_pending() {
            groups.push((GUILD_AND_CHANNEL_FRECENCY, self.channels.message(now_ms, true)));
        }
        // Both keyed by strings: a sub-command's key holds its path.
        if self.commands.has_pending() {
            groups.push((APPLICATION_COMMAND_FRECENCY, self.commands.message(now_ms, false)));
        }
        if self.applications.has_pending() {
            groups.push((APPLICATION_FRECENCY, self.applications.message(now_ms, false)));
        }
        groups.sort_by_key(|(number, _)| *number);
        (groups, sent)
    }

    /// Discord took a save that carried `sent`.
    pub fn saved(&mut self, sent: Sent) {
        self.emoji.sent(sent.emoji);
        self.reactions.sent(sent.reactions);
        self.stickers.sent(sent.stickers);
        self.channels.sent(sent.channels);
        self.commands.sent(sent.commands);
        self.applications.sent(sent.applications);
    }

    pub fn pending_json(&self) -> String {
        let persisted = Persisted {
            emoji: self.emoji.pending.clone(),
            reactions: self.reactions.pending.clone(),
            stickers: self.stickers.pending.clone(),
            channels: self.channels.pending.clone(),
            commands: self.commands.pending.clone(),
            applications: self.applications.pending.clone(),
        };
        serde_json::to_string(&persisted).unwrap_or_default()
    }

    /// Take back what [`Self::pending_json`] kept. A channel use must be an
    /// id, as the client checks when it loads them (`Ut1`).
    pub fn restore_pending(&mut self, json: &str) {
        let Ok(persisted) = serde_json::from_str::<Persisted>(json) else { return };
        let valid =
            |items: Vec<(String, u64)>| -> Vec<(String, u64)> { items.into_iter().filter(|(_, at)| *at > 0).collect() };
        self.emoji.pending = valid(persisted.emoji);
        self.reactions.pending = valid(persisted.reactions);
        self.stickers.pending = valid(persisted.stickers);
        self.channels.pending =
            valid(persisted.channels).into_iter().filter(|(key, _)| key.parse::<u64>().is_ok()).collect();
        self.commands.pending = valid(persisted.commands);
        self.applications.pending = valid(persisted.applications);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;
    const NOW: u64 = 1_000_000 * DAY;

    fn usage(total: u64, recent: &[u64]) -> Usage {
        Usage { total_uses: total, recent_uses: recent.to_vec() }
    }

    fn list(kind: Kind, entries: Vec<(String, Usage)>) -> Vec<String> {
        let mut history = History::new(kind);
        history.overwrite(&entries, &[]);
        history.frequently(NOW, |_| true)
    }

    #[test]
    fn a_use_keeps_the_last_ten_times_and_counts_them_all() {
        let mut history = History::new(Kind::Emoji);
        for n in 0..12 {
            history.track("a", 100 + n, false);
        }
        let (_, entry) = &history.entries[0];
        assert_eq!(entry.total_uses, 12);
        assert_eq!(entry.recent_uses, (102..112).collect::<Vec<u64>>(), "the oldest go");
        // A use told its time is put in order; one that is now is put last.
        history.track("a", 5, true);
        assert_eq!(history.entries[0].1.recent_uses[0], 102, "5 sorted first, and fell off");
        history.track("b", 50, true);
        history.track("b", 40, true);
        assert_eq!(history.entries[1].1.recent_uses, [40, 50]);
        history.track("c", 50, false);
        history.track("c", 40, false);
        assert_eq!(history.entries[2].1.recent_uses, [50, 40]);
    }

    #[test]
    fn ranked_by_recent_weight_and_how_often() {
        let usage = vec![
            ("old".to_owned(), usage(50, &[NOW - 100 * DAY])),
            ("often".to_owned(), usage(10, &[NOW - DAY, NOW - 2 * DAY])),
            ("gone".to_owned(), usage(9, &[])),
            ("new".to_owned(), usage(3, &[NOW])),
        ];
        // often: 10 × (100 + 100) / 2 = 1000; new: 3 × 100 = 300; old: 50 × 1 = 50.
        assert_eq!(list(Kind::Emoji, usage), ["often", "new", "old"], "what has no use is dropped");
    }

    #[test]
    fn a_use_weighs_by_whole_days_and_only_ten_count() {
        let at = |days: u64| {
            vec![("e".to_owned(), usage(1, &[NOW - days * DAY])), ("r".to_owned(), usage(1, &[NOW - 4 * DAY]))]
        };
        // The rival's 4 days weigh 70.
        assert_eq!(list(Kind::Emoji, at(3))[0], "e", "3 days is 100");
        assert_eq!(list(Kind::Emoji, at(5))[0], "e", "5 days is 70 too, a tie: the order held");
        assert_eq!(list(Kind::Emoji, at(16))[0], "r", "16 days is 50");
        let many = vec![("m".to_owned(), usage(2, &[NOW; 12]))];
        assert_eq!(list(Kind::Emoji, many), ["m"]);
    }

    #[test]
    fn a_server_or_channel_weighs_its_days_its_own_way() {
        let at = |days: u64| {
            vec![("1".to_owned(), usage(1, &[NOW - days * DAY])), ("2".to_owned(), usage(1, &[NOW - 2 * DAY]))]
        };
        // The rival's 2 days weigh 50 (for an emoji, 100).
        assert_eq!(list(Kind::Channel, at(1))[0], "1", "1 day is 70");
        assert_eq!(list(Kind::Channel, at(3)), ["1", "2"], "2 and 3 days are a tie");
        assert_eq!(list(Kind::Channel, at(4))[0], "2", "4 days is 30");
        assert_eq!(list(Kind::Emoji, at(4))[0], "2", "for an emoji 4 days is 70 and 2 days 100");
    }

    #[test]
    fn a_reaction_ranks_by_how_it_compares_to_the_most_used() {
        let usage = vec![("big".to_owned(), usage(100, &[NOW - 100 * DAY])), ("small".to_owned(), usage(10, &[NOW]))];
        // big: 1000 × (1 × .2 + .001 × .8) = 200; small: 1000 × (.1 × .2 + .1 × .8) = 100.
        // An emoji ranks by total × score: big 100, small 1000.
        assert_eq!(list(Kind::Reaction, usage.clone()), ["big", "small"]);
        assert_eq!(list(Kind::Emoji, usage), ["small", "big"]);
    }

    #[test]
    fn only_what_is_known_is_listed_and_listing_stops_at_the_kinds_count() {
        let many: Vec<(String, Usage)> = (0..60).map(|n| (n.to_string(), usage(1, &[NOW]))).collect();
        let mut history = History::new(Kind::Emoji);
        history.overwrite(&many, &[]);
        assert_eq!(history.frequently(NOW, |k| k != "0").len(), 42);
        let mut stickers = History::new(Kind::Sticker);
        stickers.overwrite(&many, &[]);
        assert_eq!(stickers.frequently(NOW, |_| true).len(), 20);
    }

    #[test]
    fn a_score_is_the_sum_of_what_the_recent_uses_weigh() {
        let mut history = History::new(Kind::Emoji);
        history.overwrite(&[("a".to_owned(), usage(9, &[NOW, NOW - 5 * DAY, NOW - 100 * DAY]))], &[]);
        assert_eq!(history.scores(NOW)["a"], 171.0, "100 + 70 + 1");
    }

    #[test]
    fn what_is_pending_is_counted_over_what_the_account_has() {
        let mut store = Store::new(Kind::Emoji);
        store.track("a", NOW);
        store.reload(&[("a".to_owned(), usage(4, &[NOW - DAY]))], &[], NOW);
        assert_eq!(store.history.entries[0].1.total_uses, 5);
        assert_eq!(store.history.entries[0].1.recent_uses, [NOW - DAY, NOW]);
        store.sent(1);
        assert!(!store.has_pending());
    }

    #[test]
    fn an_account_with_no_use_starts_with_the_seeded_ones() {
        let mut store = Store::new(Kind::Emoji);
        store.reload(&[], &["thumbsup", "eyes"], NOW);
        assert_eq!(store.history.frequently(NOW, |_| true), ["thumbsup", "eyes"]);
        assert!(!store.has_pending(), "never saved");
        store.track("x", NOW);
        store.reload(&[], &["thumbsup", "eyes"], NOW);
        assert_eq!(store.history.frequently(NOW, |_| true), ["x"], "none once something was used");
        let mut reactions = Store::new(Kind::Reaction);
        reactions.reload(&[], &["100", "100", "thumbsup"], NOW);
        assert_eq!(reactions.history.entries[0].1.total_uses, 2, "each listing is a use");
    }

    #[test]
    fn a_command_counts_a_hundredth_of_an_emoji() {
        let three = vec![("k".to_owned(), usage(3, &[NOW - 5 * DAY; 3])), ("rival".to_owned(), usage(1, &[NOW]))];
        let mut commands = History::new(Kind::Command);
        commands.overwrite(&three, &[]);
        assert_eq!(commands.scores(NOW)["k"], 2.1, "3 × 70 × 0.01");
        // Frecency ceil(3 × 2.1 / 3) = 3 beats the rival's ceil(1 × 1 / 1) = 1.
        assert_eq!(commands.frequently(NOW, |_| true), ["k", "rival"]);
        let item = build(&[(1, Value::Varint(3)), (2, packed(&[NOW - 5 * DAY; 3])), (3, int32(3)), (4, int32(2))]);
        assert_eq!(commands.saved(NOW)[0], ("k", item));
        let mut emoji = History::new(Kind::Emoji);
        emoji.overwrite(&three, &[]);
        assert_eq!(emoji.scores(NOW)["k"], 210.0);
    }

    #[test]
    fn a_command_score_adds_up_as_the_client_adds_it() {
        let mut commands = History::new(Kind::Command);
        commands.overwrite(&[("k".to_owned(), usage(10, &[NOW - 5 * DAY; 10]))], &[]);
        assert_eq!(commands.scores(NOW)["k"], 7.000000000000001, "summed a hundredth at a time");
        let saved = commands.saved(NOW);
        let item = build(&[(1, Value::Varint(10)), (2, packed(&[NOW - 5 * DAY; 10])), (3, int32(8)), (4, int32(7))]);
        assert_eq!(saved[0].1, item, "frecency ceil(10 × 7.000000000000001 / 10) = 8");
    }

    #[test]
    fn a_slash_command_counts_for_its_app_and_a_user_command_doesnt() {
        let mut stores = Stores::default();
        stores.command_used(&Command::made_up("910", command_type::CHAT, 900, None), Some(Snowflake(500)), NOW);
        stores.command_used(&Command::made_up("913", command_type::CHAT, 900, Some("500")), Some(Snowflake(500)), NOW);
        stores.command_used(&Command::made_up("916", command_type::USER, 900, None), None, NOW);
        stores.command_used(&Command::made_up("917", command_type::MESSAGE, 700, None), None, NOW);
        let keys = |store: &Store| store.pending().iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&stores.commands), ["910", "913:500", "916", "917"]);
        assert_eq!(keys(&stores.applications), ["900", "900"]);
        let mut shrug = Command::made_up("-1", command_type::CHAT, 0, None);
        shrug.builtin = Some(crate::builtins::BuiltIn::Shrug);
        stores.command_used(&shrug, Some(Snowflake(500)), NOW);
        assert_eq!(keys(&stores.applications)[2], "-1", "a built-in's app");
        let used = stores.command_use(NOW);
        assert_eq!(used.scores["913:500"], 1.0, "used today: 100 × 0.01");
        assert_eq!(used.top.len(), 5);
    }

    #[test]
    fn a_save_keeps_the_five_hundred_commands_used_last() {
        let many = |n: u64| -> Vec<(String, Usage)> {
            (1..=n).map(|k| (k.to_string(), usage(1, &[NOW - 1000 + k]))).collect()
        };
        let mut commands = History::new(Kind::Command);
        commands.overwrite(&many(530), &[]);
        let saved = commands.saved(NOW);
        assert_eq!(saved.len(), 500);
        assert!(saved.iter().all(|(key, _)| key.parse::<u64>().unwrap() > 30), "the 30 used first go");
        let mut apps = History::new(Kind::Application);
        apps.overwrite(&many(130), &[]);
        assert_eq!(apps.saved(NOW).len(), 100);
    }

    #[test]
    fn a_command_or_app_use_makes_the_first_look_load() {
        let mut stores = Stores::default();
        stores.commands.track("910", NOW);
        assert!(stores.has_pending_beyond_channels());
        let mut stores = Stores::default();
        stores.applications.track("900", NOW);
        assert!(stores.has_pending_beyond_channels() && stores.has_pending());
    }

    #[test]
    fn a_pending_file_from_before_commands_still_restores() {
        let mut stores = Stores::default();
        stores.restore_pending(r#"{"emoji":[["a",5]],"reactions":[],"stickers":[],"channels":[]}"#);
        assert_eq!(stores.emoji.pending(), [("a".to_owned(), 5)]);
        stores.commands.track("913\0show:500", 6);
        stores.applications.track("900", 7);
        let mut again = Stores::default();
        again.restore_pending(&stores.pending_json());
        assert_eq!(again.commands.pending(), [("913\0show:500".to_owned(), 6)]);
        assert_eq!(again.applications.pending(), [("900".to_owned(), 7)]);
    }

    #[test]
    fn a_save_carries_the_whole_history_of_what_was_used() {
        let mut stores = Stores::default();
        stores.emoji.track("thumbsup", NOW);
        stores.reactions.track("thumbsup", NOW);
        stores.channels.track("123", NOW);
        stores.commands.track("123\0set:500", NOW);
        stores.applications.track("900", NOW);
        let (groups, sent) = stores.before_send(NOW);
        let numbers: Vec<u32> = groups.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            numbers,
            [
                EMOJI_FRECENCY,
                APPLICATION_COMMAND_FRECENCY,
                APPLICATION_FRECENCY,
                GUILD_AND_CHANNEL_FRECENCY,
                EMOJI_REACTION_FRECENCY
            ]
        );
        // One use, its time, a frecency of 100 and its score.
        let item = build(&[(1, Value::Varint(1)), (2, packed(&[NOW])), (3, int32(100)), (4, int32(100))]);
        let map = |key: Value| {
            let entry = build(&[(1, key), (2, Value::Bytes(item.clone()))]);
            build(&[(1, Value::Bytes(entry))])
        };
        assert_eq!(groups[0].1, map(Value::Bytes(b"thumbsup".to_vec())));
        assert_eq!(groups[3].1, map(Value::Fixed64(123)), "a channel's key is an id");
        // A command's use weighs a hundredth: score 1, frecency 1.
        let command = build(&[(1, Value::Varint(1)), (2, packed(&[NOW])), (3, int32(1)), (4, int32(1))]);
        let entry = build(&[(1, Value::Bytes(b"123\0set:500".to_vec())), (2, Value::Bytes(command))]);
        assert_eq!(groups[1].1, build(&[(1, Value::Bytes(entry))]), "a command's key is a string, NUL and all");
        assert_eq!(groups[2].1, map(Value::Bytes(b"900".to_vec())));
        stores.saved(sent);
        assert!(!stores.has_pending());
        assert!(stores.before_send(NOW).0.is_empty(), "nothing pending, nothing sent");
        assert_eq!(stores.emoji.history.frequently(NOW, |_| true), ["thumbsup"], "the history stays");
    }

    #[test]
    fn a_save_keeps_the_hundred_used_last() {
        let many: Vec<(String, Usage)> = (1..=130).map(|n| (n.to_string(), usage(1, &[NOW - 1000 + n]))).collect();
        let mut history = History::new(Kind::Emoji);
        history.overwrite(&many, &[]);
        let saved = history.saved(NOW);
        assert_eq!(saved.len(), 100);
        assert!(saved.iter().all(|(key, _)| key.parse::<u64>().unwrap() > 30), "the 30 used first go");
    }

    #[test]
    fn a_use_made_while_a_save_was_on_its_way_stays_pending() {
        let mut stores = Stores::default();
        stores.emoji.track("a", NOW);
        let (_, sent) = stores.before_send(NOW);
        stores.emoji.track("b", NOW + 1);
        stores.saved(sent);
        assert_eq!(stores.emoji.pending(), [("b".to_owned(), NOW + 1)]);
    }

    #[test]
    fn what_was_pending_when_the_app_closed_comes_back() {
        let mut stores = Stores::default();
        stores.emoji.track("a", 5);
        stores.channels.track("77", 6);
        stores.channels.track("not an id", 7);
        let json = stores.pending_json();
        let mut again = Stores::default();
        again.restore_pending(&json);
        assert_eq!(again.emoji.pending(), [("a".to_owned(), 5)]);
        assert_eq!(again.channels.pending(), [("77".to_owned(), 6)], "only ids come back");
        again.restore_pending("not json");
        assert!(again.emoji.has_pending(), "a file that does not read leaves it");
    }
}
