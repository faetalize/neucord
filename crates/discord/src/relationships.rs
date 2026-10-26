//! Friends, friend requests and blocks (Discord calls them all
//! relationships), and the presence of the people they concern.
//!
//! READY sends every relationship with its user, and `presences` for the
//! friends who are not offline. RELATIONSHIP_ADD then carries a whole
//! relationship, also when only its type changed (a request was accepted);
//! RELATIONSHIP_UPDATE and _REMOVE carry only the id and changed fields.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{Snowflake, User};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Friend,
    Blocked,
    /// A friend request someone sent us.
    Incoming,
    /// A friend request we sent.
    Outgoing,
    /// Implicit relationships and anything newer, which Neucord ignores.
    Other(u8),
}

impl Kind {
    fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Kind::Friend,
            2 => Kind::Blocked,
            3 => Kind::Incoming,
            4 => Kind::Outgoing,
            other => Kind::Other(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Relationship {
    pub user: User,
    pub kind: Kind,
    /// Our private nickname for a friend.
    pub nickname: Option<String>,
    /// Discord thinks this incoming request is spam.
    pub spam: bool,
    /// An incoming request from someone we share no server or friend with.
    /// Accepting needs an explicit confirmation.
    pub stranger: bool,
}

impl Relationship {
    /// The nickname we gave them, else their display name.
    pub fn name(&self) -> &str {
        self.nickname.as_deref().unwrap_or_else(|| self.user.display_name())
    }

    /// From READY's `relationships` or a RELATIONSHIP_ADD. The user comes
    /// inline, or under `users` (READY's deduplicated users, by id).
    pub fn from_value(value: &Value, users: &HashMap<&str, &Value>) -> Option<Self> {
        let id = value["id"].as_str().or_else(|| value["user_id"].as_str())?;
        let user = match &value["user"] {
            Value::Object(_) => User::deserialize(&value["user"]).ok()?,
            _ => User::deserialize(*users.get(id)?).ok()?,
        };
        Some(Relationship {
            user,
            kind: Kind::from_raw(value["type"].as_u64()? as u8),
            nickname: value["nickname"].as_str().filter(|n| !n.is_empty()).map(String::from),
            spam: value["is_spam_request"].as_bool() == Some(true),
            stranger: value["is_stranger_request"].as_bool() == Some(true),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Online,
    Idle,
    DoNotDisturb,
    /// Also how invisible users appear.
    #[default]
    Offline,
}

impl Status {
    fn parse(raw: &str) -> Self {
        match raw {
            "online" => Status::Online,
            "idle" => Status::Idle,
            "dnd" => Status::DoNotDisturb,
            _ => Status::Offline,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Idle => "Idle",
            Status::DoNotDisturb => "Do Not Disturb",
            Status::Offline => "Offline",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Presence {
    pub status: Status,
    /// What the friends list shows under the name: the custom status, else
    /// the first activity ("Playing …").
    pub text: Option<String>,
}

impl Presence {
    /// From a presence object (READY's `presences`, PRESENCE_UPDATE).
    pub fn from_value(value: &Value) -> Self {
        let activities = value["activities"].as_array().map(Vec::as_slice).unwrap_or_default();
        let custom = activities.iter().find(|a| a["type"].as_u64() == Some(4)).and_then(|a| {
            let emoji = a["emoji"]["name"].as_str().map(|name| match a["emoji"]["id"].as_str() {
                Some(_) => format!(":{name}:"),
                None => name.to_owned(),
            });
            let text = a["state"].as_str().filter(|s| !s.is_empty());
            match (emoji, text) {
                (Some(emoji), Some(text)) => Some(format!("{emoji} {text}")),
                (emoji, text) => emoji.or(text.map(String::from)),
            }
        });
        let activity = || {
            activities.iter().find_map(|a| {
                let name = a["name"].as_str()?;
                let verb = match a["type"].as_u64()? {
                    0 => "Playing",
                    1 => "Streaming",
                    2 => "Listening to",
                    3 => "Watching",
                    5 => "Competing in",
                    _ => return None,
                };
                Some(format!("{verb} {name}"))
            })
        };
        Presence { status: Status::parse(value["status"].as_str().unwrap_or_default()), text: custom.or_else(activity) }
    }
}

/// The lists of the friends page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum List {
    #[default]
    Online,
    All,
    /// Incoming requests, then outgoing ones, then likely spam.
    Pending,
    Blocked,
}

#[derive(Clone, Debug, Default)]
pub struct Relationships {
    by_user: HashMap<Snowflake, Relationship>,
    /// Only for users we have a relationship with; absent means offline.
    presences: HashMap<Snowflake, Presence>,
}

impl Relationships {
    /// From READY: its `relationships`, `presences` and `users`.
    pub fn from_ready(ready: &Value) -> Self {
        let users: HashMap<&str, &Value> = ready["users"]
            .as_array()
            .map(|users| users.iter().filter_map(|u| Some((u["id"].as_str()?, u))).collect())
            .unwrap_or_default();
        let mut this = Relationships::default();
        for raw in ready["relationships"].as_array().into_iter().flatten() {
            if let Some(relationship) = Relationship::from_value(raw, &users) {
                this.add(relationship);
            }
        }
        for raw in ready["presences"].as_array().into_iter().flatten() {
            if let Some(id) = raw["user"]["id"].as_str().and_then(|id| id.parse().ok()) {
                this.set_presence(Snowflake(id), Presence::from_value(raw));
            }
        }
        this
    }

    /// Insert or replace (RELATIONSHIP_ADD).
    pub fn add(&mut self, relationship: Relationship) {
        self.by_user.insert(relationship.user.id, relationship);
    }

    /// A RELATIONSHIP_UPDATE: so far only the nickname changes this way.
    pub fn update(&mut self, id: Snowflake, nickname: Option<String>) {
        if let Some(relationship) = self.by_user.get_mut(&id) {
            relationship.nickname = nickname;
        }
    }

    pub fn remove(&mut self, id: Snowflake) {
        self.by_user.remove(&id);
        self.presences.remove(&id);
    }

    pub fn get(&self, id: Snowflake) -> Option<&Relationship> {
        self.by_user.get(&id)
    }

    /// Record a presence, if it is someone we have a relationship with.
    /// Returns whether anything changed.
    pub fn set_presence(&mut self, id: Snowflake, presence: Presence) -> bool {
        if !self.by_user.contains_key(&id) {
            return false;
        }
        if presence.status == Status::Offline {
            return self.presences.remove(&id).is_some();
        }
        self.presences.insert(id, presence.clone()) != Some(presence)
    }

    pub fn presence(&self, id: Snowflake) -> Presence {
        self.presences.get(&id).cloned().unwrap_or_default()
    }

    pub fn is_blocked(&self, id: Snowflake) -> bool {
        self.by_user.get(&id).is_some_and(|r| r.kind == Kind::Blocked)
    }

    /// Friend requests waiting for us, not counting likely spam.
    pub fn incoming(&self) -> u32 {
        self.by_user.values().filter(|r| r.kind == Kind::Incoming && !r.spam).count() as u32
    }

    /// One list of the friends page, sorted as Discord sorts it.
    pub fn list(&self, list: List) -> Vec<&Relationship> {
        let rank = |r: &Relationship| match (r.kind, r.spam) {
            (Kind::Incoming, false) => 0,
            (Kind::Outgoing, _) => 1,
            _ => 2,
        };
        let mut out: Vec<&Relationship> = self
            .by_user
            .values()
            .filter(|r| match list {
                List::Online => r.kind == Kind::Friend && self.presences.contains_key(&r.user.id),
                List::All => r.kind == Kind::Friend,
                List::Pending => matches!(r.kind, Kind::Incoming | Kind::Outgoing),
                List::Blocked => r.kind == Kind::Blocked,
            })
            .collect();
        out.sort_by_cached_key(|r| (rank(r), r.name().to_lowercase(), r.user.id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready() -> Relationships {
        Relationships::from_ready(&json!({
            "relationships": [
                {"id": "1", "type": 1, "nickname": null, "user": {"id": "1", "username": "bea"}},
                {"id": "2", "type": 1, "nickname": "Al", "user": {"id": "2", "username": "zed"}},
                {"id": "3", "type": 2, "user": {"id": "3", "username": "troll"}},
                {"id": "4", "type": 3, "is_stranger_request": true, "user": {"id": "4", "username": "new"}},
                {"id": "5", "type": 3, "is_spam_request": true, "user": {"id": "5", "username": "spam"}},
                {"id": "6", "type": 4, "user_id": "6"},
                {"id": "7", "type": 5, "user": {"id": "7", "username": "implicit"}},
                {"id": "8", "type": 1}
            ],
            "users": [{"id": "6", "username": "asked"}],
            "presences": [
                {"user": {"id": "1"}, "status": "idle", "activities": []},
                {"user": {"id": "99"}, "status": "online", "activities": []}
            ]
        }))
    }

    fn names(list: Vec<&Relationship>) -> Vec<&str> {
        list.into_iter().map(Relationship::name).collect()
    }

    #[test]
    fn lists_from_ready() {
        let r = ready();
        assert_eq!(names(r.list(List::All)), ["Al", "bea"], "sorted by the name shown; no user, no entry");
        assert_eq!(names(r.list(List::Online)), ["bea"]);
        assert_eq!(names(r.list(List::Pending)), ["new", "asked", "spam"], "incoming, outgoing, spam");
        assert_eq!(names(r.list(List::Blocked)), ["troll"]);
        assert_eq!(r.incoming(), 1, "spam requests don't count");
        assert!(r.is_blocked(Snowflake(3)) && !r.is_blocked(Snowflake(1)));
        assert!(r.get(Snowflake(4)).unwrap().stranger && !r.get(Snowflake(5)).unwrap().stranger);
        assert_eq!(r.presence(Snowflake(99)), Presence::default(), "strangers' presences aren't kept");
    }

    #[test]
    fn live_changes() {
        let mut r = ready();
        // Accepting a request arrives as RELATIONSHIP_ADD with the new type.
        let accepted = json!({"id": "4", "type": 1, "user": {"id": "4", "username": "new"}});
        r.add(Relationship::from_value(&accepted, &HashMap::new()).unwrap());
        assert_eq!(names(r.list(List::All)), ["Al", "bea", "new"]);
        assert_eq!(r.incoming(), 0);

        r.update(Snowflake(4), Some("Newt".into()));
        assert_eq!(r.get(Snowflake(4)).unwrap().name(), "Newt");

        assert!(r.set_presence(Snowflake(4), Presence { status: Status::Online, text: None }));
        assert!(!r.set_presence(Snowflake(4), Presence { status: Status::Online, text: None }), "unchanged");
        assert_eq!(names(r.list(List::Online)), ["bea", "Newt"]);
        assert!(r.set_presence(Snowflake(4), Presence::default()), "went offline");
        assert_eq!(names(r.list(List::Online)), ["bea"]);

        r.remove(Snowflake(1));
        assert!(r.get(Snowflake(1)).is_none());
        assert_eq!(r.presence(Snowflake(1)), Presence::default());
    }

    #[test]
    fn presence_text() {
        let text = |activities: Value| Presence::from_value(&json!({"status": "dnd", "activities": activities})).text;
        assert_eq!(text(json!([])), None);
        assert_eq!(text(json!([{"type": 0, "name": "Chess"}])), Some("Playing Chess".into()));
        assert_eq!(
            text(json!([{"type": 0, "name": "Chess"}, {"type": 4, "name": "Custom Status", "state": "busy",
                         "emoji": {"name": "🔥"}}])),
            Some("🔥 busy".into()),
            "custom status first"
        );
        assert_eq!(text(json!([{"type": 4, "emoji": {"name": "blob", "id": "1"}}])), Some(":blob:".into()));
        let p = Presence::from_value(&json!({"status": "dnd"}));
        assert_eq!(p.status, Status::DoNotDisturb);
    }
}
