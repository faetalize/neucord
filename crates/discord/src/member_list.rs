//! A server's member list: the rows the gateway sent for the ranges we
//! subscribed to ([`crate::subscriptions`]), kept as `ChannelMemberStore`
//! (963307) keeps them.
//!
//! Discord sends the list as `GUILD_MEMBER_LIST_UPDATE`: the groups it is
//! made of with their sizes, and operations on rows by index. A list has
//! a header row for each group followed by that group's members, so a row's
//! index is its place in the whole list, rows we asked for or not. Rows
//! outside the ranges we hold aren't loaded; a view of the list shows them
//! as such until a `SYNC` fills them.
//!
//! Channels that the same people can see share a list, named by an id that
//! the events carry ([`list_id`] finds which a channel shows).

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::experiments::murmur3;
use crate::permissions::Permissions;
use crate::relationships::Presence;
use crate::{Channel, Guild, Snowflake, User};

/// The group of members who are on, when no role has its own.
pub const ONLINE: &str = "online";
pub const OFFLINE: &str = "offline";
/// What a list is made of before Discord has sent its groups.
pub const UNKNOWN: &str = "unknown";

/// The id of the list `channel` of `guild` shows (`D`, 963307): what the
/// channel says, else `everyone` where `@everyone` sees it, else a hash of
/// who is allowed or denied it, which the server groups channels by.
pub fn list_id(guild: &Guild, channel: &Channel) -> String {
    if let Some(id) = &channel.member_list_id {
        return id.clone();
    }
    let denies = |o: &crate::Overwrite| o.deny.contains(Permissions::VIEW_CHANNEL);
    let everyone = guild.role(guild.id).is_some_and(|r| r.permissions.contains(Permissions::VIEW_CHANNEL));
    if everyone && !channel.permission_overwrites.iter().any(denies) {
        return "everyone".to_owned();
    }
    let mut views: Vec<String> = channel
        .permission_overwrites
        .iter()
        .filter_map(|o| {
            if o.allow.contains(Permissions::VIEW_CHANNEL) {
                Some(format!("allow:{}", o.id))
            } else {
                denies(o).then(|| format!("deny:{}", o.id))
            }
        })
        .collect();
    views.sort();
    murmur3(&views.join(",")).to_string()
}

/// A group of the list and how many it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// [`ONLINE`], [`OFFLINE`], or the id of a hoisted role.
    pub id: String,
    pub count: u32,
}

/// A member as a list row carries them.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub user: User,
    pub nick: Option<String>,
    /// Their picture in this server.
    pub avatar: Option<String>,
    pub roles: Vec<Snowflake>,
    pub presence: Presence,
}

impl Member {
    /// The name the list shows: the nickname, else the user's own.
    pub fn name(&self) -> &str {
        self.nick.as_deref().filter(|n| !n.is_empty()).unwrap_or_else(|| self.user.display_name())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Group(Group),
    Member(Member),
}

impl Item {
    fn from_value(value: &Value) -> Option<Item> {
        if let Some(group) = value["group"].as_object() {
            let id = group.get("id")?.as_str()?.to_owned();
            return Some(Item::Group(Group { id, count: count(group.get("count"))? }));
        }
        let member = value["member"].as_object().filter(|_| value["member"]["user"].is_object())?;
        let roles = member.get("roles").and_then(Value::as_array).into_iter().flatten();
        Some(Item::Member(Member {
            user: User::deserialize(&value["member"]["user"]).ok()?,
            nick: member.get("nick").and_then(Value::as_str).map(str::to_owned),
            avatar: member.get("avatar").and_then(Value::as_str).map(str::to_owned),
            roles: roles.filter_map(|r| r.as_str()?.parse().ok().map(Snowflake)).collect(),
            presence: match &value["member"]["presence"] {
                presence @ Value::Object(_) => Presence::from_value(presence),
                _ => Presence::default(),
            },
        }))
    }
}

fn count(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()
}

/// A change to the rows. Indexes are rows of the whole list.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// These rows, from the start of `range`. A row that isn't one we can
    /// read is `None`, and leaves what we had.
    Sync {
        start: u32,
        items: Vec<Option<Item>>,
    },
    /// The rows of `range` are no longer ours to show.
    Invalidate {
        range: (u32, u32),
    },
    Insert {
        index: u32,
        item: Item,
    },
    Update {
        index: u32,
        item: Item,
    },
    Delete {
        index: u32,
    },
}

impl Op {
    fn from_value(value: &Value) -> Option<Op> {
        let index = || count(value.get("index"));
        let item = || Item::from_value(&value["item"]);
        Some(match value["op"].as_str()? {
            "SYNC" => Op::Sync {
                start: range(&value["range"])?.0,
                items: value["items"].as_array()?.iter().map(Item::from_value).collect(),
            },
            "INVALIDATE" => Op::Invalidate { range: range(&value["range"])? },
            "INSERT" => Op::Insert { index: index()?, item: item()? },
            "UPDATE" => Op::Update { index: index()?, item: item()? },
            "DELETE" => Op::Delete { index: index()? },
            _ => return None,
        })
    }
}

fn range(value: &Value) -> Option<(u32, u32)> {
    let [a, b] = value.as_array()?.as_slice() else { return None };
    Some((count(Some(a))?, count(Some(b))?))
}

/// A `GUILD_MEMBER_LIST_UPDATE`.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub guild: Snowflake,
    /// Names the list: see [`list_id`].
    pub list: String,
    pub ops: Vec<Op>,
    pub groups: Vec<Group>,
    pub member_count: u64,
    pub online_count: u64,
}

impl Update {
    pub fn from_value(data: &Value) -> Option<Update> {
        Some(Update {
            guild: data["guild_id"].as_str()?.parse().ok().map(Snowflake)?,
            list: data["id"].as_str()?.to_owned(),
            ops: data["ops"].as_array()?.iter().filter_map(Op::from_value).collect(),
            groups: data["groups"]
                .as_array()?
                .iter()
                .filter_map(|g| Some(Group { id: g["id"].as_str()?.to_owned(), count: count(g.get("count"))? }))
                .collect(),
            member_count: data["member_count"].as_u64().unwrap_or(0),
            online_count: data["online_count"].as_u64().unwrap_or(0),
        })
    }

    /// The members this carries rows for, which `SYNC`, `INSERT` and
    /// `UPDATE` bring.
    pub fn members(&self) -> Vec<&Member> {
        let mut members = Vec::new();
        for op in &self.ops {
            let items: Vec<&Item> = match op {
                Op::Sync { items, .. } => items.iter().flatten().collect(),
                Op::Insert { item, .. } | Op::Update { item, .. } => vec![item],
                Op::Invalidate { .. } | Op::Delete { .. } => Vec::new(),
            };
            members.extend(items.into_iter().filter_map(|item| match item {
                Item::Member(member) => Some(member),
                Item::Group(_) => None,
            }));
        }
        members
    }
}

/// A group in place: where its header row is.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupAt {
    pub id: String,
    pub count: u32,
    pub index: usize,
}

/// What a place in the list holds.
#[derive(Debug, PartialEq)]
pub enum Slot<'a> {
    Header(&'a GroupAt),
    Member(&'a Member),
    /// A row we haven't been sent.
    Loading,
}

/// Rows that changed, as a list model announces them: from `position`,
/// `removed` rows gave way to `added`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub position: usize,
    pub removed: usize,
    pub added: usize,
}

/// One list.
#[derive(Debug)]
pub struct MemberList {
    rows: Vec<Option<Item>>,
    groups: Vec<GroupAt>,
    pub member_count: u64,
    pub online_count: u64,
}

impl Default for MemberList {
    fn default() -> Self {
        let mut list = MemberList { rows: Vec::new(), groups: Vec::new(), member_count: 0, online_count: 0 };
        list.set_groups(&[Group { id: UNKNOWN.to_owned(), count: 0 }]);
        list
    }
}

impl MemberList {
    /// How many rows the list has, headers included.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether Discord hasn't told us what the list holds yet.
    pub fn is_unknown(&self) -> bool {
        matches!(self.groups.as_slice(), [only] if only.id == UNKNOWN)
    }

    pub fn groups(&self) -> &[GroupAt] {
        &self.groups
    }

    pub fn slot(&self, index: usize) -> Slot<'_> {
        let at = self.groups.partition_point(|g| g.index <= index);
        match at.checked_sub(1).map(|i| &self.groups[i]) {
            Some(group) if group.index == index => Slot::Header(group),
            _ => match self.rows.get(index) {
                Some(Some(Item::Member(member))) => Slot::Member(member),
                _ => Slot::Loading,
            },
        }
    }

    fn set_groups(&mut self, groups: &[Group]) {
        let mut index = 0;
        self.groups = groups
            .iter()
            .map(|g| {
                let at = GroupAt { id: g.id.clone(), count: g.count, index };
                index += g.count as usize + 1;
                at
            })
            .collect();
        self.rows.resize(index, None);
    }

    fn set(&mut self, index: usize, item: Item) {
        if index >= self.rows.len() {
            self.rows.resize(index + 1, None);
        }
        self.rows[index] = Some(item);
    }

    /// Apply `update`'s operations, then its groups, as the client does.
    /// What changed, if anything did.
    pub fn apply(&mut self, update: &Update) -> Option<Change> {
        let before = self.rows.len();
        let old_groups = self.groups.clone();
        // The first row that changed, and the end of those that did, or
        // `usize::MAX` where the rows after it moved.
        let (mut first, mut last) = (usize::MAX, 0usize);
        let mut touch = |from: usize, to: usize| {
            first = first.min(from);
            last = if to == usize::MAX { to } else { last.max(to) };
        };
        for op in &update.ops {
            match op {
                Op::Sync { start, items } => {
                    let start = *start as usize;
                    for (i, item) in items.iter().enumerate() {
                        if let Some(item) = item {
                            self.set(start + i, item.clone());
                        }
                    }
                    touch(start, start + items.len());
                }
                Op::Invalidate { range: (from, to) } => {
                    let from = *from as usize;
                    // Stops at the first row we don't hold.
                    let mut end = from;
                    for i in from..=*to as usize {
                        match self.rows.get_mut(i) {
                            Some(row @ Some(_)) => *row = None,
                            _ => break,
                        }
                        end = i + 1;
                    }
                    touch(from, end);
                }
                Op::Insert { index, item } => {
                    // Past the end, it goes last.
                    let index = (*index as usize).min(self.rows.len());
                    self.rows.insert(index, Some(item.clone()));
                    touch(index, usize::MAX);
                }
                Op::Update { index, item } => {
                    self.set(*index as usize, item.clone());
                    touch(*index as usize, *index as usize + 1);
                }
                Op::Delete { index } => {
                    let index = *index as usize;
                    if matches!(self.rows.get(index), Some(Some(_))) {
                        self.rows.remove(index);
                        touch(index, usize::MAX);
                    }
                }
            }
        }
        self.set_groups(&update.groups);
        self.member_count = update.member_count;
        self.online_count = update.online_count;

        // Headers move with the counts above them.
        let moved = old_groups.iter().zip(&self.groups).position(|(a, b)| a != b);
        let moved = moved.or((old_groups.len() != self.groups.len()).then(|| old_groups.len().min(self.groups.len())));
        if let Some(group) = moved {
            let at = self.groups.get(group).or(old_groups.get(group)).map_or(0, |g| g.index);
            touch(at, usize::MAX);
        }
        let after = self.rows.len();
        if first == usize::MAX && before == after {
            return None;
        }
        let first = first.min(before.min(after));
        if last == usize::MAX || before != after {
            Some(Change { position: first, removed: before - first, added: after - first })
        } else {
            let span = last.min(after) - first;
            Some(Change { position: first, removed: span, added: span })
        }
    }

    /// `user`'s presence is `presence`: the row it is in, if we hold one.
    pub fn set_presence(&mut self, user: Snowflake, presence: &Presence) -> Option<Change> {
        let position = self.rows.iter().position(|row| matches!(row, Some(Item::Member(m)) if m.user.id == user))?;
        let Some(Item::Member(member)) = &mut self.rows[position] else { return None };
        (member.presence != *presence).then(|| {
            member.presence = presence.clone();
            Change { position, removed: 1, added: 1 }
        })
    }
}

/// Every list we hold, by server and [`Update::list`].
#[derive(Debug, Default)]
pub struct MemberLists(HashMap<(Snowflake, String), MemberList>);

impl MemberLists {
    pub fn get(&self, guild: Snowflake, list: &str) -> Option<&MemberList> {
        self.0.get(&(guild, list.to_owned()))
    }

    /// The list, made as the client does for one it has none of.
    pub fn apply(&mut self, update: &Update) -> Option<Change> {
        self.0.entry((update.guild, update.list.clone())).or_default().apply(update)
    }

    /// A user's presence changed in `guild`, or anywhere with `None`:
    /// the rows of each list it changed.
    pub fn set_presence(
        &mut self,
        guild: Option<Snowflake>,
        user: Snowflake,
        presence: &Presence,
    ) -> Vec<(Snowflake, String, Change)> {
        let mut changes = Vec::new();
        for ((g, id), list) in &mut self.0 {
            if guild.is_none_or(|guild| guild == *g)
                && let Some(change) = list.set_presence(user, presence)
            {
                changes.push((*g, id.clone(), change));
            }
        }
        changes
    }

    /// A new session: the lists we hold may have missed events
    /// (`CONNECTION_OPEN`).
    pub fn reset(&mut self) {
        self.0.clear();
    }

    pub fn remove_guild(&mut self, guild: Snowflake) {
        self.0.retain(|(g, _), _| *g != guild);
    }
}

/// The people of a group DM, as its list shows them (`sS`, 289287): us and
/// the others, by username.
pub fn group_members<'a>(me: &'a User, recipients: &'a [User]) -> Vec<&'a User> {
    let mut people: Vec<&User> = std::iter::once(me).chain(recipients).collect();
    people.sort_by_key(|u| u.username.to_lowercase());
    people
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const G: Snowflake = Snowflake(1);

    fn member(id: u64, name: &str) -> Item {
        Item::Member(Member {
            user: serde_json::from_value(json!({"id": id.to_string(), "username": name})).unwrap(),
            nick: None,
            avatar: None,
            roles: Vec::new(),
            presence: Presence::default(),
        })
    }

    fn group(id: &str, count: u32) -> Group {
        Group { id: id.to_owned(), count }
    }

    fn update(ops: Vec<Op>, groups: Vec<Group>) -> Update {
        Update { guild: G, list: "everyone".into(), ops, groups, member_count: 0, online_count: 0 }
    }

    fn names(list: &MemberList) -> Vec<String> {
        (0..list.len())
            .map(|i| match list.slot(i) {
                Slot::Header(g) => format!("[{}]", g.id),
                Slot::Member(m) => m.user.username.clone(),
                Slot::Loading => "_".to_owned(),
            })
            .collect()
    }

    fn change(position: usize, removed: usize, added: usize) -> Option<Change> {
        Some(Change { position, removed, added })
    }

    fn synced() -> MemberList {
        let mut list = MemberList::default();
        let items = vec![
            Some(Item::Group(group("online", 2))),
            Some(member(10, "ann")),
            Some(member(11, "bob")),
            Some(Item::Group(group("offline", 1))),
            Some(member(12, "cy")),
        ];
        list.apply(&update(vec![Op::Sync { start: 0, items }], vec![group("online", 2), group("offline", 1)]));
        list
    }

    #[test]
    fn a_list_starts_unknown() {
        let list = MemberList::default();
        assert!(list.is_unknown());
        assert_eq!(names(&list), ["[unknown]"]);
    }

    #[test]
    fn rows_not_sent_are_loading() {
        let mut list = MemberList::default();
        let groups = vec![group("online", 3), group("offline", 2)];
        let sync = Op::Sync { start: 0, items: vec![Some(Item::Group(group("online", 3))), Some(member(10, "ann"))] };
        assert_eq!(list.apply(&update(vec![sync], groups)), change(0, 1, 7));
        assert_eq!(names(&list), ["[online]", "ann", "_", "_", "[offline]", "_", "_"]);
        assert!(!list.is_unknown());
    }

    #[test]
    fn operations_on_rows() {
        let mut list = synced();
        assert_eq!(names(&list), ["[online]", "ann", "bob", "[offline]", "cy"]);
        let groups = vec![group("online", 3), group("offline", 1)];

        let insert = Op::Insert { index: 2, item: member(13, "dee") };
        assert_eq!(
            list.apply(&update(vec![insert], groups.clone())),
            change(0, 5, 6),
            "the header's count changed too"
        );
        assert_eq!(names(&list), ["[online]", "ann", "dee", "bob", "[offline]", "cy"]);

        let rename = Op::Update { index: 1, item: member(10, "ann2") };
        assert_eq!(list.apply(&update(vec![rename], groups.clone())), change(1, 1, 1));
        assert_eq!(names(&list)[1], "ann2");

        let delete = Op::Delete { index: 3 };
        let groups = vec![group("online", 2), group("offline", 1)];
        assert_eq!(list.apply(&update(vec![delete], groups.clone())), change(0, 6, 5));
        assert_eq!(names(&list), ["[online]", "ann2", "dee", "[offline]", "cy"]);
    }

    #[test]
    fn invalidating_stops_at_a_row_we_dont_hold() {
        let mut list = synced();
        let groups = vec![group("online", 2), group("offline", 1)];
        let op = Op::Invalidate { range: (1, 9) };
        assert_eq!(list.apply(&update(vec![op], groups)), change(1, 4, 4));
        // The header is drawn from the groups; the rows after it are not held.
        assert_eq!(names(&list), ["[online]", "_", "_", "[offline]", "_"]);
    }

    #[test]
    fn deleting_a_row_we_dont_hold_moves_nothing() {
        let mut list = synced();
        let groups = vec![group("online", 2), group("offline", 1)];
        let mut other = MemberList::default();
        other.apply(&update(vec![], groups.clone()));
        assert_eq!(other.apply(&update(vec![Op::Delete { index: 1 }], groups.clone())), None);
        assert_eq!(list.apply(&update(vec![Op::Delete { index: 99 }], groups)), None);
        assert_eq!(names(&list), ["[online]", "ann", "bob", "[offline]", "cy"]);
    }

    #[test]
    fn counts_move_the_headers() {
        let mut list = synced();
        // Someone went offline, and Discord says so by the groups alone.
        let groups = vec![group("online", 1), group("offline", 2)];
        assert_eq!(list.apply(&update(vec![], groups)), change(0, 5, 5));
        // Discord follows with the operations that move the rows.
        assert_eq!(names(&list), ["[online]", "ann", "[offline]", "_", "cy"]);
    }

    #[test]
    fn presence_changes_a_row() {
        let mut list = synced();
        let online = Presence { status: crate::relationships::Status::Online, text: None };
        assert_eq!(list.set_presence(Snowflake(11), &online), change(2, 1, 1));
        assert_eq!(list.set_presence(Snowflake(11), &online), None, "nothing new");
        assert_eq!(list.set_presence(Snowflake(99), &online), None, "not in the list");
    }

    #[test]
    fn the_events_shape() {
        let update = Update::from_value(&json!({
            "guild_id": "1", "id": "everyone", "member_count": 40, "online_count": 3,
            "groups": [{"id": "online", "count": 1}, {"id": "offline", "count": 1}],
            "ops": [
                {"op": "SYNC", "range": [0, 3], "items": [
                    {"group": {"id": "online", "count": 1}},
                    {"member": {"user": {"id": "10", "username": "ann"}, "roles": ["40"], "nick": "Annie",
                                "presence": {"status": "idle", "activities": []}}},
                    {"group": {"id": "offline", "count": 1}},
                    {"member": {"user": {"id": "12"}}},
                ]},
                {"op": "DELETE", "index": 7},
                {"op": "INVALIDATE", "range": [4, 9]},
                {"op": "SOMETHING NEW"},
            ],
        }))
        .unwrap();
        assert_eq!(
            (update.guild, update.list.as_str(), update.member_count, update.online_count),
            (G, "everyone", 40, 3)
        );
        assert_eq!(update.groups, [group("online", 1), group("offline", 1)]);
        assert_eq!(update.ops.len(), 3, "an operation we don't know is left out");
        let Op::Sync { start, items } = &update.ops[0] else { panic!("a sync first") };
        assert_eq!((*start, items.len(), items[3].is_none()), (0, 4, true), "a member without a username");
        let Some(Item::Member(ann)) = &items[1] else { panic!("a member") };
        assert_eq!(
            (ann.name(), &ann.roles[..], ann.presence.status),
            ("Annie", &[Snowflake(40)][..], crate::relationships::Status::Idle)
        );
        assert_eq!(update.members().len(), 1);
    }

    #[test]
    fn a_list_is_named_as_the_client_hashes() {
        // What the client's own function gives (181370), for a list's name.
        assert_eq!(murmur3("allow:456,deny:123"), 2797605632);
        assert_eq!(murmur3("deny:1"), 804936038);
        assert_eq!(murmur3("allow:40,deny:1"), 865732658);
    }

    fn guild(everyone: u64, channel_overwrites: Value) -> (Guild, Channel) {
        let guild: Guild = serde_json::from_value(json!({
            "id": "1", "name": "G", "roles": [{"id": "1", "permissions": everyone.to_string()}],
        }))
        .unwrap();
        let channel = serde_json::from_value(json!({
            "id": "9", "type": 0, "permission_overwrites": channel_overwrites,
        }))
        .unwrap();
        (guild, channel)
    }

    #[test]
    fn which_list_a_channel_shows() {
        let view = Permissions::VIEW_CHANNEL.0;
        let (g, c) = guild(view, json!([]));
        assert_eq!(list_id(&g, &c), "everyone");
        // An overwrite that denies nothing changes nothing.
        let (g, c) = guild(view, json!([{"id": "1", "type": 0, "allow": "0", "deny": "0"}]));
        assert_eq!(list_id(&g, &c), "everyone");

        let (g, mut c) = guild(view, json!([{"id": "1", "type": 0, "allow": "0", "deny": view.to_string()}]));
        assert_eq!(list_id(&g, &c), murmur3("deny:1").to_string());
        let (g2, c2) = guild(
            0,
            json!([{"id": "40", "type": 0, "allow": view.to_string(), "deny": "0"}, {"id": "1", "type": 0, "allow": "0", "deny": view.to_string()}]),
        );
        assert_eq!(list_id(&g2, &c2), murmur3("allow:40,deny:1").to_string());
        // What the channel says wins.
        c.member_list_id = Some("abc".into());
        assert_eq!(list_id(&g, &c), "abc");
    }

    #[test]
    fn a_group_dm_lists_everyone_by_username() {
        let user = |id: u64, name: &str| -> User {
            serde_json::from_value(json!({"id": id.to_string(), "username": name})).unwrap()
        };
        let (me, others) = (user(1, "Mia"), [user(2, "zed"), user(3, "amy")]);
        let names: Vec<&str> = group_members(&me, &others).iter().map(|u| u.username.as_str()).collect();
        assert_eq!(names, ["amy", "Mia", "zed"]);
    }
}
