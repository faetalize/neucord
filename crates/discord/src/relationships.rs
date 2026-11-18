//! Friends, friend requests and blocks (Discord calls them all
//! relationships), and the presence of the people they concern.
//!
//! READY sends every relationship with its user, and `presences` for the
//! friends who are not offline. RELATIONSHIP_ADD then carries a whole
//! relationship, also when only its type changed (a request was accepted);
//! RELATIONSHIP_UPDATE and _REMOVE carry only the id and changed fields.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::presences::Presences;
use crate::settings::UserStatus;
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
    /// `user_ignored`: we ignored them, which hides a request and silences
    /// their messages without blocking.
    pub ignored: bool,
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
            ignored: value["user_ignored"].as_bool() == Some(true),
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

    /// How the status we set shows in a list: Invisible looks offline, as
    /// it does to everyone else.
    pub fn own(set: UserStatus) -> Self {
        match set {
            UserStatus::Online => Status::Online,
            UserStatus::Idle => Status::Idle,
            UserStatus::DoNotDisturb => Status::DoNotDisturb,
            UserStatus::Invisible => Status::Offline,
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
    /// Online on a phone and nowhere else (`isMobileOnline`, 290863): the
    /// client draws such a status as a phone.
    pub mobile: bool,
}

impl Presence {
    /// From a presence object (READY's `presences`, PRESENCE_UPDATE).
    pub fn from_value(value: &Value) -> Self {
        let client_status = &value["client_status"];
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
        Presence {
            status: Status::parse(value["status"].as_str().unwrap_or_default()),
            text: custom.or_else(activity),
            mobile: client_status["mobile"] == "online"
                && client_status["desktop"] != "online"
                && client_status["vr"] != "online",
        }
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
    /// Everyone's, not only friends'.
    presences: Presences,
    /// Whom we ignored. Kept apart from `by_user`: unfriending an ignored
    /// user removes the relationship but not the ignore.
    ignored: HashSet<Snowflake>,
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
                this.set_presence(None, Snowflake(id), Presence::from_value(raw));
            }
        }
        this
    }

    /// Insert or replace (RELATIONSHIP_ADD).
    pub fn add(&mut self, relationship: Relationship) {
        self.set_ignored(relationship.user.id, relationship.ignored);
        self.by_user.insert(relationship.user.id, relationship);
    }

    /// A RELATIONSHIP_UPDATE: the nickname and the ignore flag change this way.
    pub fn update(&mut self, id: Snowflake, nickname: Option<String>, ignored: bool) {
        self.set_ignored(id, ignored);
        if let Some(relationship) = self.by_user.get_mut(&id) {
            relationship.nickname = nickname;
            relationship.ignored = ignored;
        }
    }

    /// A RELATIONSHIP_REMOVE, whose `ignored` says the ignore outlives the
    /// relationship.
    pub fn remove(&mut self, id: Snowflake, ignored: bool) {
        self.by_user.remove(&id);
        // The account's own scope was for friends: no more is sent of them.
        // Whatever a server says of them stays.
        self.presences.set(None, id, Presence::default());
        if !ignored {
            self.ignored.remove(&id);
        }
    }

    fn set_ignored(&mut self, id: Snowflake, ignored: bool) {
        if ignored {
            self.ignored.insert(id);
        } else {
            self.ignored.remove(&id);
        }
    }

    pub fn get(&self, id: Snowflake) -> Option<&Relationship> {
        self.by_user.get(&id)
    }

    /// Record what `guild` (`None`: the account) says of `id`'s presence.
    /// Returns whether what shows of them changed.
    pub fn set_presence(&mut self, guild: Option<Snowflake>, id: Snowflake, presence: Presence) -> bool {
        self.presences.set(guild, id, presence)
    }

    /// A server was left: what it said of people goes.
    pub fn forget_guild(&mut self, guild: Snowflake) {
        self.presences.forget_guild(guild);
    }

    /// A member left a server. Whether what shows of them changed.
    pub fn forget_member(&mut self, guild: Snowflake, id: Snowflake) -> bool {
        self.presences.forget_member(guild, id)
    }

    /// Anyone's presence as far as we have heard it; offline if not.
    pub fn presence(&self, id: Snowflake) -> Presence {
        self.presences.get(id).cloned().unwrap_or_default()
    }

    /// `id`'s presence, if they are online somewhere we hear of.
    pub fn known_presence(&self, id: Snowflake) -> Option<&Presence> {
        self.presences.get(id)
    }

    pub fn is_blocked(&self, id: Snowflake) -> bool {
        self.by_user.get(&id).is_some_and(|r| r.kind == Kind::Blocked)
    }

    /// Ignored, and not blocked, as the client's `isIgnored` (994500) has it.
    pub fn is_ignored(&self, id: Snowflake) -> bool {
        self.ignored.contains(&id) && !self.is_blocked(id)
    }

    /// Whose messages neither count as unread or mentions, notify, nor show
    /// as typing (`isBlockedOrIgnored`).
    pub fn is_blocked_or_ignored(&self, id: Snowflake) -> bool {
        self.is_blocked(id) || self.is_ignored(id)
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
                List::Online => r.kind == Kind::Friend && self.presences.get(r.user.id).is_some(),
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
        assert_eq!(
            r.presence(Snowflake(99)).status,
            Status::Online,
            "a stranger's is kept too, as the client keeps it"
        );
        assert!(!r.list(List::Online).iter().any(|r| r.user.id == Snowflake(99)), "though they are no friend");
    }

    #[test]
    fn live_changes() {
        let mut r = ready();
        // Accepting a request arrives as RELATIONSHIP_ADD with the new type.
        let accepted = json!({"id": "4", "type": 1, "user": {"id": "4", "username": "new"}});
        r.add(Relationship::from_value(&accepted, &HashMap::new()).unwrap());
        assert_eq!(names(r.list(List::All)), ["Al", "bea", "new"]);
        assert_eq!(r.incoming(), 0);

        r.update(Snowflake(4), Some("Newt".into()), false);
        assert_eq!(r.get(Snowflake(4)).unwrap().name(), "Newt");

        assert!(r.set_presence(None, Snowflake(4), Presence { status: Status::Online, text: None, mobile: false }));
        assert!(
            !r.set_presence(None, Snowflake(4), Presence { status: Status::Online, text: None, mobile: false }),
            "unchanged"
        );
        assert_eq!(names(r.list(List::Online)), ["bea", "Newt"]);
        assert!(r.set_presence(None, Snowflake(4), Presence::default()), "went offline");
        assert_eq!(names(r.list(List::Online)), ["bea"]);

        r.remove(Snowflake(1), false);
        assert!(r.get(Snowflake(1)).is_none());
        assert_eq!(r.presence(Snowflake(1)), Presence::default());
    }

    #[test]
    fn ignored_users_are_read_from_the_relationships() {
        let user = |id: &str| json!({"id": id, "username": format!("u{id}")});
        let mut r = Relationships::from_ready(&json!({
            "relationships": [
                {"id": "1", "type": 1, "user": user("1")},
                {"id": "2", "type": 3, "user_ignored": true, "user": user("2")},
                {"id": "3", "type": 2, "user": user("3")},
                {"id": "4", "type": 1, "user_ignored": true, "user": user("4")},
                {"id": "5", "type": 2, "user_ignored": true, "user": user("5")}
            ]
        }));
        let id = Snowflake;
        assert!(!r.is_blocked_or_ignored(id(1)), "a plain friend");
        assert!(r.is_blocked_or_ignored(id(2)) && !r.is_blocked(id(2)), "an ignored request");
        assert!(r.is_blocked_or_ignored(id(3)), "blocked");
        assert!(r.is_blocked_or_ignored(id(4)), "an ignored friend");
        assert!(!r.is_blocked_or_ignored(id(6)), "a stranger");

        // RELATIONSHIP_ADD carries the whole relationship, flag included.
        let unignored = json!({"id": "4", "type": 1, "user": user("4")});
        r.add(Relationship::from_value(&unignored, &HashMap::new()).unwrap());
        assert!(!r.is_blocked_or_ignored(id(4)), "no flag, no longer ignored");

        r.update(id(1), None, true);
        assert!(r.is_ignored(id(1)), "RELATIONSHIP_UPDATE carries the flag too");
        r.remove(id(1), true);
        assert!(r.get(id(1)).is_none() && r.is_ignored(id(1)), "unfriending doesn't unignore");
        r.remove(id(1), false);
        assert!(!r.is_ignored(id(1)));

        r.add(
            Relationship::from_value(
                &json!({"id": "3", "type": 2, "user_ignored": true, "user": user("3")}),
                &HashMap::new(),
            )
            .unwrap(),
        );
        assert!(r.is_blocked_or_ignored(id(3)) && !r.is_ignored(id(3)), "blocked beats ignored");
    }

    #[test]
    fn our_own_status_shows_as_we_set_it() {
        assert_eq!(Status::own(UserStatus::Online), Status::Online);
        assert_eq!(Status::own(UserStatus::Idle), Status::Idle);
        assert_eq!(Status::own(UserStatus::DoNotDisturb), Status::DoNotDisturb);
        assert_eq!(Status::own(UserStatus::Invisible), Status::Offline, "invisible looks offline in a list");
    }

    #[test]
    fn only_a_phone_online_is_mobile() {
        let mobile = |client_status: Value| {
            Presence::from_value(&json!({"status": "online", "activities": [], "client_status": client_status})).mobile
        };
        assert!(mobile(json!({"mobile": "online"})));
        assert!(mobile(json!({"mobile": "online", "web": "online"})), "the web doesn't count");
        assert!(!mobile(json!({"mobile": "online", "desktop": "online"})), "a desktop beats it");
        assert!(!mobile(json!({"mobile": "online", "vr": "online"})), "so does VR");
        assert!(!mobile(json!({"mobile": "idle"})), "only online counts");
        assert!(!mobile(json!({"desktop": "online"})));
        assert!(!mobile(json!({})) && !mobile(Value::Null));
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
