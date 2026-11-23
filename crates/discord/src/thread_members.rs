//! The member lists of the threads we ask for, as the client's
//! `ThreadMemberListStore` (219065) keeps who is in them.
//!
//! A thread's list is asked for through op 37's `thread_member_lists`
//! ([`crate::subscriptions`]) while it shows, and Discord answers with
//! `THREAD_MEMBER_LIST_UPDATE`: everyone in the thread. After that it sends
//! `THREAD_MEMBERS_UPDATE` for who joins and leaves. A list we don't hold is
//! not kept up to date from those, and one that is let go of (the thread was
//! archived or deleted, or is no longer asked for) is dropped, so the
//! next time it shows it is asked for whole.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::Snowflake;
use crate::model::Role;

#[derive(Debug, Default)]
struct List {
    /// Everyone in the thread, by id.
    members: BTreeSet<Snowflake>,
    /// The channel the thread is in, whose permissions decide who can see it.
    parent: Option<Snowflake>,
    guild: Snowflake,
}

#[derive(Debug, Default)]
pub struct Lists {
    by_thread: HashMap<Snowflake, List>,
}

impl Lists {
    /// A new connection: nothing we held is known to be current.
    pub fn clear(&mut self) {
        self.by_thread.clear();
    }

    /// `thread`'s whole list came (THREAD_MEMBER_LIST_UPDATE). The client
    /// holds it only where it knows the thread's parent, as `parent` says.
    pub fn replace(
        &mut self,
        guild: Snowflake,
        thread: Snowflake,
        parent: Option<Snowflake>,
        members: impl IntoIterator<Item = Snowflake>,
    ) -> bool {
        let Some(parent) = parent else { return false };
        self.by_thread.insert(thread, List { members: members.into_iter().collect(), parent: Some(parent), guild });
        true
    }

    /// Who joined and left `thread` (THREAD_MEMBERS_UPDATE); of a list we
    /// hold only. Whether it changed.
    pub fn changed(&mut self, thread: Snowflake, added: &[Snowflake], removed: &[Snowflake]) -> bool {
        let Some(list) = self.by_thread.get_mut(&thread) else { return false };
        let mut changed = false;
        for id in added {
            changed |= list.members.insert(*id);
        }
        for id in removed {
            changed |= list.members.remove(id);
        }
        changed
    }

    /// The thread was archived (THREAD_UPDATE) or deleted: its list is let
    /// go of. Whether we held one.
    pub fn forget(&mut self, thread: Snowflake) -> bool {
        self.by_thread.remove(&thread).is_some()
    }

    /// Keep only the lists of `asked` threads, which [`crate::subscriptions`]
    /// says we ask for. Whether any went.
    pub fn keep_asked(&mut self, asked: &HashSet<Snowflake>) -> bool {
        let before = self.by_thread.len();
        self.by_thread.retain(|thread, _| asked.contains(thread));
        self.by_thread.len() != before
    }

    /// Everyone in `thread`, if we hold its list.
    pub fn members(&self, thread: Snowflake) -> Option<&BTreeSet<Snowflake>> {
        self.by_thread.get(&thread).map(|l| &l.members)
    }

    /// Whether `user` is in `thread`; none where we don't hold its list.
    pub fn is_member(&self, thread: Snowflake, user: Snowflake) -> Option<bool> {
        self.members(thread).map(|m| m.contains(&user))
    }

    /// The channel `thread` is in, as its list was made.
    pub fn parent(&self, thread: Snowflake) -> Option<Snowflake> {
        self.by_thread.get(&thread)?.parent
    }

    /// The threads of `guild` whose lists we hold.
    pub fn threads_of(&self, guild: Snowflake) -> Vec<Snowflake> {
        let mut threads: Vec<Snowflake> =
            self.by_thread.iter().filter(|(_, l)| l.guild == guild).map(|(t, _)| *t).collect();
        threads.sort();
        threads
    }
}

/// What a member list of a thread groups people under: a role that lists
/// its members apart, or `online` or `offline`.
pub const ONLINE: &str = "online";
pub const OFFLINE: &str = "offline";

/// What the list needs to know of each person (`calculateNewState`).
pub struct Person {
    pub id: Snowflake,
    /// On, whatever the way (anything but offline and invisible).
    pub on: bool,
    /// The roles they have.
    pub roles: Vec<Snowflake>,
    /// Their nickname, else their name.
    pub name: String,
}

/// A group of the list.
#[derive(Debug, PartialEq, Eq)]
pub struct Section {
    /// A role's id, `online` or `offline`.
    pub id: String,
    pub title: String,
    pub people: Vec<Snowflake>,
}

/// The role that lists a member apart: their highest of those that do
/// (`hoistRoleId`), by position.
pub fn hoist_role(roles: &[Role], held: &[Snowflake]) -> Option<Snowflake> {
    roles
        .iter()
        .filter(|r| r.hoist && held.contains(&r.id))
        .max_by_key(|r| (r.position, std::cmp::Reverse(r.id)))
        .map(|r| r.id)
}

/// The sections of a thread's list as the pane shows them
/// (`ThreadMemberListStore` and its list in 903137): each role that lists its
/// members apart, from the highest, then online, then offline; people by name,
/// ignoring case, then by id, and only the sections that have anyone.
pub fn sections(roles: &[Role], people: Vec<Person>) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut ranked: Vec<&Role> = roles.iter().filter(|r| r.hoist).collect();
    ranked.sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
    let mut titles: Vec<(String, String)> = ranked.iter().map(|r| (r.id.0.to_string(), r.name.clone())).collect();
    titles.push((ONLINE.to_owned(), "Online".to_owned()));
    titles.push((OFFLINE.to_owned(), "Offline".to_owned()));
    let mut by_name: Vec<Person> = people;
    // The name as JavaScript orders strings: by UTF-16 units.
    let key = |p: &Person| (p.name.to_lowercase().encode_utf16().collect::<Vec<u16>>(), p.id);
    by_name.sort_by_key(key);
    for (id, title) in titles {
        let members: Vec<Snowflake> = by_name
            .iter()
            .filter(|p| {
                let section = match p.on {
                    true => hoist_role(roles, &p.roles).map_or(ONLINE.to_owned(), |r| r.0.to_string()),
                    false => OFFLINE.to_owned(),
                };
                section == id
            })
            .map(|p| p.id)
            .collect();
        if !members.is_empty() {
            sections.push(Section { id, title, people: members });
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Snowflake = Snowflake(20);
    const PARENT: Snowflake = Snowflake(5);
    const GUILD: Snowflake = Snowflake(1);

    fn ids(v: &[u64]) -> Vec<Snowflake> {
        v.iter().copied().map(Snowflake).collect()
    }

    fn role(id: u64, name: &str, position: i32, hoist: bool) -> Role {
        serde_json::from_value(
            serde_json::json!({"id": id.to_string(), "name": name, "position": position, "hoist": hoist}),
        )
        .unwrap()
    }

    fn person(id: u64, name: &str, on: bool, roles: &[u64]) -> Person {
        Person { id: Snowflake(id), on, roles: roles.iter().copied().map(Snowflake).collect(), name: name.to_owned() }
    }

    #[test]
    fn people_are_listed_under_their_highest_listed_role_then_online_then_offline() {
        let roles = [
            role(1, "everyone", 0, false),
            role(10, "Mods", 5, true),
            role(11, "Helpers", 3, true),
            role(12, "Fun", 9, false),
        ];
        let sections = sections(
            &roles,
            vec![
                person(7, "zed", true, &[11, 10]),
                person(6, "Bea", true, &[12]),
                person(5, "amy", true, &[11]),
                person(8, "al", false, &[10]),
                person(9, "Al", true, &[]),
                person(4, "bea", true, &[]),
            ],
        );
        let shown: Vec<(&str, &str, Vec<u64>)> = sections
            .iter()
            .map(|s| (s.id.as_str(), s.title.as_str(), s.people.iter().map(|p| p.0).collect()))
            .collect();
        assert_eq!(
            shown,
            [
                ("10", "Mods", vec![7]),
                ("11", "Helpers", vec![5]),
                // Names without regard to case, then ids; the offline one is offline whatever its role.
                ("online", "Online", vec![9, 4, 6]),
                ("offline", "Offline", vec![8]),
            ]
        );
        assert!(super::sections(&roles, Vec::new()).is_empty(), "no one, no section");
    }

    #[test]
    fn a_role_that_lists_apart_but_is_not_held_does_not_count() {
        let roles = [role(10, "Mods", 5, true), role(11, "Helpers", 3, true)];
        assert_eq!(hoist_role(&roles, &[Snowflake(11)]), Some(Snowflake(11)));
        assert_eq!(hoist_role(&roles, &[Snowflake(11), Snowflake(10)]), Some(Snowflake(10)), "the higher one");
        assert_eq!(hoist_role(&roles, &[Snowflake(99)]), None);
    }

    #[test]
    fn a_list_comes_whole_and_joins_and_leaves_change_it() {
        let mut lists = Lists::default();
        assert_eq!(lists.is_member(T, Snowflake(6)), None, "none held");
        assert!(!lists.changed(T, &ids(&[6]), &[]), "a list we don't hold isn't kept up");
        assert!(lists.replace(GUILD, T, Some(PARENT), ids(&[6, 7])));
        assert_eq!(lists.is_member(T, Snowflake(6)), Some(true));
        assert_eq!(lists.is_member(T, Snowflake(8)), Some(false));
        assert_eq!(lists.parent(T), Some(PARENT));
        assert!(lists.changed(T, &ids(&[8]), &ids(&[6])));
        assert_eq!(lists.members(T).unwrap().iter().copied().collect::<Vec<_>>(), ids(&[7, 8]));
        assert!(!lists.changed(T, &ids(&[8]), &ids(&[9])), "already in, and never was");
    }

    #[test]
    fn a_thread_whose_parent_we_dont_know_has_no_list() {
        let mut lists = Lists::default();
        assert!(!lists.replace(GUILD, T, None, ids(&[6])));
        assert!(lists.members(T).is_none());
    }

    #[test]
    fn lists_are_let_go_when_the_thread_goes_or_isnt_asked_for() {
        let mut lists = Lists::default();
        lists.replace(GUILD, T, Some(PARENT), ids(&[6]));
        lists.replace(GUILD, Snowflake(21), Some(PARENT), ids(&[7]));
        lists.replace(Snowflake(2), Snowflake(30), Some(Snowflake(9)), ids(&[8]));
        assert_eq!(lists.threads_of(GUILD), [T, Snowflake(21)]);
        assert!(lists.forget(T));
        assert!(!lists.forget(T));
        assert!(lists.keep_asked(&HashSet::from([Snowflake(30)])));
        assert!(lists.members(Snowflake(21)).is_none() && lists.members(Snowflake(30)).is_some());
        assert!(!lists.keep_asked(&HashSet::from([Snowflake(30)])), "nothing more to drop");
        lists.clear();
        assert!(lists.members(Snowflake(30)).is_none());
    }
}
