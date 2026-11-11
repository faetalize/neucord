//! Notes: the text we keep on a person, which only we see
//! (`/users/@me/notes/{id}`).
//!
//! The client loads one when something that shows it renders, if it holds
//! none for that person (`612630`): asked once, whatever the answer, and
//! never again until a new session (`NoteStore`, 279263, clears on
//! `CONNECTION_OPEN`). A note changed from another session arrives as
//! `USER_NOTE_UPDATE`.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use serde_json::Value;

use crate::Snowflake;

/// The longest note the client's box takes.
pub const MAX_LEN: usize = 256;

#[derive(Clone, Debug, Default)]
enum Held {
    /// Asked for, not answered.
    #[default]
    Loading,
    Loaded(Option<String>),
}

#[derive(Debug, Default)]
pub struct Notes {
    held: HashMap<Snowflake, Held>,
}

impl Notes {
    /// Whether to ask for `user`'s note: we hold nothing of it yet, not even
    /// that it is on its way. Marks it as on its way.
    pub fn start(&mut self, user: Snowflake) -> bool {
        match self.held.entry(user) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(Held::Loading);
                true
            }
        }
    }

    /// The note, or none where there isn't one or it didn't load.
    pub fn loaded(&mut self, user: Snowflake, note: Option<String>) {
        self.held.insert(user, Held::Loaded(note));
    }

    /// The note, once loaded; empty notes don't count.
    pub fn get(&self, user: Snowflake) -> Option<&str> {
        match self.held.get(&user)? {
            Held::Loaded(Some(note)) if !note.is_empty() => Some(note),
            _ => None,
        }
    }

    pub fn is_loading(&self, user: Snowflake) -> bool {
        matches!(self.held.get(&user), Some(Held::Loading))
    }

    /// A new session: what was held may have changed.
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

/// The note in an answer for `user`. An answer about someone else is no
/// answer (`note_user_id` is checked).
pub fn from_answer(answer: &Value, user: Snowflake) -> Option<String> {
    if answer["note_user_id"].as_str()? != user.to_string() {
        return None;
    }
    answer["note"].as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_note_is_asked_for_once_whatever_comes() {
        let mut notes = Notes::default();
        assert!(notes.start(Snowflake(1)));
        assert!(!notes.start(Snowflake(1)), "on its way");
        assert!(notes.is_loading(Snowflake(1)));
        notes.loaded(Snowflake(1), None);
        assert!(!notes.start(Snowflake(1)), "answered, with nothing");
        assert!(!notes.is_loading(Snowflake(1)));
    }

    #[test]
    fn a_new_session_asks_again() {
        let mut notes = Notes::default();
        notes.loaded(Snowflake(1), Some("hi".into()));
        assert_eq!(notes.get(Snowflake(1)), Some("hi"));
        notes.clear();
        assert!(notes.start(Snowflake(1)));
    }

    #[test]
    fn an_empty_note_is_none() {
        let mut notes = Notes::default();
        notes.loaded(Snowflake(1), Some(String::new()));
        assert_eq!(notes.get(Snowflake(1)), None);
    }

    #[test]
    fn an_answer_about_someone_else_is_none() {
        let answer = json!({"user_id": "5", "note_user_id": "2", "note": "hi"});
        assert_eq!(from_answer(&answer, Snowflake(2)).as_deref(), Some("hi"));
        assert_eq!(from_answer(&answer, Snowflake(3)), None);
        assert_eq!(from_answer(&json!({"note": "hi"}), Snowflake(2)), None);
    }
}
