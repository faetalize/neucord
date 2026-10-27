//! Polls, read the way the official client reads them: how many voted for
//! what, whether counts show yet, and how long is left.

use serde::Deserialize;

use crate::model::{Message, Snowflake};

#[derive(Clone, Debug, Deserialize)]
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
}

/// A question's or an answer's text and emoji.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PollMedia {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub emoji: Option<Emoji>,
}

/// A custom emoji has an id; a Unicode one only its name, the emoji itself.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<Snowflake>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Answer {
    pub answer_id: u32,
    #[serde(default)]
    pub poll_media: PollMedia,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Results {
    #[serde(default)]
    pub is_finalized: bool,
    #[serde(default)]
    pub answer_counts: Vec<Count>,
}

#[derive(Clone, Debug, Deserialize)]
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

    /// Counts show once we have voted, or the poll is over.
    pub fn shows_counts(&self, now_unix: i64) -> bool {
        self.is_closed(now_unix) || self.answers.iter().any(|a| self.me_voted(a.answer_id))
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

    /// What the poll asks of a voter.
    pub fn prompt(&self) -> &'static str {
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
        assert!(!p.shows_counts(NOW), "not voted, still open");
        assert_eq!(p.footer(NOW), "3 votes • 5h left");
        assert_eq!(p.prompt(), "Select one answer");
        let voted = poll(&at(60), r#"{"id":2,"count":1,"me_voted":true}"#);
        assert!(voted.shows_counts(NOW) && voted.me_voted(2));
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
        assert!(closed.is_closed(NOW) && closed.shows_counts(NOW));
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
}
