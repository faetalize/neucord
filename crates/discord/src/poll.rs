//! Polls, read the way the official client reads them: how many voted for
//! what, whether counts show yet, and how long is left, and what a voter
//! sees and may do before and after voting.

use serde::Deserialize;

use crate::model::{Emoji, Message, Snowflake};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Poll {
    pub question: PollMedia,
    #[serde(default)]
    pub answers: Vec<Answer>,
    /// ISO 8601; `None` for a poll that never closes.
    #[serde(default)]
    pub expiry: Option<String>,
    #[serde(default)]
    pub allow_multiselect: bool,
    /// Missing until someone votes, in some payloads.
    #[serde(default)]
    pub results: Option<Results>,
    /// Ours, not Discord's: what we have picked and how far a vote is. It
    /// lives here so that a poll that changed is a message that changed, and
    /// its row is drawn again.
    #[serde(skip)]
    pub ballot: Ballot,
}

/// What a voter has done to a poll that Discord doesn't know yet: the client
/// keeps it per poll (862780) and so do we, for as long as we hold the message.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ballot {
    /// In the order picked, which is the order a vote sends them in.
    pub picked: Vec<u32>,
    pub submitting: bool,
    /// Voting again over a vote already cast. Only a vote submitted while
    /// "Show results" is pressed gets here.
    pub editing: bool,
    pub show_results: bool,
}

impl Ballot {
    /// An answer was tapped (`handlePollAnswerTapped`): picked, or put back,
    /// and a poll of one answer drops what was picked before.
    pub fn tap(&mut self, answer: u32, multiselect: bool) {
        if let Some(at) = self.picked.iter().position(|&a| a == answer) {
            self.picked.remove(at);
            return;
        }
        if !multiselect {
            self.picked.clear();
        }
        self.picked.push(answer);
    }

    /// "Show results" and "Go back to vote": what was picked is let go, and
    /// a vote being sent goes on being one that is edited (`showVotes`).
    pub fn toggle_results(&mut self) {
        self.show_results = !self.show_results;
        self.picked.clear();
        self.editing = self.submitting;
    }

    /// "Cancel", and "Remove Vote", which then submits nothing: back to a
    /// blank ballot that keeps whether results show.
    pub fn clear(&mut self) {
        *self = Ballot { show_results: self.show_results, ..Ballot::default() };
    }

    /// A vote is on its way: the answers it carries.
    pub fn start_submit(&mut self) -> Vec<u32> {
        self.submitting = true;
        self.editing = false;
        self.picked.clone()
    }

    /// The vote was not accepted; what was picked stays.
    pub fn failed(&mut self) {
        self.submitting = false;
        self.editing = false;
    }
}

/// What a poll offers one voter, from `951727`'s view model: which answers
/// can be tapped, the buttons under them and what they say.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Votes and shares show.
    pub counts: bool,
    /// Answers are radio buttons or checkboxes. With counts the client opens
    /// the voters of an answer instead, which Neucord doesn't have.
    pub tappable: bool,
    /// "Select one answer", or none once it is over.
    pub prompt: Option<&'static str>,
    pub primary: Option<Primary>,
    /// "Cancel" takes the place of the vote count.
    pub cancel: bool,
    /// "Show results", under the answers.
    pub show_results: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primary {
    /// Enabled once something is picked, and while nothing holds a vote back.
    Vote {
        enabled: bool,
    },
    RemoveVote,
    /// Results are showing to someone who has not voted.
    GoBackToVote,
}

/// A question's or an answer's text and emoji.
#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PollMedia {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub emoji: Option<Emoji>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Answer {
    pub answer_id: u32,
    #[serde(default)]
    pub poll_media: PollMedia,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct Results {
    #[serde(default)]
    pub is_finalized: bool,
    #[serde(default)]
    pub answer_counts: Vec<Count>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Count {
    pub id: u32,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub me_voted: bool,
}

impl Poll {
    fn count(&self, answer: u32) -> Option<&Count> {
        self.results.as_ref()?.answer_counts.iter().find(|c| c.id == answer)
    }

    pub fn votes(&self, answer: u32) -> u32 {
        self.count(answer).map_or(0, |c| c.count)
    }

    pub fn total_votes(&self) -> u32 {
        self.answers.iter().map(|a| self.votes(a.answer_id)).sum()
    }

    pub fn me_voted(&self, answer: u32) -> bool {
        self.count(answer).is_some_and(|c| c.me_voted)
    }

    /// An answer's share of the votes, in whole percent.
    pub fn percent(&self, answer: u32) -> u32 {
        match self.total_votes() {
            0 => 0,
            total => (100.0 * self.votes(answer) as f64 / total as f64).round() as u32,
        }
    }

    /// One of the answers with the most votes, if anyone voted.
    pub fn is_leader(&self, answer: u32) -> bool {
        let most = self.answers.iter().map(|a| self.votes(a.answer_id)).max().unwrap_or(0);
        let votes = self.votes(answer);
        votes != 0 && votes >= most
    }

    pub fn is_closed(&self, now_unix: i64) -> bool {
        self.results.as_ref().is_some_and(|r| r.is_finalized) || self.time_left(now_unix).is_none()
    }

    /// What is left of it, as the official client puts it: "2d left", "5h
    /// left", "3m left", "< 1m left". `None` once it is over; a poll that
    /// never closes has "".
    pub fn time_left(&self, now_unix: i64) -> Option<String> {
        let Some(expiry) = self.expiry.as_deref() else { return Some(String::new()) };
        let ends = glib::DateTime::from_iso8601(expiry, None).ok()?.to_unix();
        let left = ends - now_unix;
        if left <= 0 {
            return None;
        }
        let (days, hours, minutes) = (left / 86_400, left / 3_600, left / 60);
        Some(match () {
            _ if days > 1 => format!("{days}d left"),
            _ if hours > 1 => format!("{hours}h left"),
            _ if minutes > 0 => format!("{minutes}m left"),
            _ => "< 1m left".into(),
        })
    }

    /// The line under it: the votes, and the time left or that it closed.
    pub fn footer(&self, now_unix: i64) -> String {
        let votes = match self.total_votes() {
            1 => "1 vote".to_owned(),
            n => format!("{n} votes"),
        };
        match self.time_left(now_unix) {
            _ if self.is_closed(now_unix) => format!("{votes} • Poll closed"),
            Some(left) if !left.is_empty() => format!("{votes} • {left}"),
            _ => votes,
        }
    }

    /// A vote for `answer` arrived, as `addReaction` in 383233 counts a vote:
    /// ours is counted once, and in a DM the other side's only if no one else
    /// counted for the answer, since two people can't vote twice.
    pub fn vote(&mut self, answer: u32, me: bool, dm: bool) {
        let results = self.results.get_or_insert_with(Results::default);
        let Some(count) = results.answer_counts.iter_mut().find(|c| c.id == answer) else {
            results.answer_counts.push(Count { id: answer, count: 1, me_voted: me });
            return;
        };
        if dm && !me && count.count - u32::from(count.me_voted).min(count.count) >= 1 {
            return;
        }
        if !(me && count.me_voted) {
            count.count += 1;
        }
        count.me_voted |= me;
    }

    /// A vote for `answer` was taken back (`removeReaction`). Ours may be gone
    /// from the count already, and an answer left with no votes has no count.
    pub fn unvote(&mut self, answer: u32, me: bool) {
        let Some(results) = &mut self.results else { return };
        let Some(count) = results.answer_counts.iter_mut().find(|c| c.id == answer) else { return };
        if !(me && !count.me_voted) {
            count.count = count.count.saturating_sub(1);
        }
        count.me_voted &= !me;
        results.answer_counts.retain(|c| c.count > 0);
    }

    /// The answers we have votes on.
    pub fn my_votes(&self) -> Vec<u32> {
        let counts = self.results.iter().flat_map(|r| &r.answer_counts);
        counts.filter(|c| c.me_voted).map(|c| c.id).collect()
    }

    /// Make our votes `answers`, before Discord has said so (`D` in 180662):
    /// what we had and don't pick is taken back first, then the new ones are
    /// counted. The gateway repeating them changes nothing.
    pub fn set_my_votes(&mut self, answers: &[u32]) {
        let had = self.my_votes();
        for &answer in had.iter().filter(|a| !answers.contains(a)) {
            self.unvote(answer, true);
        }
        for &answer in answers.iter().filter(|a| !had.contains(a)) {
            self.vote(answer, true, false);
        }
    }

    /// What it offers us (`951727`). `held_back` is a quarantine or a timeout
    /// in the server, which keep a vote from being sent. The client also
    /// serves polls it doesn't let anyone interact with, such as a search
    /// hit; Neucord shows none of those.
    pub fn view(&self, held_back: bool, now_unix: i64) -> View {
        let ballot = &self.ballot;
        let expired = self.is_closed(now_unix);
        let recorded = !self.my_votes().is_empty();
        let voted = !ballot.editing && recorded;
        let counts = voted || expired || ballot.show_results;
        let can_tap = !recorded || ballot.editing || counts;
        let can_submit = !ballot.submitting && !ballot.picked.is_empty() && !voted && !held_back;
        let primary = if expired {
            None
        } else if ballot.editing {
            Some(Primary::Vote { enabled: !ballot.picked.is_empty() })
        } else if voted {
            Some(Primary::RemoveVote)
        } else if ballot.show_results {
            Some(Primary::GoBackToVote)
        } else {
            Some(Primary::Vote { enabled: can_submit })
        };
        View {
            counts,
            tappable: can_tap && !counts,
            prompt: (!expired).then_some(self.prompt()),
            primary,
            cancel: ballot.editing,
            show_results: !expired && !voted && !ballot.show_results,
        }
    }

    /// What the poll asks of a voter.
    fn prompt(&self) -> &'static str {
        if self.allow_multiselect { "Select one or more answers" } else { "Select one answer" }
    }
}

/// A closed poll's outcome, from its POLL_RESULT message.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    NoVotes,
    Winner { text: String, emoji: Option<Emoji>, percent: u32 },
    Tie,
}

/// What a POLL_RESULT message says about the poll it closes.
pub fn outcome(message: &Message) -> Option<Outcome> {
    let field = |name| crate::system::poll_result_field(message, name);
    let total: u32 = field("total_votes")?.parse().ok()?;
    if total == 0 {
        return Some(Outcome::NoVotes);
    }
    if field("victor_answer_id").is_none_or(str::is_empty) {
        return Some(Outcome::Tie);
    }
    let votes: u32 = field("victor_answer_votes").and_then(|v| v.parse().ok()).unwrap_or(0);
    let emoji = Emoji {
        id: field("victor_answer_emoji_id").and_then(|id| id.parse().ok()).map(Snowflake),
        name: field("victor_answer_emoji_name").filter(|n| !n.is_empty()).map(String::from),
        animated: field("victor_answer_emoji_animated") == Some("true"),
    };
    Some(Outcome::Winner {
        text: field("victor_answer_text").unwrap_or_default().to_owned(),
        emoji: (emoji.id.is_some() || emoji.name.is_some()).then_some(emoji),
        percent: (100.0 * votes as f64 / total as f64).round() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn poll(expiry: &str, counts: &str) -> Poll {
        serde_json::from_str(&format!(
            r#"{{"question":{{"text":"Lunch?"}},"allow_multiselect":false,"expiry":{expiry},
                "answers":[{{"answer_id":1,"poll_media":{{"text":"Soup"}}}},
                           {{"answer_id":2,"poll_media":{{"text":"Salad","emoji":{{"name":"🥗"}}}}}},
                           {{"answer_id":3,"poll_media":{{"text":"Both"}}}}],
                "results":{{"is_finalized":false,"answer_counts":[{counts}]}}}}"#
        ))
        .unwrap()
    }

    fn at(offset: i64) -> String {
        let time = glib::DateTime::from_unix_utc(NOW + offset).unwrap();
        format!("\"{}\"", time.format_iso8601().unwrap())
    }

    #[test]
    fn counts_and_shares() {
        let p = poll(&at(3600 * 5), r#"{"id":1,"count":2,"me_voted":false},{"id":2,"count":1,"me_voted":false}"#);
        assert_eq!((p.total_votes(), p.percent(1), p.percent(2), p.percent(3)), (3, 67, 33, 0));
        assert!(p.is_leader(1) && !p.is_leader(2) && !p.is_leader(3));
        assert!(!p.view(false, NOW).counts, "not voted, still open");
        assert_eq!(p.footer(NOW), "3 votes • 5h left");
        assert_eq!(p.view(false, NOW).prompt, Some("Select one answer"));
        let voted = poll(&at(60), r#"{"id":2,"count":1,"me_voted":true}"#);
        assert!(voted.view(false, NOW).counts && voted.me_voted(2));
        assert_eq!(voted.footer(NOW), "1 vote • 1m left");
    }

    #[test]
    fn time_left_as_the_official_client_says_it() {
        let left = |seconds: i64| poll(&at(seconds), "").time_left(NOW);
        assert_eq!(left(3 * 86_400), Some("3d left".into()));
        assert_eq!(left(47 * 3_600), Some("47h left".into()), "a day and some is still hours");
        assert_eq!(left(90 * 60), Some("90m left".into()));
        assert_eq!(left(30), Some("< 1m left".into()));
        assert_eq!(left(-5), None);
        let closed = poll(&at(-5), "");
        assert!(closed.is_closed(NOW) && closed.view(false, NOW).counts);
        assert_eq!(closed.footer(NOW), "0 votes • Poll closed");
        assert_eq!(poll("null", "").footer(NOW), "0 votes", "never closes");
    }

    #[test]
    fn poll_results() {
        let result = |fields: &str| -> Message {
            serde_json::from_str(&format!(
                r#"{{"id":"1","channel_id":"1","author":{{"id":"5","username":"a"}},"timestamp":"t","type":46,
                    "embeds":[{{"type":"poll_result","fields":[{fields}]}}]}}"#
            ))
            .unwrap()
        };
        let field = |name: &str, value: &str| format!(r#"{{"name":"{name}","value":"{value}"}}"#);
        assert_eq!(outcome(&result(&field("total_votes", "0"))), Some(Outcome::NoVotes));
        assert_eq!(outcome(&result(&field("total_votes", "4"))), Some(Outcome::Tie));
        let won = [
            field("total_votes", "4"),
            field("victor_answer_id", "2"),
            field("victor_answer_text", "Salad"),
            field("victor_answer_votes", "3"),
            field("victor_answer_emoji_name", "🥗"),
        ]
        .join(",");
        assert_eq!(
            outcome(&result(&won)),
            Some(Outcome::Winner {
                text: "Salad".into(),
                emoji: Some(Emoji { id: None, name: Some("🥗".into()), animated: false }),
                percent: 75
            })
        );
    }

    fn counts(p: &Poll) -> Vec<(u32, u32, bool)> {
        p.results.iter().flat_map(|r| &r.answer_counts).map(|c| (c.id, c.count, c.me_voted)).collect()
    }

    #[test]
    fn a_vote_counts_for_its_answer() {
        let mut p = poll("null", r#"{"id":1,"count":2,"me_voted":false}"#);
        p.vote(1, false, false);
        p.vote(2, true, false);
        assert_eq!(counts(&p), [(1, 3, false), (2, 1, true)], "a first vote makes its count");
        let mut empty: Poll = serde_json::from_str(r#"{"question":{},"answers":[{"answer_id":1}]}"#).unwrap();
        empty.vote(1, false, false);
        assert_eq!(empty.votes(1), 1, "a poll nobody had voted on has no results yet");
    }

    #[test]
    fn our_own_vote_is_counted_once() {
        // We voted here, so the count has it already when the gateway repeats it.
        let mut p = poll("null", r#"{"id":1,"count":2,"me_voted":true}"#);
        p.vote(1, true, false);
        assert_eq!(counts(&p), [(1, 2, true)]);
    }

    #[test]
    fn in_a_dm_the_other_side_votes_an_answer_once() {
        let mut p = poll("null", r#"{"id":1,"count":1,"me_voted":false}"#);
        p.vote(1, false, true);
        assert_eq!(p.votes(1), 1, "a repeat in a two-person chat");
        p.vote(1, false, false);
        assert_eq!(p.votes(1), 2, "anywhere else it is another voter");
        p.vote(1, true, true);
        assert_eq!(counts(&p), [(1, 3, true)], "ours always counts");
    }

    #[test]
    fn a_vote_taken_back_leaves_its_count() {
        let mut p = poll("null", r#"{"id":1,"count":2,"me_voted":true},{"id":2,"count":1,"me_voted":false}"#);
        p.unvote(1, false);
        p.unvote(2, false);
        assert_eq!(counts(&p), [(1, 1, true)], "the last vote of an answer takes its count with it");
        p.unvote(1, true);
        assert_eq!(counts(&p), [], "ours");
        p.unvote(3, false);
    }

    #[test]
    fn taking_back_ours_twice_counts_once() {
        // The client has taken it off already when the gateway says so.
        let mut p = poll("null", r#"{"id":1,"count":2,"me_voted":false}"#);
        p.unvote(1, true);
        assert_eq!(counts(&p), [(1, 2, false)]);
    }

    fn ballot(p: &mut Poll, taps: &[u32]) {
        for &answer in taps {
            p.ballot.tap(answer, p.allow_multiselect);
        }
    }

    #[test]
    fn tapping_picks_one_answer_or_many() {
        let mut p = poll("null", "");
        ballot(&mut p, &[1, 2]);
        assert_eq!(p.ballot.picked, [2], "a single-answer poll keeps the last");
        ballot(&mut p, &[2]);
        assert!(p.ballot.picked.is_empty(), "tapping it again puts it back");
        p.allow_multiselect = true;
        ballot(&mut p, &[3, 1, 2, 3]);
        assert_eq!(p.ballot.picked, [1, 2], "in the order picked");
    }

    #[test]
    fn what_an_open_poll_offers_before_a_vote() {
        let mut p = poll(&at(3600), "");
        let view = p.view(false, NOW);
        assert_eq!((view.counts, view.tappable, view.show_results), (false, true, true));
        assert_eq!(view.primary, Some(Primary::Vote { enabled: false }), "nothing picked");
        ballot(&mut p, &[2]);
        assert_eq!(p.view(false, NOW).primary, Some(Primary::Vote { enabled: true }));
        assert_eq!(p.view(true, NOW).primary, Some(Primary::Vote { enabled: false }), "held back");
        p.ballot.submitting = true;
        assert_eq!(p.view(false, NOW).primary, Some(Primary::Vote { enabled: false }), "already sending");
    }

    #[test]
    fn once_voted_counts_show_and_the_vote_can_be_taken_back() {
        let p = poll(&at(3600), r#"{"id":2,"count":1,"me_voted":true}"#);
        let view = p.view(false, NOW);
        assert_eq!((view.counts, view.tappable, view.show_results), (true, false, false));
        assert_eq!(view.primary, Some(Primary::RemoveVote));
        let taken = poll(&at(3600), r#"{"id":2,"count":1,"me_voted":false}"#);
        assert!(!taken.view(false, NOW).counts, "someone else's vote shows nothing");
    }

    #[test]
    fn show_results_without_voting() {
        let mut p = poll(&at(3600), r#"{"id":1,"count":2,"me_voted":false}"#);
        ballot(&mut p, &[3]);
        p.ballot.toggle_results();
        let view = p.view(false, NOW);
        assert!(p.ballot.picked.is_empty(), "what was picked is let go");
        assert_eq!((view.counts, view.tappable, view.show_results), (true, false, false));
        assert_eq!(view.primary, Some(Primary::GoBackToVote));
        p.ballot.toggle_results();
        assert_eq!(p.view(false, NOW).primary, Some(Primary::Vote { enabled: false }));
    }

    #[test]
    fn a_poll_that_is_over_offers_nothing() {
        let p = poll(&at(-5), r#"{"id":1,"count":2,"me_voted":false}"#);
        let view = p.view(false, NOW);
        assert_eq!(
            (view.counts, view.tappable, view.prompt, view.primary, view.show_results),
            (true, false, None, None, false)
        );
        let mut ended = poll(&at(3600), "");
        ended.results.as_mut().unwrap().is_finalized = true;
        assert_eq!(ended.view(false, NOW).primary, None, "ended early");
    }

    #[test]
    fn voting_while_results_show_edits_the_vote() {
        // Pressing "Show results" while the vote is on its way.
        let mut p = poll(&at(3600), "");
        p.ballot.submitting = true;
        p.ballot.toggle_results();
        assert!(p.ballot.editing);
        let view = p.view(false, NOW);
        assert_eq!(view.primary, Some(Primary::Vote { enabled: false }));
        assert!(view.cancel && view.counts);
        p.ballot.clear();
        assert_eq!(p.ballot, Ballot { show_results: true, ..Ballot::default() }, "cancelling keeps showing results");
    }

    #[test]
    fn a_vote_is_the_answers_picked() {
        let mut p = poll(&at(3600), r#"{"id":1,"count":2,"me_voted":true},{"id":2,"count":1,"me_voted":false}"#);
        assert_eq!(p.my_votes(), [1]);
        p.set_my_votes(&[2, 3]);
        assert_eq!(counts(&p), [(1, 1, false), (2, 2, true), (3, 1, true)], "1 is taken back, 2 and 3 counted");
        p.vote(3, true, false);
        assert_eq!(p.votes(3), 1, "the gateway repeating it adds nothing");
        p.set_my_votes(&[]);
        assert_eq!(counts(&p), [(1, 1, false), (2, 1, false)]);
        assert_eq!(p.my_votes(), Vec::<u32>::new());
    }

    #[test]
    fn a_failed_vote_is_put_back_as_it_was() {
        let mut p = poll(&at(3600), r#"{"id":1,"count":2,"me_voted":true}"#);
        let before = p.my_votes();
        let sent = {
            ballot(&mut p, &[2]);
            p.ballot.start_submit()
        };
        p.set_my_votes(&sent);
        p.set_my_votes(&before);
        p.ballot.failed();
        assert_eq!(counts(&p), [(1, 2, true)]);
        assert_eq!((p.ballot.picked.as_slice(), p.ballot.submitting), ([2].as_slice(), false), "still picked");
    }
}
