//! Reactions: what a message carries, how the gateway's events change it,
//! and what the official client does when a request to add or take one back
//! goes wrong.
//!
//! The counting is `Message`'s in the client (`addReaction`,
//! `addReactionBatch`, `removeReaction` and `removeReactionsForEmoji`,
//! 383233), applied the same to optimistic changes and gateway events: ours
//! is counted once whichever arrives first, so the event that follows our own
//! request changes nothing. The retry rule is the unnamed `S` in 649963.
//!
//! A super reaction (`burst`) is Nitro's. It is read and shown, never sent:
//! see [`Kind`].

use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::model::{Emoji, Message, Snowflake, User};
use crate::{Error, emoji};

/// `type` in a reaction's events and requests (`ReactionTypes`). A vote on a
/// poll is the client's third, which it files as a reaction too; Neucord
/// keeps polls apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Normal,
    /// A super reaction. Only an account with Nitro may send one, so Neucord
    /// sends none, and takes none back.
    Burst,
}

impl Kind {
    /// What an event's `type` says, or `None` for a poll vote. A missing or
    /// unknown number counts as normal, as in the client.
    pub fn of_event(value: Option<u64>) -> Option<Kind> {
        match value {
            Some(1) => Some(Kind::Burst),
            Some(2) => None,
            _ => Some(Kind::Normal),
        }
    }
}

/// One emoji's reactions on a message, as the client keeps them.
#[derive(Clone, Debug, PartialEq)]
pub struct Reaction {
    pub emoji: Emoji,
    /// Normal reactions.
    pub count: u32,
    pub me: bool,
    pub burst_count: u32,
    pub me_burst: bool,
    /// The colours a super reaction is drawn in, as `#rrggbb`.
    pub burst_colors: Vec<String>,
}

/// Someone reacted, or took a reaction back (`MESSAGE_REACTION_ADD` and
/// `_REMOVE`).
#[derive(Clone, Debug, PartialEq)]
pub struct Reacted {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub user_id: Snowflake,
    pub emoji: Emoji,
    pub kind: Kind,
    /// A super reaction's colours.
    pub colors: Vec<String>,
    pub added: bool,
}

/// One of the pills under a message: a super reaction or a normal one.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill<'a> {
    pub emoji: &'a Emoji,
    pub kind: Kind,
    pub count: u32,
    /// Whether it is ours.
    pub me: bool,
}

/// What a reaction looks like in a payload. Discord sends `count_details`,
/// and the client reads the counts from it when it is there (`D` in 432005).
#[derive(Deserialize)]
struct Raw {
    emoji: Emoji,
    #[serde(default)]
    count: i64,
    #[serde(default)]
    count_details: Option<Details>,
    #[serde(default)]
    burst_count: Option<i64>,
    #[serde(default)]
    me: bool,
    #[serde(default)]
    me_burst: bool,
    #[serde(default)]
    burst_colors: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Details {
    #[serde(default)]
    burst: Option<i64>,
    #[serde(default)]
    normal: Option<i64>,
}

impl From<Raw> for Reaction {
    fn from(raw: Raw) -> Reaction {
        let (count, burst) = match raw.count_details {
            Some(details) => (details.normal.unwrap_or(0), details.burst.unwrap_or(0)),
            None => (raw.count, raw.burst_count.unwrap_or(0)),
        };
        Reaction {
            emoji: raw.emoji,
            count: count.clamp(0, u32::MAX.into()) as u32,
            me: raw.me,
            burst_count: burst.clamp(0, u32::MAX.into()) as u32,
            me_burst: raw.me_burst,
            burst_colors: raw.burst_colors.unwrap_or_default(),
        }
    }
}

/// A message's `reactions`. One that doesn't read is left out, not the
/// whole message.
pub(crate) fn list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Reaction>, D::Error> {
    let values = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(values.into_iter().filter_map(|v| Raw::deserialize(v).ok()).map(Reaction::from).collect())
}

impl Emoji {
    /// Whether this is `other`, as the client compares them (`i6`): by id
    /// where `other` has one, else by name among the Unicode ones.
    pub fn same(&self, other: &Emoji) -> bool {
        match other.id {
            Some(id) => self.id == Some(id),
            None => self.id.is_none() && self.name == other.name,
        }
    }

    /// Where a request puts it: `name:id` for a custom emoji, the emoji
    /// itself for a Unicode one.
    pub(crate) fn path(&self) -> String {
        let name = self.name.as_deref().unwrap_or_default();
        match self.id {
            Some(id) => format!("{name}:{id}"),
            None => name.to_owned(),
        }
    }
}

impl From<&emoji::Item> for Emoji {
    /// What a pick of the emoji picker is, as the client's `jq` (815807)
    /// makes it: a custom emoji by its own name, whatever `:name:` calls it
    /// here, and a Unicode one by itself.
    fn from(item: &emoji::Item) -> Emoji {
        match item {
            emoji::Item::Unicode(e) => Emoji { id: None, name: Some(e.glyph.to_owned()), animated: false },
            emoji::Item::Custom(n) => {
                Emoji { id: Some(n.emoji.id), name: Some(n.emoji.name.clone()), animated: n.emoji.animated }
            }
        }
    }
}

impl Message {
    /// The pills under it, in the client's order (`367790`): each emoji's
    /// super reactions, then its normal ones, only those someone made.
    pub fn reaction_pills(&self) -> Vec<Pill<'_>> {
        self.reactions
            .iter()
            .flat_map(|r| {
                let burst = (r.burst_count > 0).then_some(Pill {
                    emoji: &r.emoji,
                    kind: Kind::Burst,
                    count: r.burst_count,
                    me: r.me_burst,
                });
                let normal =
                    (r.count > 0).then_some(Pill { emoji: &r.emoji, kind: Kind::Normal, count: r.count, me: r.me });
                burst.into_iter().chain(normal)
            })
            .collect()
    }

    /// Whether we have reacted with `emoji` the other way than `kind`, which
    /// the client refuses to add on top (`userHasReactedWithEmoji`): a
    /// reaction and a super reaction with the same emoji can't both be ours.
    pub fn reacted_the_other_way(&self, emoji: &Emoji, kind: Kind) -> bool {
        self.reactions.iter().any(|r| {
            r.emoji.same(emoji)
                && match kind {
                    Kind::Burst => r.me,
                    Kind::Normal => r.me_burst,
                }
        })
    }

    /// Someone reacted (`addReaction`). `mine` is whether it was us, `dm`
    /// whether this is a DM, where the other person's reaction is counted
    /// once however often it arrives. `colors` are a super reaction's.
    pub fn add_reaction(&mut self, emoji: &Emoji, mine: bool, kind: Kind, colors: &[String], dm: bool) {
        let Some(found) = self.reactions.iter_mut().find(|r| r.emoji.same(emoji)) else {
            self.reactions.push(match kind {
                Kind::Burst => Reaction {
                    emoji: emoji.clone(),
                    count: 0,
                    me: false,
                    burst_count: 1,
                    me_burst: mine,
                    burst_colors: colors.to_vec(),
                },
                Kind::Normal => Reaction {
                    emoji: emoji.clone(),
                    count: 1,
                    me: mine,
                    burst_count: 0,
                    me_burst: false,
                    burst_colors: Vec::new(),
                },
            });
            return;
        };
        if dm && !mine {
            let others = |count: u32, me: bool| i64::from(count) - i64::from(me) >= 1;
            let counted = match kind {
                Kind::Normal => others(found.count, found.me),
                Kind::Burst => others(found.burst_count, found.me_burst),
            };
            if counted {
                return;
            }
        }
        match kind {
            Kind::Burst if mine && found.me_burst => {}
            Kind::Burst => {
                found.burst_count += 1;
                found.me_burst |= mine;
                if found.burst_colors.is_empty() {
                    found.burst_colors = colors.to_vec();
                }
            }
            Kind::Normal if mine && found.me => {}
            Kind::Normal => {
                found.count += 1;
                found.me |= mine;
            }
        }
    }

    /// Several people reacted at once (`addReactionBatch`, which is
    /// `MESSAGE_REACTION_ADD_MANY`). The client counts each as a normal
    /// reaction, the `type` Discord sends being a name its batch never
    /// reads, and without the DM rule.
    pub fn add_reactions(&mut self, batch: &[(Emoji, Vec<Snowflake>)], me: Option<Snowflake>) {
        for (emoji, users) in batch {
            for user in users {
                self.add_reaction(emoji, me == Some(*user), Kind::Normal, &[], false);
            }
        }
    }

    /// A reaction was taken back (`removeReaction`). Ours may be gone from
    /// the count already, and an emoji left with none has no pill.
    pub fn remove_reaction(&mut self, emoji: &Emoji, mine: bool, kind: Kind) {
        let Some(at) = self.reactions.iter().position(|r| r.emoji.same(emoji)) else { return };
        let found = &mut self.reactions[at];
        // What isn't ours to take back leaves the count where it is.
        let take = |count: u32, ours: bool| if mine && !ours { count } else { count.saturating_sub(1) };
        match kind {
            Kind::Burst => {
                found.burst_count = take(found.burst_count, found.me_burst);
                found.me_burst &= !mine;
            }
            Kind::Normal => {
                found.count = take(found.count, found.me);
                found.me &= !mine;
            }
        }
        if found.count == 0 && found.burst_count == 0 {
            self.reactions.remove(at);
        }
    }

    /// Everyone's reactions with `emoji` were removed.
    pub fn remove_reactions_for_emoji(&mut self, emoji: &Emoji) {
        self.reactions.retain(|r| !r.emoji.same(emoji));
    }
}

/// Who reacted with what, as far as we know (`MessageReactionsStore`,
/// 956703): the people a request for them returned, and those whose reactions
/// came as events since. Kept for the session.
#[derive(Debug, Default)]
pub struct Reactors {
    entries: std::collections::HashMap<(Snowflake, String, Kind), Entry>,
}

#[derive(Debug, Default)]
struct Entry {
    /// Asked for already, whoever asked (`getReactions` asks once).
    fetched: bool,
    /// In the order they came.
    users: Vec<User>,
}

impl Reactors {
    fn entry(&mut self, message: Snowflake, emoji: &Emoji, kind: Kind) -> &mut Entry {
        self.entries
            .entry((
                message,
                format!("{}:{}", emoji.name.as_deref().unwrap_or_default(), emoji.id.map_or(0, |i| i.0)),
                kind,
            ))
            .or_default()
    }

    /// Who we know reacted.
    pub fn users(&mut self, message: Snowflake, emoji: &Emoji, kind: Kind) -> &[User] {
        &self.entry(message, emoji, kind).users
    }

    /// Whether to ask for them now: the first time only, as `getReactions`
    /// does when the tooltip is wanted.
    pub fn first_ask(&mut self, message: Snowflake, emoji: &Emoji, kind: Kind) -> bool {
        !std::mem::replace(&mut self.entry(message, emoji, kind).fetched, true)
    }

    /// People a request returned (`MESSAGE_REACTION_ADD_USERS`).
    pub fn arrived(&mut self, message: Snowflake, emoji: &Emoji, kind: Kind, users: Vec<User>) {
        let entry = self.entry(message, emoji, kind);
        for user in users {
            match entry.users.iter_mut().find(|u| u.id == user.id) {
                Some(known) => *known = user,
                None => entry.users.push(user),
            }
        }
    }

    /// An event: `user` reacted or took it back. The client keeps only the
    /// people it knows, so a reaction by one we have no name for isn't listed.
    pub fn reacted(
        &mut self,
        message: Snowflake,
        emoji: &Emoji,
        kind: Kind,
        user: Snowflake,
        known: Option<User>,
        added: bool,
    ) {
        let entry = self.entry(message, emoji, kind);
        match (added, known) {
            (true, Some(user)) => match entry.users.iter_mut().find(|u| u.id == user.id) {
                Some(listed) => *listed = user,
                None => entry.users.push(user),
            },
            (true, None) => {}
            (false, _) => entry.users.retain(|u| u.id != user),
        }
    }

    /// Everyone's reactions with `emoji` were removed, or all of `message`'s
    /// (`emoji` none). The client leaves them listed; a reaction made after
    /// would then show them with it.
    pub fn removed(&mut self, message: Snowflake, emoji: Option<&Emoji>) {
        let wanted = emoji.map(|e| format!("{}:{}", e.name.as_deref().unwrap_or_default(), e.id.map_or(0, |i| i.0)));
        self.entries.retain(|(m, key, _), _| *m != message || wanted.as_ref().is_some_and(|w| w != key));
    }

    /// A new session knows nothing of the last (`CONNECTION_OPEN`).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// What the tooltip over a reaction says (`e_` in 30492): up to three of the
/// people who reacted, and how many more `total` says there are.
pub fn who_reacted(emoji: &str, names: &[String], total: u32) -> Option<String> {
    let names = &names[..names.len().min(3)];
    let more = total.saturating_sub(names.len() as u32);
    let others = match more {
        0 => String::new(),
        1 => "1 other".to_owned(),
        n => format!("{n} others"),
    };
    let who = match (names, more) {
        ([], _) => return None,
        ([a], 0) => a.clone(),
        ([a], _) => format!("{a} and {others}"),
        ([a, b], 0) => format!("{a} and {b}"),
        ([a, b], _) => format!("{a}, {b}, and {others}"),
        ([a, b, c], 0) => format!("{a}, {b}, and {c}"),
        ([a, b, c], _) => format!("{a}, {b}, {c}, and {others}"),
        _ => unreachable!("at most three names"),
    };
    Some(format!("{emoji} reacted by {who}"))
}

/// The text of `+:name:`, which reacts to the newest message in place of
/// sending (`reaction` in 751258): the box, trimmed, is `+:`, a name and a
/// last `:`.
pub fn shortcut(text: &str) -> Option<&str> {
    let name = text.trim().strip_prefix("+:")?.strip_suffix(':')?;
    let line_end = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    (!name.is_empty() && !name.contains(line_end)).then_some(name)
}

/// Where in the client a reaction was asked for: the `location` its request
/// carries (`qN` in 649963).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// `+:name:` in the message box.
    Message,
    /// A reaction under a message, or the button that adds one.
    InlineButton,
    /// The emoji picker the message menu's Add Reaction opens.
    ReactionPicker,
}

impl Location {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Location::Message => "Message",
            Location::InlineButton => "Message Inline Button",
            Location::ReactionPicker => "Message Reaction Picker",
        }
    }
}

/// `TOO_MANY_REACTIONS`: a message holds at most 20 different reactions.
const TOO_MANY_REACTIONS: u64 = 30010;

/// What the client tells the person about a reaction, in a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The message has as many different reactions as it takes.
    TooMany,
    /// A reaction and a super reaction with one emoji can't both be ours.
    Clash,
}

impl Notice {
    pub fn title(self) -> &'static str {
        match self {
            Notice::TooMany => "We appreciate the enthusiasm, but...",
            Notice::Clash => "Oops!",
        }
    }

    pub fn body(self) -> &'static str {
        match self {
            Notice::TooMany => "Your reaction was not added because there are too many reactions on this message.",
            Notice::Clash => {
                "You can\u{2019}t react and super react with the same emoji. \
                 There\u{2019}s a lot of emoji to choose from, so maybe try another one."
            }
        }
    }

    /// The dialog's one button.
    pub fn confirm(self) -> &'static str {
        match self {
            Notice::TooMany => "Okay",
            Notice::Clash => "Got It",
        }
    }
}

/// A request that failed, with what the rule below needs of its response.
#[derive(Debug)]
pub struct Refusal {
    pub error: Error,
    /// The `Retry-After` header as sent.
    pub retry_after: Option<String>,
}

/// What to do about a refused request (`S` in 649963).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Send it again, once, after this long.
    Retry(Duration),
    /// Leave the change as it is, sending nothing more: a 429 whose wait
    /// can't be read.
    Keep,
    /// Put the change back.
    Undo(Option<Notice>),
}

/// How the client answers a refusal. `retried` is whether this was already
/// the second try.
///
/// A 429 waits the whole seconds `Retry-After` says (`parseInt`), once; any
/// other refusal but a 403 is tried again at once, once. A 403 is final:
/// `TOO_MANY_REACTIONS` also says so, and `REACTION_BLOCKED` shakes the
/// client's window, which Neucord has no way to.
pub fn judge(refusal: &Refusal, retried: bool) -> Verdict {
    match &refusal.error {
        Error::Http { status: 429, .. } if retried => Verdict::Undo(None),
        Error::Http { status: 429, .. } => match refusal.retry_after.as_deref().and_then(parse_int) {
            Some(seconds) => Verdict::Retry(Duration::from_secs(seconds)),
            None => Verdict::Keep,
        },
        Error::Http { status: 403, .. } => {
            Verdict::Undo((refusal.error.code() == Some(TOO_MANY_REACTIONS)).then_some(Notice::TooMany))
        }
        // Nothing was sent, and sending again would meet the same pause.
        Error::Paused(_) => Verdict::Undo(None),
        _ if retried => Verdict::Undo(None),
        _ => Verdict::Retry(Duration::ZERO),
    }
}

/// JavaScript's `parseInt(text, 10)`, for a number that can't be negative:
/// the digits it starts with, after any spaces.
fn parse_int(text: &str) -> Option<u64> {
    let digits: String = text.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ME: Snowflake = Snowflake(5);
    const THEM: Snowflake = Snowflake(6);

    fn message(reactions: serde_json::Value) -> Message {
        serde_json::from_value(json!({"id": "3", "channel_id": "9", "type": 0, "content": "hi",
            "author": {"id": "7", "username": "a"}, "timestamp": "", "reactions": reactions}))
        .unwrap()
    }

    fn unicode(glyph: &str) -> Emoji {
        Emoji { id: None, name: Some(glyph.into()), animated: false }
    }

    fn custom(id: u64) -> Emoji {
        Emoji { id: Some(Snowflake(id)), name: Some("blob".into()), animated: false }
    }

    /// `(count, me, burst_count, me_burst)` of each reaction.
    fn counts(m: &Message) -> Vec<(u32, bool, u32, bool)> {
        m.reactions.iter().map(|r| (r.count, r.me, r.burst_count, r.me_burst)).collect()
    }

    #[test]
    fn reads_what_discord_sends() {
        let m = message(json!([
            {"emoji": {"id": null, "name": "🔥"}, "count": 3, "count_details": {"burst": 1, "normal": 2},
             "me": true, "me_burst": false, "burst_colors": ["#ff0000"]},
            {"emoji": {"id": "12", "name": "blob", "animated": true}, "count": 1, "me": false},
            {"nonsense": true},
        ]));
        assert_eq!(counts(&m), vec![(2, true, 1, false), (1, false, 0, false)], "normal comes from count_details");
        assert_eq!(m.reactions[0].burst_colors, ["#ff0000"]);
        assert!(m.reactions[1].emoji.animated && m.reactions[1].burst_colors.is_empty());
        assert!(message(json!(null)).reactions.is_empty());
    }

    #[test]
    fn counts_never_go_below_nothing() {
        let m = message(json!([{"emoji": {"name": "🔥"}, "count": -2, "count_details": {"normal": -1, "burst": -4}}]));
        assert_eq!(counts(&m), vec![(0, false, 0, false)]);
    }

    #[test]
    fn pills_put_super_reactions_first_and_skip_empty_ones() {
        let m = message(json!([
            {"emoji": {"name": "🔥"}, "count_details": {"normal": 2, "burst": 1}, "me_burst": true},
            {"emoji": {"name": "🙂"}, "count_details": {"normal": 0, "burst": 0}},
            {"emoji": {"id": "12", "name": "blob"}, "count_details": {"normal": 4, "burst": 0}, "me": true},
        ]));
        let pills: Vec<_> = m.reaction_pills().iter().map(|p| (p.emoji.path(), p.kind, p.count, p.me)).collect();
        assert_eq!(
            pills,
            [
                ("🔥".to_owned(), Kind::Burst, 1, true),
                ("🔥".to_owned(), Kind::Normal, 2, false),
                ("blob:12".to_owned(), Kind::Normal, 4, true),
            ]
        );
    }

    #[test]
    fn emoji_are_the_same_by_id_or_by_name() {
        assert!(custom(1).same(&custom(1)));
        assert!(!custom(1).same(&custom(2)));
        let renamed = Emoji { name: Some("other".into()), ..custom(1) };
        assert!(renamed.same(&custom(1)), "a custom emoji is its id");
        assert!(unicode("🔥").same(&unicode("🔥")) && !unicode("🔥").same(&unicode("🙂")));
        let nameless = Emoji { id: None, name: Some("blob".into()), animated: false };
        assert!(!custom(1).same(&nameless), "a Unicode name isn't a custom emoji");
        assert!(!nameless.same(&custom(1)));
    }

    #[test]
    fn requests_name_the_emoji() {
        assert_eq!(unicode("🔥").path(), "🔥");
        assert_eq!(custom(12).path(), "blob:12");
    }

    #[test]
    fn a_reaction_is_counted_once_for_us() {
        let mut m = message(json!([]));
        let fire = unicode("🔥");
        // Our own, as the optimistic change, then as the gateway's echo.
        m.add_reaction(&fire, true, Kind::Normal, &[], false);
        m.add_reaction(&fire, true, Kind::Normal, &[], false);
        assert_eq!(counts(&m), vec![(1, true, 0, false)]);
        m.add_reaction(&fire, false, Kind::Normal, &[], false);
        assert_eq!(counts(&m), vec![(2, true, 0, false)]);
    }

    #[test]
    fn a_dm_counts_the_other_side_once() {
        let mut m = message(json!([]));
        let fire = unicode("🔥");
        m.add_reaction(&fire, false, Kind::Normal, &[], true);
        m.add_reaction(&fire, false, Kind::Normal, &[], true);
        assert_eq!(counts(&m), vec![(1, false, 0, false)], "the same event twice");
        m.add_reaction(&fire, true, Kind::Normal, &[], true);
        assert_eq!(counts(&m), vec![(2, true, 0, false)], "ours is a second person");
        m.add_reaction(&fire, false, Kind::Normal, &[], false);
        assert_eq!(counts(&m), vec![(3, true, 0, false)], "a server has no such rule");
    }

    #[test]
    fn super_reactions_count_apart() {
        let mut m = message(json!([]));
        let fire = unicode("🔥");
        m.add_reaction(&fire, false, Kind::Burst, &["#111111".into()], false);
        m.add_reaction(&fire, true, Kind::Burst, &["#222222".into()], false);
        m.add_reaction(&fire, true, Kind::Burst, &[], false);
        assert_eq!(counts(&m), vec![(0, false, 2, true)]);
        assert_eq!(m.reactions[0].burst_colors, ["#111111"], "the first colours stay");
        m.add_reaction(&fire, true, Kind::Normal, &[], false);
        assert_eq!(counts(&m), vec![(1, true, 2, true)]);
    }

    #[test]
    fn a_batch_counts_everyone_as_a_normal_reaction() {
        let mut m = message(json!([]));
        let (fire, blob) = (unicode("🔥"), custom(12));
        m.add_reactions(&[(fire.clone(), vec![ME, THEM]), (blob, vec![THEM])], Some(ME));
        assert_eq!(counts(&m), vec![(2, true, 0, false), (1, false, 0, false)]);
        m.add_reactions(&[(fire, vec![ME])], Some(ME));
        assert_eq!(counts(&m)[0], (2, true, 0, false), "ours was counted already");
    }

    #[test]
    fn taking_a_reaction_back() {
        let mut m = message(json!([
            {"emoji": {"name": "🔥"}, "count_details": {"normal": 2, "burst": 0}, "me": true},
            {"emoji": {"name": "🙂"}, "count_details": {"normal": 1, "burst": 0}, "me": true},
        ]));
        let (fire, smile) = (unicode("🔥"), unicode("🙂"));
        m.remove_reaction(&fire, true, Kind::Normal);
        assert_eq!(counts(&m)[0], (1, false, 0, false));
        m.remove_reaction(&fire, true, Kind::Normal);
        assert_eq!(counts(&m)[0], (1, false, 0, false), "ours is gone already: the echo of our own change");
        m.remove_reaction(&fire, false, Kind::Normal);
        assert_eq!(counts(&m).len(), 1, "no one left, no pill");
        m.remove_reaction(&smile, true, Kind::Normal);
        assert!(m.reactions.is_empty());
        m.remove_reaction(&smile, true, Kind::Normal);
    }

    #[test]
    fn taking_back_leaves_the_other_kind() {
        let mut m = message(json!([{"emoji": {"name": "🔥"}, "count_details": {"normal": 1, "burst": 1},
                                    "me": true, "me_burst": true}]));
        m.remove_reaction(&unicode("🔥"), true, Kind::Burst);
        assert_eq!(counts(&m), vec![(1, true, 0, false)]);
    }

    #[test]
    fn removing_an_emoji_removes_all_of_it() {
        let mut m = message(
            json!([{"emoji": {"name": "🔥"}, "count": 3}, {"emoji": {"id": "12", "name": "blob"}, "count": 1}]),
        );
        m.remove_reactions_for_emoji(&custom(12));
        assert_eq!(m.reactions.len(), 1);
        m.remove_reactions_for_emoji(&custom(12));
        assert_eq!(m.reactions.len(), 1);
    }

    #[test]
    fn no_reacting_and_super_reacting_with_one_emoji() {
        let m =
            message(json!([{"emoji": {"name": "🔥"}, "count_details": {"normal": 0, "burst": 1}, "me_burst": true},
                               {"emoji": {"name": "🙂"}, "count_details": {"normal": 1, "burst": 0}, "me": true}]));
        assert!(m.reacted_the_other_way(&unicode("🔥"), Kind::Normal));
        assert!(!m.reacted_the_other_way(&unicode("🔥"), Kind::Burst));
        assert!(m.reacted_the_other_way(&unicode("🙂"), Kind::Burst));
        assert!(!m.reacted_the_other_way(&unicode("🙂"), Kind::Normal));
        assert!(!m.reacted_the_other_way(&unicode("🌍"), Kind::Normal));
    }

    #[test]
    fn events_name_their_kind() {
        assert_eq!(Kind::of_event(None), Some(Kind::Normal));
        assert_eq!(Kind::of_event(Some(0)), Some(Kind::Normal));
        assert_eq!(Kind::of_event(Some(1)), Some(Kind::Burst));
        assert_eq!(Kind::of_event(Some(2)), None, "a poll vote has its own events");
    }

    #[test]
    fn the_shortcut_is_a_name_between_colons() {
        assert_eq!(shortcut("+:fire:"), Some("fire"));
        assert_eq!(shortcut("  +:fire:  "), Some("fire"));
        assert_eq!(shortcut("+:a b:"), Some("a b"), "whatever is between");
        assert_eq!(shortcut("+:a:b:"), Some("a:b"));
        for text in ["+:fire", "+fire:", "+::", "+:", "hi +:fire:", "+:fire: hi", "+:a\nb:", ":fire:"] {
            assert_eq!(shortcut(text), None, "{text:?}");
        }
    }

    fn user(id: u64, name: &str) -> User {
        User { id: Snowflake(id), username: name.into(), global_name: None, avatar: None, bot: false, system: false }
    }

    #[test]
    fn the_tooltip_names_up_to_three_and_counts_the_rest() {
        let say = |names: &[&str], total| {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            who_reacted(":fire:", &names, total)
        };
        assert_eq!(say(&[], 3), None, "no one known, no tooltip");
        assert_eq!(say(&["A"], 1).as_deref(), Some(":fire: reacted by A"));
        assert_eq!(say(&["A"], 2).as_deref(), Some(":fire: reacted by A and 1 other"));
        assert_eq!(say(&["A", "B"], 2).as_deref(), Some(":fire: reacted by A and B"));
        assert_eq!(say(&["A", "B"], 5).as_deref(), Some(":fire: reacted by A, B, and 3 others"));
        assert_eq!(say(&["A", "B", "C"], 3).as_deref(), Some(":fire: reacted by A, B, and C"));
        assert_eq!(say(&["A", "B", "C"], 4).as_deref(), Some(":fire: reacted by A, B, C, and 1 other"));
        assert_eq!(say(&["A", "B", "C", "D"], 9).as_deref(), Some(":fire: reacted by A, B, C, and 6 others"));
        assert_eq!(say(&["A", "B"], 1).as_deref(), Some(":fire: reacted by A and B"), "never fewer than we name");
    }

    #[test]
    fn who_reacted_is_asked_for_once_and_kept_up_by_events() {
        let mut reactors = Reactors::default();
        let (message, fire) = (Snowflake(3), unicode("🔥"));
        assert!(reactors.first_ask(message, &fire, Kind::Normal));
        assert!(!reactors.first_ask(message, &fire, Kind::Normal));
        assert!(reactors.first_ask(message, &fire, Kind::Burst), "super reactions are listed apart");
        assert!(reactors.first_ask(Snowflake(4), &fire, Kind::Normal), "and each message");
        reactors.arrived(message, &fire, Kind::Normal, vec![user(1, "a"), user(2, "b")]);
        reactors.reacted(message, &fire, Kind::Normal, Snowflake(3), Some(user(3, "c")), true);
        reactors.reacted(message, &fire, Kind::Normal, Snowflake(9), None, true);
        let ids = |r: &mut Reactors| r.users(message, &fire, Kind::Normal).iter().map(|u| u.id.0).collect::<Vec<_>>();
        assert_eq!(ids(&mut reactors), [1, 2, 3], "someone we have no name for isn't listed");
        reactors.reacted(message, &fire, Kind::Normal, Snowflake(2), None, false);
        reactors.arrived(message, &fire, Kind::Normal, vec![user(1, "renamed"), user(5, "e")]);
        assert_eq!(ids(&mut reactors), [1, 3, 5]);
        assert_eq!(reactors.users(message, &fire, Kind::Normal)[0].username, "renamed");
    }

    #[test]
    fn removing_reactions_forgets_who_made_them() {
        let mut reactors = Reactors::default();
        let (message, fire, blob) = (Snowflake(3), unicode("🔥"), custom(12));
        for emoji in [&fire, &blob] {
            reactors.arrived(message, emoji, Kind::Normal, vec![user(1, "a")]);
            reactors.arrived(Snowflake(4), emoji, Kind::Normal, vec![user(1, "a")]);
        }
        reactors.removed(message, Some(&blob));
        assert_eq!(reactors.users(message, &blob, Kind::Normal).len(), 0);
        assert_eq!(reactors.users(message, &fire, Kind::Normal).len(), 1);
        reactors.removed(message, None);
        assert_eq!(reactors.users(message, &fire, Kind::Normal).len(), 0);
        assert_eq!(reactors.users(Snowflake(4), &fire, Kind::Normal).len(), 1, "another message's stay");
    }

    fn refused(status: u32, body: &str, retry_after: Option<&str>) -> Refusal {
        Refusal { error: Error::Http { status, body: body.into() }, retry_after: retry_after.map(Into::into) }
    }

    #[test]
    fn a_rate_limit_waits_the_whole_seconds_it_says() {
        let wait = |header| judge(&refused(429, "{}", header), false);
        assert_eq!(wait(Some("2")), Verdict::Retry(Duration::from_secs(2)));
        assert_eq!(wait(Some("2.9")), Verdict::Retry(Duration::from_secs(2)), "parseInt stops at the dot");
        assert_eq!(wait(Some("0")), Verdict::Retry(Duration::ZERO));
        assert_eq!(wait(None), Verdict::Keep, "no wait to read: nothing is sent, nothing undone");
        assert_eq!(wait(Some("soon")), Verdict::Keep);
        assert_eq!(judge(&refused(429, "{}", Some("2")), true), Verdict::Undo(None), "only once");
    }

    #[test]
    fn a_403_is_final() {
        let many = r#"{"code": 30010, "message": "Maximum number of reactions reached (20)"}"#;
        assert_eq!(judge(&refused(403, many, None), false), Verdict::Undo(Some(Notice::TooMany)));
        assert_eq!(judge(&refused(403, many, None), true), Verdict::Undo(Some(Notice::TooMany)));
        let blocked = r#"{"code": 90001, "message": "Reaction blocked"}"#;
        assert_eq!(judge(&refused(403, blocked, None), false), Verdict::Undo(None));
        assert_eq!(judge(&refused(403, "not json", None), false), Verdict::Undo(None));
    }

    #[test]
    fn anything_else_is_tried_again_once() {
        for status in [400, 404, 500, 401] {
            assert_eq!(judge(&refused(status, "{}", None), false), Verdict::Retry(Duration::ZERO), "{status}");
            assert_eq!(judge(&refused(status, "{}", None), true), Verdict::Undo(None), "{status}");
        }
        assert_eq!(
            judge(&Refusal { error: Error::Unauthorized, retry_after: None }, false),
            Verdict::Retry(Duration::ZERO)
        );
    }

    #[test]
    fn a_pause_is_not_asked_again() {
        let paused = Refusal { error: Error::Paused(Duration::from_secs(30)), retry_after: None };
        assert_eq!(judge(&paused, false), Verdict::Undo(None));
    }

    #[test]
    fn locations_are_the_clients() {
        assert_eq!(Location::Message.name(), "Message");
        assert_eq!(Location::InlineButton.name(), "Message Inline Button");
        assert_eq!(Location::ReactionPicker.name(), "Message Reaction Picker");
    }
}
