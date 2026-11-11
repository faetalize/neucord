//! The accounts logged in on this device, as the client's
//! `MultiAccountStore` keeps them (274303): who they are, in the order they
//! were added, and whether their token still works. The tokens themselves
//! are in the keyring ([`crate::session`]).
//!
//! The client keeps one gateway connection, for the account it shows.
//! Switching closes it and connects the other account's token afresh; the
//! other accounts are only ever asked about with `GET /users/@me`, when a
//! list of them opens (`validate` in 429707, [`Roster::validated`]).

use serde::{Deserialize, Serialize};

use crate::{Error, Snowflake, User};

/// How many accounts the client keeps: `MultiAccountStore` drops what comes
/// after the fifth, and the Manage Accounts modal (623161) refuses a sixth.
pub const MAX_ACCOUNTS: usize = 5;

/// What the client knows of an account's token (`u` in 274303).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Discord turned it down, or there is none: the account must log in again.
    Invalid,
    Validating,
    #[default]
    Valid,
}

/// One account of the list. What a list shows of it comes from here, so it
/// shows without a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Snowflake,
    pub username: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(skip)]
    pub status: Status,
}

impl Entry {
    fn of(user: &User) -> Self {
        Entry {
            id: user.id,
            username: user.username.clone(),
            avatar: user.avatar.clone(),
            discriminator: user.discriminator.clone(),
            status: Status::Valid,
        }
    }

    fn update(&mut self, user: &User) {
        self.username = user.username.clone();
        self.avatar = user.avatar.clone();
        self.discriminator = user.discriminator.clone();
    }

    /// As the lists name an account: its username, and its `#discriminator`
    /// unless it has none (`hasUniqueUsername` in 619019 is a discriminator
    /// of "0").
    pub fn tag(&self) -> String {
        match self.discriminator.as_deref() {
            None | Some("" | "0") => self.username.clone(),
            Some(discriminator) => format!("{}#{discriminator}", self.username),
        }
    }

    /// The account as a [`User`], for drawing its avatar.
    pub fn user(&self) -> User {
        User {
            id: self.id,
            username: self.username.clone(),
            global_name: None,
            avatar: self.avatar.clone(),
            bot: false,
            system: false,
            discriminator: self.discriminator.clone(),
        }
    }
}

/// What asking Discord about a stored token came to.
#[derive(Debug)]
pub enum Answer {
    /// The keyring holds none: nothing is asked.
    NoToken,
    User(User),
    /// 401 or 403.
    Rejected,
    /// Anything else: the token isn't blamed for it.
    Unreachable,
}

impl Answer {
    pub fn of(result: Result<User, Error>) -> Self {
        match result {
            Ok(user) => Answer::User(user),
            Err(Error::Unauthorized | Error::Http { status: 401 | 403, .. }) => Answer::Rejected,
            Err(_) => Answer::Unreachable,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    users: Vec<Entry>,
    /// The account that was shown last, which the next launch opens.
    #[serde(default)]
    current: Option<Snowflake>,
}

impl Roster {
    /// Reads what [`Self::to_json`] wrote; anything else is an empty list,
    /// which only costs logging in again.
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a roster is plain data")
    }

    pub fn entries(&self) -> &[Entry] {
        &self.users
    }

    pub fn get(&self, id: Snowflake) -> Option<&Entry> {
        self.users.iter().find(|e| e.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    /// Whether another account may be added: the Manage Accounts modal says
    /// "Sorry, you can only add 5 accounts." instead.
    pub fn is_full(&self) -> bool {
        self.users.len() >= MAX_ACCOUNTS
    }

    pub fn current(&self) -> Option<Snowflake> {
        self.current
    }

    /// An account is connected (`CONNECTION_OPEN`): added after the others,
    /// or refreshed. The client keeps the first five; any beyond are
    /// returned, for their tokens to go.
    pub fn opened(&mut self, user: &User) -> Vec<Snowflake> {
        match self.users.iter_mut().find(|e| e.id == user.id) {
            Some(entry) => {
                entry.update(user);
                entry.status = Status::Valid;
            }
            None => self.users.push(Entry::of(user)),
        }
        self.current = Some(user.id);
        if self.users.len() <= MAX_ACCOUNTS {
            return Vec::new();
        }
        let dropped: Vec<Snowflake> = self.users.drain(MAX_ACCOUNTS..).map(|e| e.id).collect();
        if dropped.contains(&user.id) {
            self.current = None;
        }
        dropped
    }

    /// The user changed their name or avatar (`CURRENT_USER_UPDATE`).
    pub fn updated(&mut self, user: &User) {
        if let Some(entry) = self.users.iter_mut().find(|e| e.id == user.id) {
            entry.update(user);
        }
    }

    pub fn remove(&mut self, id: Snowflake) {
        self.users.retain(|e| e.id != id);
        if self.current == Some(id) {
            self.current = None;
        }
    }

    pub fn validating(&mut self, id: Snowflake) {
        self.set_status(id, Status::Validating);
    }

    /// What asking about `id`'s token came to (`validate` in 429707): a
    /// rejection or another account's answer marks it invalid; anything
    /// that isn't a rejection leaves it valid. What answers for an account
    /// other than the shown one refreshes its name and avatar.
    pub fn validated(&mut self, id: Snowflake, answer: Answer) {
        let status = match &answer {
            Answer::NoToken | Answer::Rejected => Status::Invalid,
            Answer::User(user) if user.id != id => Status::Invalid,
            Answer::User(user) => {
                if self.current != Some(id) {
                    self.updated(user);
                }
                Status::Valid
            }
            Answer::Unreachable => Status::Valid,
        };
        self.set_status(id, status);
    }

    pub fn is_validating(&self) -> bool {
        self.users.iter().any(|e| e.status == Status::Validating)
    }

    fn set_status(&mut self, id: Snowflake, status: Status) {
        if let Some(entry) = self.users.iter_mut().find(|e| e.id == id) {
            entry.status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64, name: &str) -> User {
        User {
            id: Snowflake(id),
            username: name.into(),
            global_name: None,
            avatar: Some(format!("av{id}")),
            bot: false,
            system: false,
            discriminator: Some("0".into()),
        }
    }

    fn ids(roster: &Roster) -> Vec<u64> {
        roster.entries().iter().map(|e| e.id.0).collect()
    }

    #[test]
    fn accounts_keep_the_order_they_were_added_in() {
        let mut roster = Roster::default();
        roster.opened(&user(2, "b"));
        roster.opened(&user(1, "a"));
        roster.opened(&user(2, "b2"));
        assert_eq!(ids(&roster), [2, 1]);
        assert_eq!(roster.get(Snowflake(2)).unwrap().username, "b2", "a known account is refreshed");
        assert_eq!(roster.current(), Some(Snowflake(2)));
    }

    #[test]
    fn a_sixth_account_is_dropped_with_its_token() {
        let mut roster = Roster::default();
        for id in 1..=5 {
            assert!(roster.opened(&user(id, "x")).is_empty());
        }
        assert!(roster.is_full());
        assert_eq!(roster.opened(&user(6, "x")), [Snowflake(6)]);
        assert_eq!(ids(&roster), [1, 2, 3, 4, 5]);
        assert_eq!(roster.current(), None, "the one dropped is not remembered");
        assert!(roster.opened(&user(3, "x")).is_empty(), "a known account is never over the limit");
    }

    #[test]
    fn removing_forgets_the_current_account() {
        let mut roster = Roster::default();
        roster.opened(&user(1, "a"));
        roster.opened(&user(2, "b"));
        roster.remove(Snowflake(1));
        assert_eq!((ids(&roster), roster.current()), (vec![2], Some(Snowflake(2))));
        roster.remove(Snowflake(2));
        assert_eq!((roster.is_empty(), roster.current()), (true, None));
    }

    #[test]
    fn validation_follows_the_client() {
        let mut roster = Roster::default();
        roster.opened(&user(1, "a"));
        roster.opened(&user(2, "b"));
        let status = |roster: &Roster, id| roster.get(Snowflake(id)).unwrap().status;

        roster.validating(Snowflake(1));
        assert!(roster.is_validating());
        roster.validated(Snowflake(1), Answer::Rejected);
        assert_eq!(status(&roster, 1), Status::Invalid);
        assert!(!roster.is_validating());
        roster.validated(Snowflake(1), Answer::Unreachable);
        assert_eq!(status(&roster, 1), Status::Valid, "only a rejection blames the token");
        roster.validated(Snowflake(1), Answer::NoToken);
        assert_eq!(status(&roster, 1), Status::Invalid);
        roster.validated(Snowflake(1), Answer::User(user(9, "other")));
        assert_eq!(status(&roster, 1), Status::Invalid, "a token that belongs to someone else");

        // Another account's answer refreshes it; the shown account's doesn't.
        roster.validated(Snowflake(1), Answer::User(user(1, "renamed")));
        assert_eq!(roster.get(Snowflake(1)).unwrap().username, "renamed");
        roster.validated(Snowflake(2), Answer::User(user(2, "ignored")));
        assert_eq!(roster.get(Snowflake(2)).unwrap().username, "b");
    }

    #[test]
    fn answers_from_requests() {
        assert!(matches!(Answer::of(Err(Error::Unauthorized)), Answer::Rejected));
        assert!(matches!(Answer::of(Err(Error::Http { status: 403, body: String::new() })), Answer::Rejected));
        assert!(matches!(Answer::of(Err(Error::Http { status: 500, body: String::new() })), Answer::Unreachable));
        assert!(matches!(Answer::of(Ok(user(1, "a"))), Answer::User(_)));
    }

    #[test]
    fn the_list_survives_the_keyring() {
        let mut roster = Roster::default();
        roster.opened(&user(1, "a"));
        roster.validated(Snowflake(1), Answer::Rejected);
        let back = Roster::from_json(&roster.to_json());
        assert_eq!(ids(&back), [1]);
        assert_eq!(back.current(), Some(Snowflake(1)));
        assert_eq!(back.get(Snowflake(1)).unwrap().status, Status::Valid, "whether a token works is asked again");
        assert!(Roster::from_json("not json").is_empty());
    }

    #[test]
    fn tags_show_a_discriminator_only_when_there_is_one() {
        let mut entry = Entry::of(&user(1, "a"));
        assert_eq!(entry.tag(), "a");
        entry.discriminator = Some("1234".into());
        assert_eq!(entry.tag(), "a#1234");
        entry.discriminator = None;
        assert_eq!(entry.tag(), "a");
    }
}
