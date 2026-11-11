//! A user's profile: what `GET /users/{id}/profile` answers, when the
//! official client asks for it, and what it keeps.
//!
//! The client asks as a popout opens (`365958`, through `342296`) and holds
//! the answer in `UserProfileStore` (851767), where `917445` decides whether
//! a profile needs asking for again. Those rules are copied here and need
//! nothing of the window.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::experiments::Experiments;
use crate::model::{Channel, ChannelKind, Overwrite, Role};
use crate::permissions::Permissions;
use crate::{Snowflake, User};

/// How long an answer stays current: the client asks again once a profile it
/// holds is this old (`fetchEndedAt` in 917445).
const FRESH_MS: u64 = 60_000;

/// Where a profile is shown, which the request says (`type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Popout,
    /// The panel beside a DM.
    Sidebar,
}

/// The experiment that redesigns the panel beside a DM, and with it what
/// the panel asks for: the variant that enables it is 1 (`rD` in 289287).
const SIDEBAR_REDESIGN: &str = "2026-06-user-profile-sidebar-redesign";

/// Whether Discord gave the account the redesigned panel.
pub fn sidebar_redesign(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, SIDEBAR_REDESIGN).is_some_and(|a| a.variant == 1)
}

/// A profile asked for, as the client puts it: the query and what goes in it
/// (`342296` for a popout, `ok` in 289287 for the panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub surface: Surface,
    pub user: Snowflake,
    /// The server the popout was opened in; none in DMs.
    pub guild: Option<Snowflake>,
    pub mutual_guilds: bool,
    pub mutual_friends: bool,
    /// Only how many friends we share, not who: what the panel asks for
    /// unless the redesign asks for them.
    pub mutual_friend_count: bool,
    /// The role of theirs that links an account of another service, which
    /// the server's linked roles are told apart by (see [`connections_role`]).
    pub connections_role: Option<Snowflake>,
}

impl Request {
    /// For a popout on `user`: others' mutual servers are asked for, and
    /// their mutual friends but not a bot's.
    pub fn popout(user: &User, me: Snowflake, guild: Option<Snowflake>, connections_role: Option<Snowflake>) -> Self {
        let other = user.id != me;
        Request {
            surface: Surface::Popout,
            user: user.id,
            guild,
            mutual_guilds: other,
            mutual_friends: other && !user.bot,
            mutual_friend_count: false,
            connections_role,
        }
    }

    /// For the panel beside a DM with `user`: their mutual servers, and for
    /// anyone but a bot their mutual friends, or just how many unless the
    /// account has the redesign (`redesign`).
    pub fn sidebar(user: &User, redesign: bool) -> Self {
        Request {
            surface: Surface::Sidebar,
            user: user.id,
            guild: None,
            mutual_guilds: true,
            mutual_friends: !user.bot && redesign,
            mutual_friend_count: !user.bot && !redesign,
            connections_role: None,
        }
    }

    /// The query, in the order the client's fields come: `undefined` ones
    /// are left out, `false` ones are sent.
    pub fn query(&self) -> String {
        let kind = match self.surface {
            Surface::Popout => "popout",
            Surface::Sidebar => "sidebar",
        };
        let mut query = format!(
            "type={kind}&with_mutual_guilds={}&with_mutual_friends={}&with_mutual_friends_count={}",
            self.mutual_guilds, self.mutual_friends, self.mutual_friend_count
        );
        if let Some(guild) = self.guild {
            query.push_str(&format!("&guild_id={guild}"));
        }
        if let Some(role) = self.connections_role {
            query.push_str(&format!("&connections_role_id={role}"));
        }
        query
    }
}

/// A role of `member` that links an account of another service, for the
/// request's `connections_role_id` (`_g` in 370480): of the roles the server
/// has for that, the highest of theirs that the channel's overwrites let
/// talk, else the highest of theirs.
pub fn connections_role(
    roles: &[Role],
    guild: Snowflake,
    member: &[Snowflake],
    channel: Option<&Channel>,
) -> Option<Snowflake> {
    let mut theirs: Vec<&Role> = roles.iter().filter(|r| r.linked && member.contains(&r.id)).collect();
    // `@everyone` last, then the highest position, then the oldest.
    theirs.sort_by_key(|r| (r.id == guild, std::cmp::Reverse(r.position), r.id));
    let overwrites: &[Overwrite] =
        channel.filter(|c| lets_roles_talk(c.kind)).map_or(&[], |c| &c.permission_overwrites);
    let talk = Permissions::VIEW_CHANNEL.0 | Permissions::SEND_MESSAGES.0;
    let allowed = |role: &Role| overwrites.iter().any(|o| o.id == role.id && o.deny.0 & talk == 0);
    theirs.iter().find(|r| allowed(r)).or(theirs.first()).map(|r| r.id)
}

/// A member's roles as the profile lists them (1659): linked ones last,
/// otherwise the highest first.
pub fn listed_roles<'a>(roles: &'a [Role], guild: Snowflake, member: &[Snowflake]) -> Vec<&'a Role> {
    let mut theirs: Vec<&Role> = roles.iter().filter(|r| member.contains(&r.id)).collect();
    theirs.sort_by_key(|r| (r.linked, r.id == guild, std::cmp::Reverse(r.position), r.id));
    theirs
}

/// The channels whose overwrites narrow the roles above (920268).
fn lets_roles_talk(kind: ChannelKind) -> bool {
    use ChannelKind::*;
    matches!(kind, Text | Voice | Announcement | Forum | PublicThread | PrivateThread)
}

/// A badge: its picture is `badge-icons/{icon}.png` on the CDN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    pub id: String,
    pub description: String,
    pub icon: String,
    pub link: Option<String>,
}

/// What a server says of a member in a profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub nick: Option<String>,
    pub roles: Vec<Snowflake>,
    /// Their picture in this server.
    pub avatar: Option<String>,
    /// When they joined it, in seconds since 1970.
    pub joined_at: Option<i64>,
}

/// An avatar decoration: the image drawn around a picture
/// (`avatar_decoration_data`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoration {
    /// Its product in the collectibles shop, which has it as an image.
    pub sku: Option<String>,
    /// Its name among the older presets, which have it too.
    pub asset: Option<String>,
    /// When it lapses, in seconds since 1970.
    pub expires_at: Option<i64>,
}

impl Decoration {
    fn from_value(value: &Value) -> Option<Decoration> {
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let (sku, asset) = (text(&value["sku_id"]), text(&value["asset"]));
        (sku.is_some() || asset.is_some()).then(|| Decoration {
            sku,
            asset,
            expires_at: crate::model::unix_rounded_up(&value["expires_at"]),
        })
    }

    /// Where its still image is, at `now` (seconds since 1970): the shop's,
    /// else the preset's, and none once it has lapsed (`D` in 486020).
    pub fn url(&self, now: i64, size: u16) -> Option<String> {
        if self.expires_at.is_some_and(|at| at <= now) {
            return None;
        }
        match (&self.sku, &self.asset) {
            (Some(sku), _) => Some(crate::cdn::decoration_url(sku)),
            (None, Some(asset)) => Some(crate::cdn::decoration_preset_url(asset, size)),
            (None, None) => None,
        }
    }
}

/// A server we share with them, and what they are called there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutualGuild {
    pub id: Snowflake,
    pub nick: Option<String>,
}

/// What they set for one server: it covers their own where it says something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerProfile {
    pub bio: Option<String>,
    pub pronouns: Option<String>,
    pub banner: Option<String>,
}

/// The answer, as far as Neucord reads it.
#[derive(Clone, Debug)]
pub struct Profile {
    pub user: User,
    pub badges: Vec<Badge>,
    pub decoration: Option<Decoration>,
    pub bio: String,
    pub pronouns: String,
    pub banner: Option<String>,
    /// For the server asked about, if they are in it.
    pub member: Option<Member>,
    pub server: Option<(Snowflake, ServerProfile)>,
    /// The badges the server adds (`guild_badges`).
    pub server_badges: Vec<Badge>,
    pub mutual_guilds: Option<Vec<MutualGuild>>,
    pub mutual_friends: Option<Vec<User>>,
    pub mutual_friends_count: Option<u32>,
}

impl Profile {
    pub fn from_value(value: &Value) -> Option<Profile> {
        let user = User::deserialize(&value["user"]).ok()?;
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let badges = |v: &Value| -> Vec<Badge> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    Some(Badge {
                        id: b["id"].as_str()?.to_owned(),
                        description: b["description"].as_str().unwrap_or_default().to_owned(),
                        icon: b["icon"].as_str()?.to_owned(),
                        link: text(&b["link"]),
                    })
                })
                .collect()
        };
        let id = |v: &Value| v.as_str().and_then(|s| s.parse().ok()).map(Snowflake);
        let member = value["guild_member"].is_object().then(|| {
            let m = &value["guild_member"];
            Member {
                nick: text(&m["nick"]),
                roles: m["roles"].as_array().into_iter().flatten().filter_map(id).collect(),
                avatar: text(&m["avatar"]),
                joined_at: crate::model::unix_rounded_up(&m["joined_at"]),
            }
        });
        let server = id(&value["guild_member_profile"]["guild_id"]).map(|guild| {
            let p = &value["guild_member_profile"];
            (guild, ServerProfile { bio: text(&p["bio"]), pronouns: text(&p["pronouns"]), banner: text(&p["banner"]) })
        });
        let mutual_guilds = value["mutual_guilds"].as_array().map(|list| {
            list.iter().filter_map(|g| Some(MutualGuild { id: id(&g["id"])?, nick: text(&g["nick"]) })).collect()
        });
        let mutual_friends = value["mutual_friends"]
            .as_array()
            .map(|list| list.iter().filter_map(|u| User::deserialize(u).ok()).collect::<Vec<_>>());
        // A list says more than a count does, as in the client's store.
        let mutual_friends_count = match &mutual_friends {
            Some(list) => Some(list.len() as u32),
            None => value["mutual_friends_count"].as_u64().map(|n| n as u32),
        };
        Some(Profile {
            user,
            badges: badges(&value["badges"]),
            decoration: Decoration::from_value(&value["user"]["avatar_decoration_data"]),
            bio: value["user_profile"]["bio"].as_str().unwrap_or_default().to_owned(),
            pronouns: value["user_profile"]["pronouns"].as_str().unwrap_or_default().to_owned(),
            banner: text(&value["user_profile"]["banner"]),
            member,
            server,
            server_badges: badges(&value["guild_badges"]),
            mutual_guilds,
            mutual_friends,
            mutual_friends_count,
        })
    }

    /// What the popout shows: the server's bio, pronouns and banner where
    /// they set one for it, else their own (`527212`).
    pub fn shown(&self) -> Shown<'_> {
        let server = self.server.as_ref().map(|(guild, profile)| (*guild, profile));
        fn pick<'a>(own: &'a str, theirs: Option<&'a String>) -> &'a str {
            match theirs {
                Some(text) if !text.is_empty() => text,
                _ => own,
            }
        }
        Shown {
            bio: pick(&self.bio, server.and_then(|(_, p)| p.bio.as_ref())),
            pronouns: pick(&self.pronouns, server.and_then(|(_, p)| p.pronouns.as_ref())),
            banner: match server {
                Some((guild, ServerProfile { banner: Some(hash), .. })) => Some(Banner::Server { guild, hash }),
                _ => self.banner.as_deref().map(Banner::User),
            },
        }
    }
}

pub struct Shown<'a> {
    pub bio: &'a str,
    pub pronouns: &'a str,
    pub banner: Option<Banner<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner<'a> {
    User(&'a str),
    Server { guild: Snowflake, hash: &'a str },
}

/// What a request for a profile came to, when it did.
#[derive(Clone, Debug)]
struct Held<T> {
    /// The answer: none for a profile that was asked for and refused.
    data: Option<T>,
    /// When the request ended, in milliseconds since 1970.
    ended: u64,
    /// The HTTP status it failed with.
    failed: Option<u32>,
}

impl<T> Held<T> {
    fn stale(&self, now: u64) -> bool {
        now.saturating_sub(self.ended) >= FRESH_MS
    }
}

/// What a popout does about a profile (`917445`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Nothing to ask: it is being asked for, current, or was refused lately.
    No,
    /// Ask, and show what is held meanwhile.
    Behind,
    /// Ask, and show nothing until it is answered: nothing is held.
    Before,
}

/// Profiles by user, and by user and server, with the mutual servers and
/// friends they came with.
#[derive(Debug, Default)]
pub struct Profiles {
    users: HashMap<Snowflake, Held<Profile>>,
    servers: HashMap<(Snowflake, Snowflake), Held<ServerProfile>>,
    /// What a server says of a member, which only its own popouts show.
    members: HashMap<(Snowflake, Snowflake), (Option<Member>, Vec<Badge>)>,
    asking: HashSet<(Snowflake, Option<Snowflake>)>,
    mutual_guilds: HashMap<Snowflake, Vec<MutualGuild>>,
    mutual_friends: HashMap<Snowflake, Vec<User>>,
    mutual_friend_counts: HashMap<Snowflake, u32>,
    /// Whose mutual friends are on their way, with a profile or alone.
    asking_friends: HashSet<Snowflake>,
}

impl Profiles {
    /// Whether `request` is to be sent at `now` (ms since 1970). A server's
    /// profile exists only if Discord answered with one, so a server that
    /// never does is asked again every time.
    pub fn plan(&self, request: &Request, now: u64) -> Fetch {
        if self.asking.contains(&(request.user, request.guild)) {
            return Fetch::No;
        }
        let kept = match request.guild {
            Some(guild) => self.servers.get(&(request.user, guild)).map(|h| (h.stale(now), h.failed)),
            None => self.users.get(&request.user).map(|h| (h.stale(now), h.failed)),
        };
        let Some((stale, failed)) = kept else { return Fetch::Before };
        if matches!(failed, Some(404 | 429)) && !stale {
            return Fetch::No;
        }
        let missing = |wanted: bool, held: bool| wanted && !held;
        let user = request.user;
        let again = stale
            || missing(request.mutual_guilds, self.mutual_guilds.contains_key(&user))
            || missing(request.mutual_friends, self.mutual_friends.contains_key(&user))
            || missing(request.mutual_friend_count, self.mutual_friend_counts.contains_key(&user));
        if again { Fetch::Behind } else { Fetch::No }
    }

    /// `request` is on its way.
    pub fn asked(&mut self, request: &Request) {
        self.asking.insert((request.user, request.guild));
        if request.mutual_friends {
            self.asking_friends.insert(request.user);
        }
    }

    /// Discord answered.
    pub fn answered(&mut self, request: &Request, profile: Profile, now: u64) {
        let user = profile.user.id;
        self.asking.remove(&(request.user, request.guild));
        self.asking_friends.remove(&request.user);
        if let Some(guilds) = &profile.mutual_guilds {
            self.mutual_guilds.insert(user, guilds.clone());
        }
        if let Some(count) = profile.mutual_friends_count {
            self.mutual_friend_counts.insert(user, count);
            if count == 0 {
                self.mutual_friends.insert(user, Vec::new());
            }
        }
        if let Some(friends) = &profile.mutual_friends {
            let mut friends = friends.clone();
            friends.sort_by_key(|f| f.username.to_lowercase());
            self.mutual_friends.insert(user, friends);
        }
        if let Some(guild) = request.guild {
            self.members.insert((user, guild), (profile.member.clone(), profile.server_badges.clone()));
        }
        if let Some((guild, server)) = &profile.server {
            self.servers.insert((user, *guild), Held { data: Some(server.clone()), ended: now, failed: None });
        }
        self.users.insert(user, Held { data: Some(profile), ended: now, failed: None });
    }

    /// Discord refused, or couldn't be asked. A 404 means no such user, so
    /// they share nothing with us.
    pub fn refused(&mut self, request: &Request, status: Option<u32>, now: u64) {
        let user = request.user;
        self.asking.remove(&(user, request.guild));
        self.asking_friends.remove(&user);
        let held = self.users.entry(user).or_insert(Held { data: None, ended: 0, failed: None });
        (held.ended, held.failed) = (now, Some(status.unwrap_or(0)));
        if let Some(server) = request.guild.and_then(|g| self.servers.get_mut(&(user, g))) {
            (server.ended, server.failed) = (now, Some(status.unwrap_or(0)));
        }
        if status == Some(404) {
            self.mutual_friend_counts.insert(user, 0);
            self.mutual_friends.insert(user, Vec::new());
            self.mutual_guilds.insert(user, Vec::new());
        }
    }

    /// What we hold of `user`.
    pub fn get(&self, user: Snowflake) -> Option<&Profile> {
        self.users.get(&user)?.data.as_ref()
    }

    /// What we hold of `user`, as shown in `guild` (none: outside any
    /// server): what a server says of them is there only for its own.
    pub fn get_in(&self, user: Snowflake, guild: Option<Snowflake>) -> Option<Profile> {
        let mut profile = self.get(user)?.clone();
        profile.member = None;
        profile.server = None;
        profile.server_badges = Vec::new();
        if let Some(guild) = guild {
            profile.server = self.servers.get(&(user, guild)).and_then(|h| Some((guild, h.data.clone()?)));
            if let Some((member, badges)) = self.members.get(&(user, guild)) {
                profile.member = member.clone();
                profile.server_badges = badges.clone();
            }
        }
        Some(profile)
    }

    /// Whether the last request for `user` failed.
    pub fn failed(&self, user: Snowflake) -> bool {
        self.users.get(&user).is_some_and(|h| h.failed.is_some())
    }

    /// Whether `request` is being answered.
    pub fn is_asking(&self, request: &Request) -> bool {
        self.asking.contains(&(request.user, request.guild))
    }

    pub fn mutual_guilds(&self, user: Snowflake) -> Option<&[MutualGuild]> {
        self.mutual_guilds.get(&user).map(Vec::as_slice)
    }

    pub fn mutual_friends(&self, user: Snowflake) -> Option<&[User]> {
        self.mutual_friends.get(&user).map(Vec::as_slice)
    }

    /// How many friends we share, if Discord said or the list did.
    pub fn mutual_friend_count(&self, user: Snowflake) -> Option<u32> {
        self.mutual_friend_counts.get(&user).copied()
    }

    /// Whether to ask for the list of `user`'s mutual friends, as the panel's
    /// section does when it opens (229187): not while it is on its way, nor
    /// for none, and when the list isn't here or isn't as long as the count.
    pub fn wants_friend_list(&self, user: Snowflake) -> bool {
        if self.asking_friends.contains(&user) {
            return false;
        }
        match (self.mutual_friend_count(user), self.mutual_friends.get(&user)) {
            (Some(0), _) => false,
            (Some(count), Some(list)) => list.len() != count as usize,
            _ => true,
        }
    }

    /// The list of mutual friends is on its way.
    pub fn asked_friend_list(&mut self, user: Snowflake) {
        self.asking_friends.insert(user);
    }

    /// The list of mutual friends arrived (`MUTUAL_FRIENDS_FETCH_SUCCESS`).
    pub fn friend_list_arrived(&mut self, user: Snowflake, mut friends: Vec<User>) {
        self.asking_friends.remove(&user);
        friends.sort_by_key(|f| f.username.to_lowercase());
        self.mutual_friend_counts.insert(user, friends.len() as u32);
        self.mutual_friends.insert(user, friends);
    }

    pub fn friend_list_failed(&mut self, user: Snowflake) {
        self.asking_friends.remove(&user);
    }

    /// What a profile held of `user` is worth is in doubt: their account, a
    /// member of a server of ours, or a friendship of theirs changed.
    pub fn doubt(&mut self, user: Snowflake) {
        if let Some(held) = self.users.get_mut(&user) {
            (held.ended, held.failed) = (0, None);
        }
        for ((who, _), held) in &mut self.servers {
            if *who == user {
                (held.ended, held.failed) = (0, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::experiments::murmur3;

    const NOW: u64 = 1_000_000;

    fn user(id: u64, bot: bool) -> User {
        User {
            id: Snowflake(id),
            username: format!("user{id}"),
            global_name: None,
            avatar: None,
            bot,
            system: false,
            discriminator: None,
        }
    }

    fn answer(user: u64) -> Value {
        json!({
            "user": {"id": user.to_string(), "username": "someone", "global_name": "Someone"},
            "user_profile": {"bio": "hello", "pronouns": "they/them", "banner": "abc"},
            "badges": [{"id": "staff", "description": "Staff", "icon": "icon1", "link": "https://example.com"}],
            "mutual_guilds": [{"id": "10", "nick": null}, {"id": "11", "nick": "nick"}],
            "mutual_friends_count": 2,
            "guild_member": {"nick": "server nick", "roles": ["5", "6"], "avatar": "av"},
            "guild_member_profile": {"guild_id": "10", "bio": "server bio", "pronouns": "", "banner": null},
        })
    }

    #[test]
    fn popout_query_follows_the_clients_field_order() {
        let me = Snowflake(1);
        let mut request = Request::popout(&user(2, false), me, Some(Snowflake(10)), Some(Snowflake(7)));
        assert_eq!(
            request.query(),
            "type=popout&with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false\
             &guild_id=10&connections_role_id=7"
        );
        request.guild = None;
        request.connections_role = None;
        assert_eq!(
            request.query(),
            "type=popout&with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false"
        );
    }

    #[test]
    fn the_panel_asks_for_a_count_or_the_friends_by_the_experiment() {
        let person = user(2, false);
        let plain = Request::sidebar(&person, false);
        assert_eq!(
            plain.query(),
            "type=sidebar&with_mutual_guilds=true&with_mutual_friends=false&with_mutual_friends_count=true"
        );
        let redesigned = Request::sidebar(&person, true);
        assert_eq!(
            redesigned.query(),
            "type=sidebar&with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false"
        );
        // A bot has no friends to share, whichever it is.
        let bot = Request::sidebar(&user(3, true), true);
        assert_eq!(
            bot.query(),
            "type=sidebar&with_mutual_guilds=true&with_mutual_friends=false&with_mutual_friends_count=false"
        );
    }

    #[test]
    fn the_redesign_is_variant_one_of_its_experiment() {
        let assigned = |variant: i64| {
            Experiments::from_ready(&json!({
                "apex_experiments": { "assignments": { "1": { "10": { "assignments": [
                    [murmur3(SIDEBAR_REDESIGN), variant, 0, 1]
                ] } } } }
            }))
        };
        assert!(sidebar_redesign(&assigned(1), Snowflake(10)));
        assert!(!sidebar_redesign(&assigned(0), Snowflake(10)));
        assert!(!sidebar_redesign(&Experiments::default(), Snowflake(10)), "none assigned");
    }

    #[test]
    fn a_count_asked_for_and_not_held_asks_again() {
        let mut profiles = Profiles::default();
        let asked = Request::sidebar(&user(2, false), false);
        profiles.answered(&asked, Profile::from_value(&answer(2)).unwrap(), NOW);
        // Their count came with the answer.
        assert_eq!(profiles.plan(&asked, NOW + 1), Fetch::No);
        let mut value = answer(4);
        value["mutual_friends_count"] = Value::Null;
        profiles.answered(&Request::sidebar(&user(4, false), false), Profile::from_value(&value).unwrap(), NOW);
        assert_eq!(profiles.plan(&Request::sidebar(&user(4, false), false), NOW + 1), Fetch::Behind);
    }

    #[test]
    fn the_friend_list_is_asked_for_until_it_is_as_long_as_the_count() {
        let mut profiles = Profiles::default();
        let them = Snowflake(2);
        assert!(profiles.wants_friend_list(them), "nothing is known");
        profiles.answered(&Request::sidebar(&user(2, false), false), Profile::from_value(&answer(2)).unwrap(), NOW);
        assert_eq!(profiles.mutual_friend_count(them), Some(2));
        assert!(profiles.wants_friend_list(them), "a count and no list");
        profiles.asked_friend_list(them);
        assert!(!profiles.wants_friend_list(them), "on its way");
        profiles.friend_list_failed(them);
        assert!(profiles.wants_friend_list(them), "a failure asks again");
        profiles.friend_list_arrived(them, vec![user(9, false), user(8, false)]);
        assert!(!profiles.wants_friend_list(them), "as long as the count");
        let names: Vec<u64> = profiles.mutual_friends(them).unwrap().iter().map(|u| u.id.0).collect();
        assert_eq!(names, [8, 9], "by name");
        // A count of none needs no list.
        profiles.answered(
            &request(5),
            Profile::from_value(&json!({"user": {"id": "5", "username": "x"}, "mutual_friends_count": 0})).unwrap(),
            NOW,
        );
        assert!(!profiles.wants_friend_list(Snowflake(5)));
    }

    #[test]
    fn a_profile_while_its_friends_are_asked_with_it_waits_for_them() {
        let mut profiles = Profiles::default();
        let asked = Request::popout(&user(2, false), Snowflake(1), None, None);
        profiles.asked(&asked);
        assert!(!profiles.wants_friend_list(Snowflake(2)), "they come with the profile");
        profiles.refused(&asked, Some(500), NOW);
        assert!(profiles.wants_friend_list(Snowflake(2)));
    }

    #[test]
    fn decorations_have_a_still_image_until_they_lapse() {
        let shop = Decoration::from_value(&json!({"asset": "a_x", "sku_id": "77", "expires_at": null})).unwrap();
        assert_eq!(
            shop.url(0, 128).as_deref(),
            Some("https://cdn.discordapp.com/media/v1/collectibles-shop/77/static")
        );
        let preset = Decoration::from_value(&json!({"asset": "a_x"})).unwrap();
        assert!(preset.url(0, 128).unwrap().contains("/avatar-decoration-presets/a_x.png?size=128"));
        assert_eq!(Decoration::from_value(&json!(null)), None);
        let lapsing =
            Decoration::from_value(&json!({"sku_id": "7", "expires_at": "2020-01-01T00:00:00+00:00"})).unwrap();
        assert!(lapsing.url(1_500_000_000, 128).is_some(), "before");
        assert_eq!(lapsing.url(1_700_000_000, 128), None, "after");
    }

    #[test]
    fn what_a_server_says_of_a_member_stays_in_that_server() {
        let mut profiles = Profiles::default();
        let (user, here, there) = (Snowflake(3), Snowflake(10), Snowflake(11));
        let mut asked = request(3);
        asked.guild = Some(here);
        profiles.answered(&asked, Profile::from_value(&answer(3)).unwrap(), NOW);
        let shown = profiles.get_in(user, Some(here)).unwrap();
        assert_eq!(shown.member.as_ref().unwrap().nick.as_deref(), Some("server nick"));
        assert_eq!(shown.shown().bio, "server bio");
        for elsewhere in [Some(there), None] {
            let shown = profiles.get_in(user, elsewhere).unwrap();
            assert!(shown.member.is_none() && shown.server.is_none());
            assert_eq!(shown.shown().bio, "hello", "their own");
        }
        // An answer for no server leaves what we held of that server alone.
        profiles.answered(&request(3), Profile::from_value(&answer_without_server(3)).unwrap(), NOW);
        assert!(profiles.get_in(user, Some(here)).unwrap().member.is_some());
    }

    fn answer_without_server(user: u64) -> Value {
        let mut value = answer(user);
        value.as_object_mut().unwrap().remove("guild_member");
        value.as_object_mut().unwrap().remove("guild_member_profile");
        value
    }

    #[test]
    fn when_they_joined_a_server_is_read() {
        let mut value = answer(3);
        value["guild_member"]["joined_at"] = json!("2023-01-01T00:00:00+00:00");
        let joined = Profile::from_value(&value).unwrap().member.unwrap().joined_at;
        assert_eq!(joined, Some(1_672_531_200));
    }

    #[test]
    fn bots_and_ourselves_ask_for_less() {
        let me = Snowflake(1);
        let bot = Request::popout(&user(2, true), me, None, None);
        assert_eq!((bot.mutual_guilds, bot.mutual_friends), (true, false));
        let own = Request::popout(&user(1, false), me, None, None);
        assert_eq!((own.mutual_guilds, own.mutual_friends), (false, false));
        assert!(own.query().starts_with("type=popout&with_mutual_guilds=false&with_mutual_friends=false&"));
    }

    #[test]
    fn reads_the_answer() {
        let profile = Profile::from_value(&answer(3)).unwrap();
        assert_eq!(profile.user.id, Snowflake(3));
        assert_eq!(profile.badges.len(), 1);
        assert_eq!(profile.member.as_ref().unwrap().roles, [Snowflake(5), Snowflake(6)]);
        assert_eq!(profile.mutual_guilds.as_ref().unwrap().len(), 2);
        assert_eq!(profile.mutual_friends_count, Some(2));
    }

    #[test]
    fn a_servers_bio_covers_theirs_only_where_it_says_something() {
        let shown = Profile::from_value(&answer(3)).unwrap();
        let shown = shown.shown();
        assert_eq!((shown.bio, shown.pronouns), ("server bio", "they/them"));
        assert_eq!(shown.banner, Some(Banner::User("abc")));
        let mut value = answer(3);
        value["guild_member_profile"]["banner"] = json!("server banner");
        let profile = Profile::from_value(&value).unwrap();
        assert_eq!(profile.shown().banner, Some(Banner::Server { guild: Snowflake(10), hash: "server banner" }));
    }

    #[test]
    fn a_list_of_friends_gives_the_count() {
        let mut value = answer(3);
        value["mutual_friends"] =
            json!([{"id": "8", "username": "b"}, {"id": "9", "username": "a"}, {"id": "4", "username": "c"}]);
        assert_eq!(Profile::from_value(&value).unwrap().mutual_friends_count, Some(3));
    }

    fn request(user: u64) -> Request {
        Request::popout(&user_for(user), Snowflake(1), None, None)
    }

    fn user_for(id: u64) -> User {
        user(id, false)
    }

    #[test]
    fn nothing_held_is_asked_for_before_showing() {
        assert_eq!(Profiles::default().plan(&request(2), NOW), Fetch::Before);
    }

    #[test]
    fn a_current_profile_with_what_was_wanted_is_not_asked_again() {
        let mut profiles = Profiles::default();
        let mut value = answer(2);
        value["mutual_friends"] = json!([]);
        profiles.answered(&request(2), Profile::from_value(&value).unwrap(), NOW);
        assert_eq!(profiles.plan(&request(2), NOW + 59_999), Fetch::No);
        assert_eq!(profiles.plan(&request(2), NOW + 60_000), Fetch::Behind);
    }

    #[test]
    fn a_profile_short_of_what_is_wanted_is_asked_again_behind_it() {
        let mut profiles = Profiles::default();
        // Their mutual servers came, their friends didn't.
        profiles.answered(&request(2), Profile::from_value(&answer(2)).unwrap(), NOW);
        assert_eq!(profiles.plan(&request(2), NOW), Fetch::Behind);
        let mut bot = request(2);
        bot.mutual_friends = false;
        assert_eq!(profiles.plan(&bot, NOW), Fetch::No);
    }

    #[test]
    fn what_is_being_asked_is_not_asked_again() {
        let mut profiles = Profiles::default();
        profiles.asked(&request(2));
        assert_eq!(profiles.plan(&request(2), NOW), Fetch::No);
        assert!(profiles.is_asking(&request(2)));
        profiles.refused(&request(2), Some(500), NOW);
        assert!(!profiles.is_asking(&request(2)));
    }

    #[test]
    fn a_refusal_that_says_stop_holds_for_a_minute() {
        for status in [404, 429] {
            let mut profiles = Profiles::default();
            profiles.refused(&request(2), Some(status), NOW);
            assert_eq!(profiles.plan(&request(2), NOW + 1), Fetch::No, "{status}");
            assert_eq!(profiles.plan(&request(2), NOW + 60_000), Fetch::Behind, "{status}");
        }
    }

    #[test]
    fn another_failure_is_asked_again_for_what_it_left_missing() {
        let mut profiles = Profiles::default();
        profiles.refused(&request(2), Some(500), NOW);
        assert_eq!(profiles.plan(&request(2), NOW + 1), Fetch::Behind);
        assert!(profiles.failed(Snowflake(2)));
    }

    #[test]
    fn no_such_user_shares_nothing() {
        let mut profiles = Profiles::default();
        profiles.refused(&request(2), Some(404), NOW);
        assert_eq!(profiles.mutual_guilds(Snowflake(2)), Some(&[][..]));
        assert_eq!(profiles.mutual_friends(Snowflake(2)), Some(&[][..]));
    }

    #[test]
    fn a_server_that_sends_no_profile_is_asked_every_time() {
        let mut profiles = Profiles::default();
        let mut asked = request(2);
        asked.guild = Some(Snowflake(10));
        let mut value = answer(2);
        value["mutual_friends"] = json!([]);
        value["guild_member_profile"] = Value::Null;
        profiles.answered(&asked, Profile::from_value(&value).unwrap(), NOW);
        assert_eq!(profiles.plan(&asked, NOW + 1), Fetch::Before);
        // Elsewhere what came is current.
        assert_eq!(profiles.plan(&request(2), NOW + 1), Fetch::No);
    }

    #[test]
    fn doubt_makes_a_profile_stale_and_forgives_a_refusal() {
        let mut profiles = Profiles::default();
        let mut value = answer(2);
        value["mutual_friends"] = json!([]);
        profiles.answered(&request(2), Profile::from_value(&value).unwrap(), NOW);
        profiles.doubt(Snowflake(2));
        assert_eq!(profiles.plan(&request(2), NOW + 1), Fetch::Behind);
        profiles.refused(&request(2), Some(429), NOW);
        profiles.doubt(Snowflake(2));
        assert_eq!(profiles.plan(&request(2), NOW + 1), Fetch::Behind);
    }

    fn role(id: u64, position: i32, linked: bool) -> Role {
        serde_json::from_value(json!({
            "id": id.to_string(), "position": position,
            "tags": if linked { json!({"guild_connections": null}) } else { json!({}) },
        }))
        .unwrap()
    }

    fn channel(overwrites: Value) -> Channel {
        serde_json::from_value(json!({"id": "99", "type": 0, "permission_overwrites": overwrites})).unwrap()
    }

    #[test]
    fn only_linked_roles_count_for_connections() {
        let roles = [role(1, 0, false), role(5, 3, false), role(6, 2, true), role(7, 1, true)];
        let member = [Snowflake(5), Snowflake(6), Snowflake(7)];
        assert_eq!(connections_role(&roles, Snowflake(1), &member, None), Some(Snowflake(6)));
        assert_eq!(connections_role(&roles, Snowflake(1), &member[..1], None), None);
    }

    #[test]
    fn roles_are_listed_with_linked_ones_last() {
        let roles = [role(1, 0, false), role(5, 1, false), role(6, 9, true), role(7, 3, false), role(8, 3, false)];
        let member: Vec<Snowflake> = [5, 6, 7, 8].map(Snowflake).into();
        let listed: Vec<u64> = listed_roles(&roles, Snowflake(1), &member).iter().map(|r| r.id.0).collect();
        // Higher first, the older of two at one height first, and the linked one after them all.
        assert_eq!(listed, [7, 8, 5, 6]);
    }

    #[test]
    fn a_channel_prefers_the_linked_roles_it_lets_talk() {
        let roles = [role(6, 2, true), role(7, 1, true)];
        let member = [Snowflake(6), Snowflake(7)];
        let overwrite = |role: u64, deny: Permissions| json!({"id": role.to_string(), "type": 0, "allow": "0", "deny": deny.0.to_string()});
        let pick = |overwrites: Value| connections_role(&roles, Snowflake(1), &member, Some(&channel(overwrites)));
        // Denied either one of seeing and writing, a role no longer counts.
        let (view, send) = (Permissions::VIEW_CHANNEL, Permissions::SEND_MESSAGES);
        assert_eq!(pick(json!([overwrite(6, send), overwrite(7, Permissions::NONE)])), Some(Snowflake(7)));
        assert_eq!(pick(json!([overwrite(6, view), overwrite(7, Permissions::NONE)])), Some(Snowflake(7)));
        // What else it denies doesn't matter.
        let other = Permissions::ADD_REACTIONS;
        assert_eq!(pick(json!([overwrite(6, other), overwrite(7, Permissions::NONE)])), Some(Snowflake(6)));
        // With none of them in the overwrites, the highest of theirs.
        assert_eq!(pick(json!([])), Some(Snowflake(6)));
        // Nor does a channel kind the client skips narrow them.
        let mut stage = channel(json!([overwrite(6, send), overwrite(7, Permissions::NONE)]));
        stage.kind = ChannelKind::Stage;
        assert_eq!(connections_role(&roles, Snowflake(1), &member, Some(&stage)), Some(Snowflake(6)));
    }
}
