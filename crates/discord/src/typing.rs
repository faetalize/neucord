//! Who is typing where, and when we say that we are.
//!
//! Both halves are the official client's `TypingStore` (741961): the store
//! keeps `TYPING_START` entries for ten seconds, and our own typing goes out as
//! `POST /channels/{id}/typing` on a schedule of its own, which the client
//! derives from the same ten seconds.

use std::collections::HashMap;

use crate::Snowflake;

/// How long a `TYPING_START` shows, from the last one for that person.
pub const EXPIRY_MS: i64 = 10_000;

/// At most one send per this long: 0.8 of [`EXPIRY_MS`], so an entry never
/// lapses while we keep typing.
const SEND_EVERY_MS: i64 = EXPIRY_MS / 10 * 8;

/// How long after a keystroke the first send waits, so a few characters that
/// are deleted again say nothing.
const FIRST_SEND_DELAY_MS: i64 = 1_500;

/// With more people than this typing (counting us) the client stays quiet:
/// nobody would notice one more.
const CROWD: usize = 5;

/// What the message box's text says about typing, whenever the user changed
/// it (`handleTextareaChange` in 699344).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Typing: a send is due.
    Typed,
    /// The box was emptied.
    Cleared,
    /// Neither: a slash command or a reaction shortcut isn't a message
    /// being written.
    Ignored,
}

impl Input {
    pub fn of(text: &str) -> Input {
        if text.is_empty() {
            Input::Cleared
        } else if text.starts_with('/') || reaction_shortcut(text) {
            Input::Ignored
        } else {
            Input::Typed
        }
    }
}

/// `+`, `+:`, `+:name` and `+:name:`, what the client matches with
/// `/^\+(?!\w+):?(?!:)(\w+)?:?$/` to react to the last message.
fn reaction_shortcut(text: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let Some(rest) = text.strip_prefix('+') else { return false };
    if rest.starts_with(word) {
        return false;
    }
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    !rest.starts_with(':') && matches!(rest.trim_start_matches(word), "" | ":")
}

/// A send to make once `delay_ms` has passed, if [`Typing::send_due`] still
/// agrees then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scheduled {
    pub token: u64,
    pub delay_ms: i64,
}

/// Our own typing: the channel we last typed in, whether its send is still
/// waiting, and when we last took a keystroke for one.
#[derive(Clone, Copy, Debug)]
struct Local {
    channel: Snowflake,
    waiting: Option<u64>,
    /// When the keystroke that scheduled the last send came, not when the
    /// send went out.
    last: i64,
}

#[derive(Debug, Default)]
pub struct Typing {
    /// Each channel's typers in the order they started, with when they lapse.
    channels: HashMap<Snowflake, Vec<(Snowflake, i64)>>,
    local: Option<Local>,
    tokens: u64,
    /// The user turned sending off (Neucord's own option: the client always
    /// sends). Showing who else is typing is unaffected.
    silent: bool,
}

impl Typing {
    /// `user` is typing in `channel`; a repeat restarts their ten seconds
    /// and keeps their place, unless they had lapsed, which puts them last.
    pub fn start(&mut self, channel: Snowflake, user: Snowflake, now_ms: i64) {
        let typers = self.channels.entry(channel).or_default();
        typers.retain(|(_, lapses)| *lapses > now_ms);
        let lapses = now_ms + EXPIRY_MS;
        match typers.iter_mut().find(|(id, _)| *id == user) {
            Some(typer) => typer.1 = lapses,
            None => typers.push((user, lapses)),
        }
    }

    /// `user` stopped typing in `channel`, or sent their message.
    pub fn stop(&mut self, channel: Snowflake, user: Snowflake) {
        if let Some(typers) = self.channels.get_mut(&channel) {
            typers.retain(|(id, _)| *id != user);
            if typers.is_empty() {
                self.channels.remove(&channel);
            }
        }
    }

    /// A new session starts from nothing. Our own typing carries on, as the
    /// client's does.
    pub fn clear(&mut self) {
        self.channels.clear();
    }

    /// Who is typing in `channel` now, in the order they started, us included.
    pub fn typers(&self, channel: Snowflake, now_ms: i64) -> Vec<Snowflake> {
        let typers = self.channels.get(&channel).map(Vec::as_slice).unwrap_or_default();
        typers.iter().filter(|(_, lapses)| *lapses > now_ms).map(|(id, _)| *id).collect()
    }

    /// When the first of `channel`'s entries lapses, to redraw then.
    pub fn next_lapse(&self, channel: Snowflake, now_ms: i64) -> Option<i64> {
        let typers = self.channels.get(&channel)?;
        typers.iter().map(|(_, lapses)| *lapses).filter(|&lapses| lapses > now_ms).min()
    }

    /// Drop what has lapsed.
    pub fn prune(&mut self, now_ms: i64) {
        self.channels.retain(|_, typers| {
            typers.retain(|(_, lapses)| *lapses > now_ms);
            !typers.is_empty()
        });
    }

    /// A message by `author` arrived in `channel`: they are done typing. Ours,
    /// the moment we send it, also ends the schedule (`MESSAGE_CREATE`, with
    /// `optimistic`), so that what we type next starts afresh.
    pub fn message(&mut self, channel: Snowflake, author: Snowflake, ours_going_out: bool) {
        if ours_going_out && self.local.is_some_and(|l| l.channel == channel) {
            self.local = None;
        }
        self.stop(channel, author);
    }

    /// Whether our typing goes out at all. Turning it off withdraws a send
    /// that is waiting, so the notice the user just opted out of doesn't
    /// follow them.
    pub fn set_sending(&mut self, sending: bool) {
        self.silent = !sending;
        if self.silent {
            self.local = None;
        }
    }

    /// The user typed in `channel` (`TYPING_START_LOCAL`). Returns when a send
    /// is to be made, `None` while one is waiting or went out lately.
    pub fn typed(&mut self, channel: Snowflake, me: Snowflake, now_ms: i64) -> Option<Scheduled> {
        if self.silent {
            return None;
        }
        if self.local.is_some_and(|l| l.channel != channel) {
            self.local = None;
        }
        if self.local.is_some_and(|l| l.waiting.is_some() || l.last + SEND_EVERY_MS > now_ms) {
            return None;
        }
        // Back after a long pause there is nothing to wait for, but the first
        // keystrokes of a session, and of an ongoing one, still are.
        let delay_ms =
            if self.local.is_none_or(|l| l.last > now_ms - 2 * SEND_EVERY_MS) { FIRST_SEND_DELAY_MS } else { 0 };
        self.tokens += 1;
        let token = self.tokens;
        self.local = Some(Local { channel, waiting: Some(token), last: now_ms });
        self.start(channel, me, now_ms);
        Some(Scheduled { token, delay_ms })
    }

    /// The box was emptied before the send went out, so none will
    /// (`TYPING_STOP_LOCAL`). Once it has, this changes nothing: it lapses on
    /// its own, and the schedule keeps its place.
    pub fn cleared(&mut self, channel: Snowflake, me: Snowflake) {
        if self.local.is_some_and(|l| l.channel == channel && l.waiting.is_some()) {
            self.local = None;
            self.stop(channel, me);
        }
    }

    /// The wait of `token` ended: the channel to send to, unless it was
    /// withdrawn meanwhile or the channel is crowded (our own entry counts).
    pub fn send_due(&mut self, token: u64, now_ms: i64) -> Option<Snowflake> {
        let local = self.local.as_mut().filter(|l| l.waiting == Some(token))?;
        local.waiting = None;
        let channel = local.channel;
        (self.typers(channel, now_ms).len() <= CROWD).then_some(channel)
    }
}

/// The names of `typers` the line lists: not ours, nobody we blocked, and
/// only those we have a name for, as the client's `getUser` does. Ignored
/// users, which the client also leaves out, aren't modelled.
pub fn shown(
    typers: &[Snowflake],
    me: Snowflake,
    blocked: impl Fn(Snowflake) -> bool,
    name: impl Fn(Snowflake) -> Option<String>,
) -> Vec<String> {
    typers.iter().filter(|&&id| id != me && !blocked(id)).filter_map(|&id| name(id)).collect()
}

/// A piece of the line that names who is typing, which draws names apart
/// from the words around them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Name(String),
    Words(&'static str),
}

/// The line for `names`: nothing for nobody, the names of up to three, and
/// otherwise "Several people". `narrow` says the names don't fit where the
/// line goes, which makes "Multiple people" of up to three.
pub fn sentence(names: &[String], narrow: bool) -> Vec<Piece> {
    let name = |n: &String| Piece::Name(n.clone());
    let many = |who: &str| vec![Piece::Name(who.to_owned()), Piece::Words(" are typing...")];
    match names {
        [] => Vec::new(),
        _ if narrow && names.len() <= 3 => many("Multiple people"),
        [a] => vec![name(a), Piece::Words(" is typing...")],
        [a, b] => vec![name(a), Piece::Words(" and "), name(b), Piece::Words(" are typing...")],
        [a, b, c] => {
            vec![name(a), Piece::Words(", "), name(b), Piece::Words(", and "), name(c), Piece::Words(" are typing...")]
        }
        _ => many("Several people"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Snowflake = Snowflake(1);
    const ROOM: Snowflake = Snowflake(100);
    const OTHER_ROOM: Snowflake = Snowflake(200);

    fn typers(typing: &Typing, now: i64) -> Vec<u64> {
        typing.typers(ROOM, now).into_iter().map(|id| id.0).collect()
    }

    #[test]
    fn entries_last_ten_seconds_from_the_latest_start() {
        let mut typing = Typing::default();
        typing.start(ROOM, Snowflake(7), 0);
        typing.start(ROOM, Snowflake(8), 3_000);
        assert_eq!(typers(&typing, 9_999), [7, 8]);
        assert_eq!(typers(&typing, 10_000), [8], "the first lapsed");
        typing.start(ROOM, Snowflake(7), 12_000);
        assert_eq!(typers(&typing, 12_999), [8, 7], "7 lapsed, so it comes back last");
        assert_eq!(typers(&typing, 13_000), [7]);
    }

    #[test]
    fn a_repeat_restarts_the_entry_and_keeps_its_place() {
        let mut typing = Typing::default();
        typing.start(ROOM, Snowflake(7), 0);
        typing.start(ROOM, Snowflake(8), 1_000);
        typing.start(ROOM, Snowflake(7), 5_000);
        assert_eq!(typers(&typing, 6_000), [7, 8], "7 started first");
        assert_eq!(typers(&typing, 11_500), [7], "8 lapsed at 11 s, 7 at 15 s");
        assert_eq!(typing.next_lapse(ROOM, 11_500), Some(15_000));
        assert_eq!(typing.next_lapse(OTHER_ROOM, 0), None);
    }

    #[test]
    fn a_message_ends_its_authors_entry_only() {
        let mut typing = Typing::default();
        typing.start(ROOM, Snowflake(7), 0);
        typing.start(ROOM, Snowflake(8), 0);
        typing.start(OTHER_ROOM, Snowflake(7), 0);
        typing.message(ROOM, Snowflake(7), false);
        assert_eq!(typers(&typing, 1), [8]);
        assert_eq!(typing.typers(OTHER_ROOM, 1), [Snowflake(7)], "in that channel only");
    }

    #[test]
    fn a_new_session_forgets_who_was_typing() {
        let mut typing = Typing::default();
        typing.start(ROOM, Snowflake(7), 0);
        typing.clear();
        assert!(typers(&typing, 1).is_empty());
    }

    #[test]
    fn pruning_drops_what_lapsed() {
        let mut typing = Typing::default();
        typing.start(ROOM, Snowflake(7), 0);
        typing.start(ROOM, Snowflake(8), 5_000);
        typing.prune(10_000);
        assert_eq!(typing.next_lapse(ROOM, 0), Some(15_000));
        typing.prune(15_000);
        assert_eq!(typing.next_lapse(ROOM, 0), None);
    }

    #[test]
    fn typing_with_sending_off_schedules_nothing_and_shows_no_entry_of_ours() {
        let mut typing = Typing::default();
        typing.set_sending(false);
        assert_eq!(typing.typed(ROOM, ME, 0), None);
        assert!(typing.typers(ROOM, 0).is_empty());
        typing.start(ROOM, Snowflake(7), 0);
        assert_eq!(typers(&typing, 1), [7], "other people's typing still shows");
    }

    #[test]
    fn turning_sending_off_withdraws_the_send_that_is_waiting() {
        let mut typing = Typing::default();
        let waiting = typing.typed(ROOM, ME, 0).unwrap();
        typing.set_sending(false);
        assert_eq!(typing.send_due(waiting.token, 1_500), None);
    }

    #[test]
    fn turning_sending_back_on_sends_with_the_next_keystroke() {
        let mut typing = Typing::default();
        typing.set_sending(false);
        typing.typed(ROOM, ME, 0);
        typing.set_sending(true);
        let first = typing.typed(ROOM, ME, 100).expect("scheduled");
        assert_eq!(typing.send_due(first.token, 1_600), Some(ROOM));
    }

    #[test]
    fn the_first_keystroke_sends_after_a_moment() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).expect("scheduled");
        assert_eq!(first.delay_ms, 1_500);
        assert!(typing.typers(ROOM, 0).contains(&ME), "our own entry shows at once");
        assert_eq!(typing.typed(ROOM, ME, 200), None, "one is already waiting");
        assert_eq!(typing.send_due(first.token, 1_500), Some(ROOM));
        assert_eq!(typing.send_due(first.token, 1_501), None, "a send is made once");
    }

    #[test]
    fn typing_on_sends_again_after_eight_seconds_of_the_last_keystroke_that_counted() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.send_due(first.token, 1_500);
        assert_eq!(typing.typed(ROOM, ME, 7_999), None, "less than 8 s after the keystroke that scheduled it");
        let second = typing.typed(ROOM, ME, 8_000).expect("8 s on");
        assert_eq!(second.delay_ms, 1_500, "still the same stretch of typing");
        typing.send_due(second.token, 9_500);
        assert_eq!(typing.typed(ROOM, ME, 15_999), None, "counted from 8 s, not from the send");
        assert!(typing.typed(ROOM, ME, 16_000).is_some());
    }

    #[test]
    fn coming_back_after_a_long_pause_sends_at_once() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.send_due(first.token, 1_500);
        assert_eq!(typing.typed(ROOM, ME, 15_999).map(|s| s.delay_ms), Some(1_500), "under 16 s is the same stretch");
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.send_due(first.token, 1_500);
        assert_eq!(typing.typed(ROOM, ME, 16_000).map(|s| s.delay_ms), Some(0), "twice 8 s on");
    }

    #[test]
    fn emptying_the_box_withdraws_a_send_that_hasnt_gone_out() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.cleared(ROOM, ME);
        assert_eq!(typing.send_due(first.token, 1_500), None);
        assert!(typing.typers(ROOM, 1).is_empty(), "and our entry goes");
        assert!(typing.typed(ROOM, ME, 100).is_some(), "typing again starts afresh");
    }

    #[test]
    fn emptying_the_box_after_the_send_changes_nothing() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.send_due(first.token, 1_500);
        typing.cleared(ROOM, ME);
        assert!(typing.typers(ROOM, 2_000).contains(&ME), "it lapses on its own");
        assert_eq!(typing.typed(ROOM, ME, 2_000), None, "and the 8 s still run");
    }

    #[test]
    fn sending_a_message_ends_the_schedule() {
        let mut typing = Typing::default();
        let first = typing.typed(ROOM, ME, 0).unwrap();
        typing.send_due(first.token, 1_500);
        typing.message(ROOM, ME, true);
        assert!(typing.typers(ROOM, 2_000).is_empty());
        assert_eq!(typing.typed(ROOM, ME, 2_000).map(|s| s.delay_ms), Some(1_500), "the next message starts afresh");

        let waiting = typing.typed(OTHER_ROOM, ME, 3_000).unwrap();
        typing.message(ROOM, ME, true);
        assert_eq!(typing.send_due(waiting.token, 4_500), Some(OTHER_ROOM), "another channel's isn't touched");
    }

    #[test]
    fn a_message_shown_again_cancels_what_was_typed_since() {
        let mut typing = Typing::default();
        typing.message(ROOM, ME, true);
        let meanwhile = typing.typed(ROOM, ME, 500).unwrap();
        typing.message(ROOM, ME, true);
        assert_eq!(typing.send_due(meanwhile.token, 2_000), None, "the send waiting is withdrawn");
        assert!(typing.typers(ROOM, 2_000).is_empty());
    }

    #[test]
    fn typing_in_another_channel_withdraws_the_send_waiting_in_the_first() {
        let mut typing = Typing::default();
        let here = typing.typed(ROOM, ME, 0).unwrap();
        let there = typing.typed(OTHER_ROOM, ME, 500).unwrap();
        assert_eq!(typing.send_due(here.token, 1_500), None);
        assert_eq!(typing.send_due(there.token, 2_000), Some(OTHER_ROOM));
    }

    #[test]
    fn a_crowd_of_more_than_five_is_left_alone() {
        let mut typing = Typing::default();
        for other in 10..14 {
            typing.start(ROOM, Snowflake(other), 0);
        }
        let five = typing.typed(ROOM, ME, 0).unwrap();
        assert_eq!(typing.send_due(five.token, 1_500), Some(ROOM), "four others and us make five");

        let mut typing = Typing::default();
        for other in 10..15 {
            typing.start(ROOM, Snowflake(other), 0);
        }
        let six = typing.typed(ROOM, ME, 0).unwrap();
        assert_eq!(typing.send_due(six.token, 1_500), None, "five others and us make six");
        assert_eq!(typing.typed(ROOM, ME, 3_000), None, "and the 8 s run all the same");
    }

    #[test]
    fn what_the_text_says() {
        for typed in ["h", "hello", "+1", "+ :a:", "+::", "++", "hi /there", "see +:a:"] {
            assert_eq!(Input::of(typed), Input::Typed, "{typed:?}");
        }
        for ignored in ["/", "/shrug", "+", "+:", "+:smile", "+:smile:", "+:a_1:"] {
            assert_eq!(Input::of(ignored), Input::Ignored, "{ignored:?}");
        }
        assert_eq!(Input::of(""), Input::Cleared);
    }

    #[test]
    fn the_line_leaves_out_us_the_blocked_and_the_unknown() {
        let typers = [ME, Snowflake(7), Snowflake(8), Snowflake(9)];
        let name = |id: Snowflake| (id != Snowflake(9)).then(|| format!("user{}", id.0));
        assert_eq!(shown(&typers, ME, |id| id == Snowflake(8), name), ["user7"]);
        assert!(shown(&[ME], ME, |_| false, name).is_empty());
    }

    fn line(names: &[&str], narrow: bool) -> String {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        sentence(&names, narrow)
            .into_iter()
            .map(|piece| match piece {
                Piece::Name(n) => format!("[{n}]"),
                Piece::Words(w) => w.to_owned(),
            })
            .collect()
    }

    #[test]
    fn names_are_listed_up_to_three() {
        assert_eq!(line(&[], false), "");
        assert_eq!(line(&["ann"], false), "[ann] is typing...");
        assert_eq!(line(&["ann", "bo"], false), "[ann] and [bo] are typing...");
        assert_eq!(line(&["ann", "bo", "cy"], false), "[ann], [bo], and [cy] are typing...");
        assert_eq!(line(&["ann", "bo", "cy", "di"], false), "[Several people] are typing...");
    }

    #[test]
    fn names_that_dont_fit_make_multiple_people() {
        assert_eq!(line(&["ann"], true), "[Multiple people] are typing...");
        assert_eq!(line(&["ann", "bo", "cy"], true), "[Multiple people] are typing...");
        assert_eq!(line(&["ann", "bo", "cy", "di"], true), "[Several people] are typing...");
        assert_eq!(line(&[], true), "");
    }
}
