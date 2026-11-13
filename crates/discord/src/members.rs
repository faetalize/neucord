//! Other people's roles in each server, which give their names a colour, and
//! asking the gateway for the ones we lack.
//!
//! A live message carries its author's member; a page of history doesn't.
//! After a page loads, the official client asks for the members of its
//! authors and of whom they mention, unless it knows them or asked already
//! (`GuildMemberRequesterStore`). [`Members`] keeps that same account, so
//! Neucord asks for exactly what the client would.

use std::collections::{HashMap, HashSet};

use crate::{Guild, Message, Role, Snowflake};

/// How many users one request may name.
pub const REQUEST_LIMIT: usize = 100;

/// A colour a name or a role mention is drawn in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleColour {
    Solid(u32),
    /// Two or three colours, `0xRRGGBB`, from start to end.
    Gradient(Vec<u32>),
}

impl RoleColour {
    /// The colour at `t`, from 0 (the start) to 1 (the end).
    pub fn at(&self, t: f64) -> u32 {
        let stops = match self {
            RoleColour::Solid(colour) => return *colour,
            RoleColour::Gradient(stops) => stops,
        };
        let scaled = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
        let i = (scaled.floor() as usize).min(stops.len() - 2);
        let (from, to, t) = (stops[i], stops[i + 1], scaled - i as f64);
        let channel = |shift: u32| {
            let (a, b) = (f64::from((from >> shift) & 0xff), f64::from((to >> shift) & 0xff));
            ((a + (b - a) * t).round() as u32) << shift
        };
        channel(16) | channel(8) | channel(0)
    }
}

impl Guild {
    /// What `role` is drawn in: nothing without a colour, a gradient where
    /// the server has them and the role sets a second colour.
    pub fn role_colour(&self, role: &Role) -> Option<RoleColour> {
        if role.color == 0 {
            return None;
        }
        let gradient = role.colors.as_ref().filter(|_| self.gradient_roles).and_then(|colors| {
            let secondary = colors.secondary_color?;
            let primary = if colors.primary_color == 0 { role.color } else { colors.primary_color };
            Some(RoleColour::Gradient(
                [Some(primary), Some(secondary), colors.tertiary_color].into_iter().flatten().collect(),
            ))
        });
        Some(gradient.unwrap_or(RoleColour::Solid(role.color)))
    }

    /// A member's colour: that of their highest role with one.
    pub fn member_colour(&self, roles: &[Snowflake]) -> Option<RoleColour> {
        let highest = self.highest_role(roles.iter().filter_map(|&id| self.role(id)).filter(|r| r.color > 0))?;
        self.role_colour(highest)
    }
}

/// What a server says of one of its members that shows in the chat: their
/// roles colour their name, and their nickname and picture replace their own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub roles: Vec<Snowflake>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
}

impl Known {
    /// From a member object, with or without its user.
    pub fn from_value(value: &serde_json::Value) -> Known {
        let text = |v: &serde_json::Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        Known {
            roles: crate::model::Member::from_value(value).roles,
            nick: text(&value["nick"]),
            avatar: text(&value["avatar"]),
        }
    }
}

/// What we know of other members, and what we asked the gateway for.
#[derive(Debug, Default)]
pub struct Members {
    known: HashMap<(Snowflake, Snowflake), Known>,
    requests: HashMap<Snowflake, Requests>,
}

#[derive(Debug, Default)]
struct Requests {
    /// To ask for next time we flush.
    pending: HashSet<Snowflake>,
    /// Asked for since we connected, answered or not.
    sent: HashSet<Snowflake>,
    /// Asked for and not answered yet.
    unanswered: HashSet<Snowflake>,
}

impl Members {
    /// `user`'s roles in `guild`, if we know their member.
    pub fn roles(&self, guild: Snowflake, user: Snowflake) -> Option<&[Snowflake]> {
        self.known.get(&(guild, user)).map(|k| k.roles.as_slice())
    }

    /// What `user` is called in `guild`, if they set a nickname there.
    pub fn nick(&self, guild: Snowflake, user: Snowflake) -> Option<&str> {
        self.known.get(&(guild, user))?.nick.as_deref()
    }

    /// Their picture in `guild`, if they set one there.
    pub fn avatar(&self, guild: Snowflake, user: Snowflake) -> Option<&str> {
        self.known.get(&(guild, user))?.avatar.as_deref()
    }

    /// Everyone's nickname in `guild`.
    pub fn nicks(&self, guild: Snowflake) -> impl Iterator<Item = (Snowflake, &str)> {
        self.known
            .iter()
            .filter(move |((g, _), _)| *g == guild)
            .filter_map(|((_, u), k)| Some((*u, k.nick.as_deref()?)))
    }

    /// Members Discord sent, asked for or not, and the users it says aren't
    /// members: both answer a request. Whether any member's roles are new
    /// to us, or what shows of them: how a name's colour or text can change.
    pub fn add(&mut self, guild: Snowflake, members: Vec<(Snowflake, Known)>, not_found: &[Snowflake]) -> bool {
        let requests = self.requests.entry(guild).or_default();
        for user in members.iter().map(|(user, _)| user).chain(not_found) {
            requests.unanswered.remove(user);
            requests.pending.remove(user);
        }
        let mut changed = false;
        for (user, known) in members {
            changed |= self.known.insert((guild, user), known.clone()).as_ref() != Some(&known);
        }
        changed
    }

    /// A page of `guild`'s messages loaded: ask for the authors and the
    /// people they mention whose member we don't know.
    pub fn page_loaded(&mut self, guild: Snowflake, messages: &[Message]) {
        for message in messages {
            for user in std::iter::once(&message.author).chain(&message.mentions) {
                self.request(guild, user.id);
            }
        }
    }

    /// Ask for `user`'s member in `guild`, unless we know it or asked
    /// (`requestMember`).
    pub fn request(&mut self, guild: Snowflake, user: Snowflake) {
        if self.known.contains_key(&(guild, user)) {
            return;
        }
        let requests = self.requests.entry(guild).or_default();
        if !requests.sent.contains(&user) {
            requests.pending.insert(user);
        }
    }

    /// The requests to send now: for each server, at most
    /// [`REQUEST_LIMIT`] users at a time.
    pub fn flush(&mut self) -> Vec<(Snowflake, Vec<Snowflake>)> {
        let mut out = Vec::new();
        for (&guild, requests) in &mut self.requests {
            let mut users: Vec<Snowflake> =
                requests.pending.drain().filter(|&user| !self.known.contains_key(&(guild, user))).collect();
            users.sort();
            requests.sent.extend(&users);
            requests.unanswered.extend(&users);
            out.extend(users.chunks(REQUEST_LIMIT).map(|chunk| (guild, chunk.to_vec())));
        }
        out.sort();
        out
    }

    /// A resumed session: ask again for what went unanswered, at the next
    /// [`Members::flush`].
    pub fn resumed(&mut self) {
        for (&guild, requests) in &mut self.requests {
            requests.unanswered.retain(|&user| !self.known.contains_key(&(guild, user)));
            requests.pending.extend(&requests.unanswered);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const G: Snowflake = Snowflake(1);

    fn guild(gradients: bool) -> Guild {
        let features = if gradients { json!(["ENHANCED_ROLE_COLORS"]) } else { json!([]) };
        serde_json::from_value(json!({
            "id": "1", "name": "G", "features": features,
            "roles": [
                {"id": "1", "color": 0x111111, "position": 0},
                {"id": "40", "color": 0xff0000, "position": 2},
                {"id": "41", "color": 0x00ff00, "position": 2},
                {"id": "42", "color": 0, "position": 9},
                {"id": "43", "color": 0x0000ff, "position": 1,
                 "colors": {"primary_color": 0x0000ff, "secondary_color": 0xffffff, "tertiary_color": null}},
            ],
        }))
        .unwrap()
    }

    fn message(author: u64, mentions: &[u64]) -> Message {
        let mentions: Vec<_> = mentions.iter().map(|id| json!({"id": id.to_string(), "username": "u"})).collect();
        serde_json::from_value(json!({
            "id": "100", "channel_id": "9", "timestamp": "t", "mentions": mentions,
            "author": {"id": author.to_string(), "username": "a"},
        }))
        .unwrap()
    }

    #[test]
    fn highest_coloured_role() {
        let g = guild(false);
        let id = Snowflake;
        assert_eq!(g.member_colour(&[id(43), id(40)]), Some(RoleColour::Solid(0xff0000)));
        assert_eq!(g.member_colour(&[id(41), id(40)]), Some(RoleColour::Solid(0xff0000)), "a tie: the older role");
        assert_eq!(g.member_colour(&[id(42), id(43)]), Some(RoleColour::Solid(0x0000ff)), "a role without colour");
        assert_eq!(g.member_colour(&[id(1)]), Some(RoleColour::Solid(0x111111)), "@everyone, last");
        assert_eq!(g.member_colour(&[id(42), id(99)]), None);
    }

    #[test]
    fn gradients_where_the_server_has_them() {
        let role = |g: &Guild| g.role(Snowflake(43)).cloned().unwrap();
        assert_eq!(guild(false).role_colour(&role(&guild(false))), Some(RoleColour::Solid(0x0000ff)));
        let gradient = guild(true).role_colour(&role(&guild(true))).unwrap();
        assert_eq!(gradient, RoleColour::Gradient(vec![0x0000ff, 0xffffff]));
        assert_eq!((gradient.at(0.0), gradient.at(0.5), gradient.at(1.0)), (0x0000ff, 0x8080ff, 0xffffff));
        let three = RoleColour::Gradient(vec![0x000000, 0xff0000, 0x000000]);
        assert_eq!((three.at(0.5), three.at(0.75)), (0xff0000, 0x800000));
    }

    #[test]
    fn asks_once_for_what_it_lacks() {
        let mut members = Members::default();
        members.add(G, vec![(Snowflake(5), Known::default())], &[]);
        members.page_loaded(G, &[message(5, &[6]), message(7, &[6])]);
        assert_eq!(members.flush(), [(G, vec![Snowflake(6), Snowflake(7)])], "not who we know, nor twice");
        members.page_loaded(G, &[message(7, &[])]);
        assert!(members.flush().is_empty(), "asked already");

        let roles = || Known { roles: vec![Snowflake(40)], ..Known::default() };
        assert!(members.add(G, vec![(Snowflake(6), roles())], &[]), "new roles");
        assert!(!members.add(G, vec![(Snowflake(6), roles())], &[]), "the same");
        assert_eq!(members.roles(G, Snowflake(6)), Some(&[Snowflake(40)][..]));
        members.resumed();
        assert_eq!(members.flush(), [(G, vec![Snowflake(7)])], "unanswered, asked again after a resume");
        members.add(G, vec![], &[Snowflake(7)]);
        members.resumed();
        assert!(members.flush().is_empty(), "not a member is an answer too");
    }

    #[test]
    fn nicknames_and_pictures_are_per_server() {
        let mut members = Members::default();
        let known = Known::from_value(&json!({"roles": ["40"], "nick": "Nick", "avatar": "av"}));
        assert!(members.add(G, vec![(Snowflake(5), known)], &[]));
        assert_eq!((members.nick(G, Snowflake(5)), members.avatar(G, Snowflake(5))), (Some("Nick"), Some("av")));
        assert_eq!(members.nick(Snowflake(2), Snowflake(5)), None, "another server");
        assert_eq!(members.nicks(G).collect::<Vec<_>>(), [(Snowflake(5), "Nick")]);
        // A nickname changing is a change, and an emptied one is none.
        let emptied = Known::from_value(&json!({"roles": ["40"], "nick": "", "avatar": null}));
        assert!(members.add(G, vec![(Snowflake(5), emptied)], &[]));
        assert_eq!(members.nick(G, Snowflake(5)), None);
    }

    #[test]
    fn a_hundred_at_a_time() {
        let mut members = Members::default();
        let authors: Vec<Message> = (10..260).map(|id| message(id, &[])).collect();
        members.page_loaded(G, &authors);
        let sizes: Vec<usize> = members.flush().iter().map(|(_, users)| users.len()).collect();
        assert_eq!(sizes, [100, 100, 50]);
    }
}
