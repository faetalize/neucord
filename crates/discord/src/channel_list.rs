//! Which channels the sidebar lists: folded categories and Hide Muted
//! Channels, as the client's channel list decides (`computeState` in 791513).

use std::collections::HashSet;

use serde_json::Value;

use crate::Snowflake;

/// The categories that are folded. The client keeps them in its own store
/// (924985): read from the `collapsed` of the channel overrides when it
/// connects and never from a later settings event, and saved with the next
/// settings request, or fifteen seconds after the last fold (931260).
#[derive(Clone, Debug, Default)]
pub struct Collapsed {
    now: HashSet<Snowflake>,
    /// What Discord has, as of the last request we sent.
    saved: HashSet<Snowflake>,
}

impl Collapsed {
    /// From READY's `user_guild_settings`: a plain array, or `{entries: [...]}`.
    pub fn from_ready(value: &Value) -> Self {
        let entries = value.get("entries").unwrap_or(value);
        let now: HashSet<Snowflake> = entries
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|entry| entry["channel_overrides"].as_array().into_iter().flatten())
            .filter(|o| o["collapsed"] == true)
            .filter_map(|o| o["channel_id"].as_str()?.parse().ok().map(Snowflake))
            .collect();
        Collapsed { saved: now.clone(), now }
    }

    pub fn contains(&self, category: Snowflake) -> bool {
        self.now.contains(&category)
    }

    /// Fold a category, or unfold it. Says whether that changed anything.
    pub fn set(&mut self, category: Snowflake, folded: bool) -> bool {
        if folded { self.now.insert(category) } else { self.now.remove(&category) }
    }

    /// The categories whose state Discord doesn't have yet, with the state it
    /// should get.
    pub fn unsaved(&self) -> Vec<(Snowflake, bool)> {
        let mut changes: Vec<(Snowflake, bool)> = self
            .now
            .symmetric_difference(&self.saved)
            .map(|&category| (category, self.now.contains(&category)))
            .collect();
        changes.sort_unstable_by_key(|&(category, _)| category);
        changes
    }

    /// The request that carries [`Self::unsaved`] is on its way: the client
    /// counts them saved before it is answered.
    pub fn saving(&mut self) {
        self.saved = self.now.clone();
    }
}

/// What the client looks at to decide whether a channel is listed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// It is open, or the voice channel we're in.
    pub selected: bool,
    /// A thread of it is open.
    pub thread_open: bool,
    /// Threads are listed under it.
    pub has_threads: bool,
    pub mentions: bool,
    /// Its own mute is running, not a category's.
    pub muted: bool,
    /// Its category's mute is running.
    pub category_muted: bool,
    pub unread: bool,
    /// A text, announcement, forum or media channel (`ig` in 95701).
    pub text_like: bool,
    pub vocal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Listed,
    /// Left out because its category is folded, which still shows the category.
    Folded,
    /// Left out because it is muted and muted channels are hidden.
    Hidden,
}

/// Whether a channel is listed (`eA.computeState` in 791513): the channel
/// open, with threads shown or with mentions always is; then muted channels go
/// when they are hidden; then, in a folded category, the muted, voice and
/// read ones.
pub fn row(facts: &Facts, hide_muted: bool, category_folded: bool) -> Row {
    if facts.selected || facts.thread_open || facts.has_threads || facts.mentions {
        Row::Listed
    } else if hide_muted && facts.muted {
        Row::Hidden
    } else if category_folded
        && (facts.muted || facts.category_muted || facts.vocal || (facts.text_like && !facts.unread))
    {
        Row::Folded
    } else {
        Row::Listed
    }
}

/// Whether a category is listed: with a channel, or with one folded away
/// (`shouldShowEmptyCategory`, which it is not for channels hidden as muted).
pub fn category_listed(rows: &[Row]) -> bool {
    rows.iter().any(|&r| r != Row::Hidden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn quiet() -> Facts {
        Facts { text_like: true, ..Facts::default() }
    }

    #[test]
    fn folds_come_from_the_overrides_and_are_saved_as_changes() {
        let mut folds = Collapsed::from_ready(&json!({"entries": [
            {"guild_id": "1", "channel_overrides": [
                {"channel_id": "10", "collapsed": true}, {"channel_id": "11", "collapsed": false}]},
            {"guild_id": "2", "channel_overrides": [{"channel_id": "20", "collapsed": true}]},
            {"guild_id": null, "channel_overrides": null},
        ]}));
        assert!(folds.contains(Snowflake(10)) && folds.contains(Snowflake(20)) && !folds.contains(Snowflake(11)));
        assert!(folds.unsaved().is_empty(), "what Discord told us is saved");

        assert!(folds.set(Snowflake(11), true));
        assert!(!folds.set(Snowflake(11), true), "already folded");
        assert!(folds.set(Snowflake(10), false));
        assert_eq!(folds.unsaved(), [(Snowflake(10), false), (Snowflake(11), true)]);
        folds.saving();
        assert!(folds.unsaved().is_empty());

        // Folding and unfolding before a save leaves nothing to say.
        folds.set(Snowflake(12), true);
        folds.set(Snowflake(12), false);
        assert!(folds.unsaved().is_empty());
        assert!(
            Collapsed::from_ready(&json!([{"channel_overrides": [{"channel_id": "5", "collapsed": true}]}]))
                .contains(Snowflake(5))
        );
    }

    #[test]
    fn a_folded_category_lists_only_what_is_unread() {
        let read = quiet();
        let unread = Facts { unread: true, ..quiet() };
        assert_eq!(row(&read, false, false), Row::Listed);
        assert_eq!(row(&read, false, true), Row::Folded);
        assert_eq!(row(&unread, false, true), Row::Listed);
        assert_eq!(row(&Facts { vocal: true, text_like: false, ..Facts::default() }, false, true), Row::Folded);
        // Muted, even when unread, and under a muted category.
        assert_eq!(row(&Facts { muted: true, ..unread }, false, true), Row::Folded);
        assert_eq!(row(&Facts { category_muted: true, ..unread }, false, true), Row::Folded);
        // A kind the client doesn't call text-like (`ig`) stays.
        assert_eq!(row(&Facts::default(), false, true), Row::Listed);
    }

    #[test]
    fn what_is_open_or_mentions_us_stays_listed() {
        let muted = Facts { muted: true, ..quiet() };
        for stays in [
            Facts { selected: true, ..muted },
            Facts { thread_open: true, ..muted },
            Facts { has_threads: true, ..muted },
            Facts { mentions: true, ..muted },
        ] {
            assert_eq!(row(&stays, true, true), Row::Listed);
        }
        assert_eq!(row(&muted, true, true), Row::Hidden, "hidden before folded");
    }

    #[test]
    fn hiding_muted_channels_leaves_a_muted_categorys_alone() {
        assert_eq!(row(&Facts { muted: true, ..quiet() }, true, false), Row::Hidden);
        assert_eq!(row(&Facts { muted: true, ..quiet() }, false, false), Row::Listed);
        assert_eq!(row(&Facts { category_muted: true, ..quiet() }, true, false), Row::Listed);
    }

    #[test]
    fn a_category_with_only_hidden_channels_goes_too() {
        assert!(category_listed(&[Row::Hidden, Row::Listed]));
        assert!(category_listed(&[Row::Folded]), "folded, but its heading stays to unfold it");
        assert!(!category_listed(&[Row::Hidden, Row::Hidden]));
        assert!(!category_listed(&[]));
    }
}
