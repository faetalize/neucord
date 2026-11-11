//! Making a poll, as the official client's poll editor does (`eh` in 50518,
//! its `createPoll` in 180662): what it holds, what it refuses to post, and
//! the `poll` it sends. Voting and showing polls is [`crate::poll`].
//!
//! Only the text layout is made here: the client's other layout, answers
//! that are images, needs uploads and a picker for them.

use serde_json::{Value, json};

use crate::model::Emoji;

/// The editor's input limits, written into its fields (`maxLength` on the
/// question and the answers, and the answer count where one can be added).
pub const QUESTION_MAX: usize = 300;
pub const ANSWER_MAX: usize = 55;
pub const ANSWERS_MAX: usize = 10;

/// `layout_type` of a poll whose answers are text with an emoji at most.
const LAYOUT_DEFAULT: u8 = 1;

/// How long the poll runs, the editor's choices (`fT` in 412136), and the
/// hours the request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Duration {
    Hour = 1,
    FourHours = 4,
    EightHours = 8,
    #[default]
    Day = 24,
    ThreeDays = 72,
    Week = 168,
    TwoWeeks = 336,
}

impl Duration {
    pub const ALL: [Duration; 7] = [
        Duration::Hour,
        Duration::FourHours,
        Duration::EightHours,
        Duration::Day,
        Duration::ThreeDays,
        Duration::Week,
        Duration::TwoWeeks,
    ];

    pub fn hours(self) -> u32 {
        self as u32
    }

    /// The words the editor lists it by.
    pub fn label(self) -> &'static str {
        match self {
            Duration::Hour => "1 hour",
            Duration::FourHours => "4 hours",
            Duration::EightHours => "8 hours",
            Duration::Day => "24 hours",
            Duration::ThreeDays => "3 days",
            Duration::Week => "1 week",
            Duration::TwoWeeks => "2 weeks",
        }
    }
}

/// One answer: its text, and the emoji beside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    pub text: String,
    pub emoji: Option<Emoji>,
}

impl Answer {
    /// Whether it has text (`ND` in 969632). Only these are posted: an
    /// empty row is dropped, and one with just an emoji is refused.
    fn is_filled(&self) -> bool {
        !self.text.trim().is_empty()
    }
}

/// A poll being written.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub question: String,
    pub answers: Vec<Answer>,
    pub multiselect: bool,
    pub duration: Duration,
}

impl Default for Draft {
    /// Two empty answers and a day, as the editor opens.
    fn default() -> Draft {
        Draft {
            question: String::new(),
            answers: vec![Answer::default(), Answer::default()],
            multiselect: false,
            duration: Duration::default(),
        }
    }
}

/// What is missing, where the editor says so: on the question, and on the
/// answers by row.
#[derive(Debug, Default, PartialEq)]
pub struct Errors {
    pub question: Option<&'static str>,
    pub answers: Vec<Option<&'static str>>,
}

impl Errors {
    pub fn is_empty(&self) -> bool {
        self.question.is_none() && self.answers.iter().all(Option::is_none)
    }

    /// The row to put the cursor in: the question, else the first answer.
    pub fn first_answer(&self) -> Option<usize> {
        self.answers.iter().position(Option::is_some)
    }
}

impl Draft {
    /// Whether another answer fits, and whether one may go: a poll keeps
    /// at least one row.
    pub fn can_add(&self) -> bool {
        self.answers.len() < ANSWERS_MAX
    }

    pub fn can_remove(&self) -> bool {
        self.answers.len() > 1
    }

    /// What the editor refuses to post. A poll with no filled answer is
    /// told on the first row; an answer with an emoji and no text on its
    /// own, which wins there.
    pub fn check(&self) -> Errors {
        let mut errors = Errors { question: None, answers: vec![None; self.answers.len()] };
        if self.question.trim().is_empty() {
            errors.question = Some("Oops, you forgot something. Please add a question.");
        }
        if !self.answers.iter().any(Answer::is_filled)
            && let Some(first) = errors.answers.first_mut()
        {
            *first = Some("Oops, you forgot something. Please add at least one answer.");
        }
        for (answer, error) in self.answers.iter().zip(&mut errors.answers) {
            if answer.emoji.is_some() && !answer.is_filled() {
                *error = Some("Don\u{2019}t leave the answer field hanging! Please add some text.");
            }
        }
        errors
    }

    /// Whether leaving loses nothing, so no one is asked first (`Wn` in
    /// 969632): no question, and no answer with text. Emoji alone don't count.
    pub fn is_blank(&self) -> bool {
        self.question.is_empty() && !self.answers.iter().any(Answer::is_filled)
    }

    /// The `poll` of the message, as `createPoll` builds it. Text is
    /// trimmed. A custom emoji goes by its id alone, a Unicode one by itself.
    pub fn body(&self) -> Value {
        let answers: Vec<Value> = self
            .answers
            .iter()
            .filter(|a| a.is_filled())
            .map(|a| {
                let mut media = json!({ "text": a.text.trim() });
                match a.emoji.as_ref() {
                    Some(Emoji { id: Some(id), .. }) => media["emoji"] = json!({ "id": id, "name": "" }),
                    Some(Emoji { name: Some(name), .. }) => media["emoji"] = json!({ "name": name }),
                    _ => {}
                }
                json!({ "poll_media": media })
            })
            .collect();
        json!({
            "question": { "text": self.question.trim() },
            "answers": answers,
            "allow_multiselect": self.multiselect,
            "duration": self.duration.hours(),
            "layout_type": LAYOUT_DEFAULT,
        })
    }
}

/// What of `text` a field holding `held` more may take, as a `maxLength` of
/// `max` lets it in: the start of it that fits, in whole characters. Lengths
/// are the client's, UTF-16 units.
pub fn fit(text: &str, held: usize, max: usize) -> &str {
    let room = max.saturating_sub(held);
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > room {
            return &text[..at];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::Snowflake;

    fn answer(text: &str) -> Answer {
        Answer { text: text.to_owned(), emoji: None }
    }

    fn with_emoji(text: &str, emoji: Emoji) -> Answer {
        Answer { text: text.to_owned(), emoji: Some(emoji) }
    }

    fn draft(question: &str, answers: Vec<Answer>) -> Draft {
        Draft { question: question.to_owned(), answers, ..Draft::default() }
    }

    #[test]
    fn the_body_is_what_the_client_sends() {
        let custom = Emoji { id: Some(Snowflake(7)), name: Some("kek".into()), animated: true };
        let unicode = Emoji { id: None, name: Some("🔥".into()), animated: false };
        let mut poll = draft(
            "  Best snack?  ",
            vec![with_emoji(" Chips ", unicode), answer(""), answer("Fruit"), with_emoji("Cake", custom)],
        );
        poll.multiselect = true;
        poll.duration = Duration::Week;
        assert_eq!(
            poll.body(),
            json!({
                "question": { "text": "Best snack?" },
                "answers": [
                    { "poll_media": { "text": "Chips", "emoji": { "name": "🔥" } } },
                    { "poll_media": { "text": "Fruit" } },
                    { "poll_media": { "text": "Cake", "emoji": { "id": "7", "name": "" } } },
                ],
                "allow_multiselect": true,
                "duration": 168,
                "layout_type": 1,
            })
        );
    }

    #[test]
    fn a_poll_needs_a_question_and_an_answer_with_text() {
        let errors = Draft::default().check();
        assert!(errors.question.unwrap().contains("question"));
        assert!(errors.answers[0].unwrap().contains("at least one answer"));
        assert_eq!(errors.answers[1], None, "only the first row says so");
        assert_eq!(errors.first_answer(), Some(0));

        assert!(draft("Q?", vec![answer("A"), answer("")]).check().is_empty(), "an empty row is dropped");
        assert!(draft("   ", vec![answer("A")]).check().question.is_some(), "a question is trimmed");
        assert!(draft("Q?", vec![answer("  ")]).check().answers[0].is_some(), "and so is an answer");
    }

    #[test]
    fn an_emoji_alone_is_not_an_answer() {
        let emoji = Emoji { id: None, name: Some("🔥".into()), animated: false };
        let errors = draft("Q?", vec![answer("A"), with_emoji("", emoji.clone())]).check();
        assert_eq!(errors.answers[0], None);
        assert!(errors.answers[1].unwrap().contains("add some text"));

        let errors = draft("Q?", vec![with_emoji(" ", emoji)]).check();
        assert!(
            errors.answers[0].unwrap().contains("add some text"),
            "that is what the first row says, not no answers"
        );
    }

    #[test]
    fn leaving_asks_unless_something_is_written() {
        let emoji = Emoji { id: None, name: Some("🔥".into()), animated: false };
        assert!(Draft::default().is_blank());
        assert!(draft("", vec![with_emoji("", emoji)]).is_blank());
        assert!(!draft(" ", vec![answer("")]).is_blank(), "the question is not trimmed here");
        assert!(!draft("", vec![answer("A")]).is_blank());
        assert!(draft("", vec![answer("  ")]).is_blank());
    }

    #[test]
    fn answers_stop_at_ten_and_a_row_always_stays() {
        let mut poll = Draft::default();
        assert!(poll.can_add() && poll.can_remove());
        poll.answers.resize(ANSWERS_MAX, Answer::default());
        assert!(!poll.can_add());
        poll.answers.truncate(1);
        assert!(poll.can_add() && !poll.can_remove());
    }

    #[test]
    fn a_field_takes_what_fits_in_utf16_units() {
        assert_eq!(fit("hello", 0, 3), "hel");
        assert_eq!(fit("hello", 3, 5), "he");
        assert_eq!(fit("hello", 5, 5), "");
        assert_eq!(fit("hello", 9, 5), "", "already past it");
        assert_eq!(fit("hi", 0, 5), "hi");
        // 🙂 is two units: one fits in three units beside "a", a second does not.
        assert_eq!(fit("🙂🙂", 1, 4), "🙂");
        assert_eq!(fit("🙂", 0, 1), "", "never half of one");
    }

    #[test]
    fn the_durations_are_the_editors_hours() {
        let hours: Vec<u32> = Duration::ALL.iter().map(|d| d.hours()).collect();
        assert_eq!(hours, [1, 4, 8, 24, 72, 168, 336]);
        assert_eq!(Draft::default().duration.hours(), 24);
    }
}
