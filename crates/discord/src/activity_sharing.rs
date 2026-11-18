//! What the client asks after the Profile Privacy row changes (`gS`, 365258;
//! the dialog is 413201), or the activity sharing default does (`g8`): the
//! default follows the profile visibility, and the servers that would then
//! differ from it are offered to follow too.

use std::collections::HashSet;

use crate::Snowflake;
use crate::settings::{ActivityDefault, ProfileVisibility};

/// A server counts as large above this many members.
const LARGE: u64 = 200;

/// What the servers asked about are told to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Sharing activity stops in them.
    Restricting,
    /// Sharing starts in them.
    Expanding,
}

/// What the client knows of a server for the question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Server {
    pub id: Snowflake,
    /// Seconds since 1970; the account's own join.
    pub joined_at: Option<i64>,
    /// None until a count is known.
    pub member_count: Option<u64>,
}

/// What changed to bring the question about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The profile visibility, which the default follows.
    ProfileVisibility,
    /// The default itself, in Activity Privacy.
    Default,
}

/// The question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub cause: Cause,
    /// The servers it is asked for, those joined last first.
    pub servers: Vec<Snowflake>,
    pub direction: Direction,
    /// What the default becomes if the answer is yes.
    pub default: ActivityDefault,
    /// The new visibility as the dialog says it, lower case.
    pub setting_name: &'static str,
}

/// What a visibility maps to (`A`, 365258).
fn mapped(visibility: ProfileVisibility) -> ActivityDefault {
    match visibility {
        ProfileVisibility::FriendsAndAllServers => ActivityDefault::Off,
        ProfileVisibility::FriendsAndSmallServers => ActivityDefault::LargeServers,
        ProfileVisibility::FriendsOnly => ActivityDefault::On,
    }
}

/// The dialog's question for the profile visibility changing from `old` to
/// `new`, if it asks one (`gS`). `restricted` is the list of servers where
/// activity isn't shared, as the client reads it, and `servers` those we are
/// in, in the order of the server rail.
pub fn after_profile_privacy(
    old: ProfileVisibility,
    new: ProfileVisibility,
    current: ActivityDefault,
    restricted: &[Snowflake],
    servers: &[Server],
) -> Option<Change> {
    let default = mapped(new);
    // The default already is what the new visibility maps to, or it was not
    // following the old one: its change would not match what was chosen.
    if current == default
        || (mapped(old).restraint() > default.restraint()) != (current.restraint() > default.restraint())
    {
        return None;
    }
    let (servers, direction) = affected(current, default, restricted, servers)?;
    Some(Change {
        cause: Cause::ProfileVisibility,
        servers,
        direction,
        default,
        setting_name: match new {
            ProfileVisibility::FriendsAndAllServers => "friends & all servers",
            ProfileVisibility::FriendsAndSmallServers => "friends & small servers only",
            ProfileVisibility::FriendsOnly => "friends only",
        },
    })
}

/// The question for the default itself changing from `old` to `new` (`g8`),
/// which Activity Privacy asks.
pub fn after_default(
    old: ActivityDefault,
    new: ActivityDefault,
    restricted: &[Snowflake],
    servers: &[Server],
) -> Option<Change> {
    if old == new {
        return None;
    }
    let (servers, direction) = affected(old, new, restricted, servers)?;
    Some(Change {
        cause: Cause::Default,
        servers,
        direction,
        default: new,
        // In the words of the profile visibility each default stands for (`Xc`).
        setting_name: match new {
            ActivityDefault::Off => "friends & all servers",
            ActivityDefault::LargeServers => "friends & small servers only",
            ActivityDefault::On => "friends only",
        },
    })
}

/// The servers that differ from `new` once the default moves there from
/// `current`, those joined last first, and which way (`g8`, 365258).
fn affected(
    current: ActivityDefault,
    default: ActivityDefault,
    restricted: &[Snowflake],
    servers: &[Server],
) -> Option<(Vec<Snowflake>, Direction)> {
    let direction =
        if default.restraint() < current.restraint() { Direction::Restricting } else { Direction::Expanding };
    #[derive(PartialEq)]
    enum Scope {
        All,
        Large,
        Small,
    }
    let scope = match (current, default) {
        (ActivityDefault::Off, ActivityDefault::LargeServers)
        | (ActivityDefault::LargeServers, ActivityDefault::Off) => Scope::Large,
        (ActivityDefault::On, ActivityDefault::LargeServers) | (ActivityDefault::LargeServers, ActivityDefault::On) => {
            Scope::Small
        }
        _ => Scope::All,
    };
    let listed: HashSet<Snowflake> = restricted.iter().copied().collect();
    let mut affected: Vec<&Server> = servers
        .iter()
        .filter(|server| {
            let held = listed.contains(&server.id);
            if (direction == Direction::Restricting && held) || (direction == Direction::Expanding && !held) {
                return false;
            }
            match (&scope, server.member_count) {
                (Scope::All, _) => true,
                (_, None) => direction == Direction::Restricting,
                (Scope::Large, Some(count)) => count > LARGE,
                (Scope::Small, Some(count)) => count <= LARGE,
            }
        })
        .collect();
    if affected.is_empty() {
        return None;
    }
    // Those joined last first; one with no date after the others.
    affected.sort_by(|a, b| match (a.joined_at, b.joined_at) {
        (Some(a), Some(b)) => b.cmp(&a),
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
    });
    Some((affected.into_iter().map(|server| server.id).collect(), direction))
}

/// A server in Activity Privacy's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: Snowflake,
    pub name: String,
    /// Seconds since 1970.
    pub joined_at: Option<i64>,
}

/// How the list is ordered (`S4` in 212909).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    Server,
    Newest,
    SharingOn,
    SharingOff,
}

impl Order {
    pub const ALL: [Order; 4] = [Order::Server, Order::Newest, Order::SharingOn, Order::SharingOff];

    pub fn label(self) -> &'static str {
        match self {
            Order::Server => "Server Order",
            Order::Newest => "Newest",
            Order::SharingOn => "Activity Sharing On",
            Order::SharingOff => "Activity Sharing Off",
        }
    }
}

/// The servers to list: those whose name has `query` in it (any case),
/// ordered as `order` says. `servers` is in the order of the server list, and
/// `restricted` those without sharing. A server with no join date comes
/// before the dated ones in Newest, as it does in the client.
pub fn listed(servers: &[Listed], query: &str, order: Order, restricted: &[Snowflake]) -> Vec<Listed> {
    let mut shown: Vec<Listed> = servers.to_vec();
    let held = |server: &Listed| restricted.contains(&server.id);
    match order {
        Order::Server => {}
        Order::Newest => shown.sort_by_key(|server| (server.joined_at.is_some(), std::cmp::Reverse(server.joined_at))),
        Order::SharingOn => shown.sort_by_key(held),
        Order::SharingOff => shown.sort_by_key(|server| !held(server)),
    }
    let query = query.to_lowercase();
    shown.retain(|server| query.is_empty() || server.name.to_lowercase().contains(&query));
    shown
}

/// "Toggle All On" when any server is without sharing, else "Toggle All Off".
/// The list of servers without sharing it saves: none, or every one of
/// `all`.
pub fn toggle_all(restricted: &[Snowflake], all: &[Snowflake]) -> Vec<Snowflake> {
    if restricted.is_empty() { all.to_vec() } else { Vec::new() }
}

/// What the button over the list says (`7lxcLO` and `zh6UEs`).
pub fn toggle_all_label(restricted: &[Snowflake]) -> &'static str {
    if restricted.is_empty() { "Toggle All Off" } else { "Toggle All On" }
}

impl Change {
    /// The list of servers without sharing once it is confirmed (`gF`).
    pub fn restricted_after(&self, restricted: &[Snowflake]) -> Vec<Snowflake> {
        match self.direction {
            Direction::Restricting => {
                let mut all = restricted.to_vec();
                for id in &self.servers {
                    if !all.contains(id) {
                        all.push(*id);
                    }
                }
                all
            }
            Direction::Expanding => restricted.iter().copied().filter(|id| !self.servers.contains(id)).collect(),
        }
    }

    /// The dialog's title and the question under it (`vz` after the profile
    /// visibility, `ae` after the default).
    pub fn title(&self) -> &'static str {
        match (self.cause, self.direction) {
            (Cause::ProfileVisibility, Direction::Restricting) => "Stop sharing activity too?",
            (Cause::ProfileVisibility, Direction::Expanding) => "Share activity too?",
            (Cause::Default, Direction::Restricting) => "Turn Off Activity In Your Servers?",
            (Cause::Default, Direction::Expanding) => "Turn On Activity In Your Servers?",
        }
    }

    /// The question: `name` is in the text in bold.
    pub fn question(&self, name: &str) -> String {
        match (self.cause, self.direction) {
            (Cause::ProfileVisibility, Direction::Restricting) => format!(
                "Your profile visibility is set to {name}. Update your activity sharing settings to {name} too? This will turn off activity in these servers"
            ),
            (Cause::ProfileVisibility, Direction::Expanding) => format!(
                "Your profile visibility is set to {name}. Update your activity sharing setting to {name} too? This will turn on activity in these servers"
            ),
            (Cause::Default, _) => {
                format!("You changed your default sharing settings to {name}. Update your servers to match?")
            }
        }
    }

    pub fn confirm(&self) -> &'static str {
        match (self.cause, self.direction) {
            (Cause::ProfileVisibility, Direction::Restricting) => "Stop sharing",
            (Cause::ProfileVisibility, Direction::Expanding) => "Share",
            (Cause::Default, Direction::Restricting) => "Turn Off",
            (Cause::Default, Direction::Expanding) => "Turn On",
        }
    }

    pub fn toast(&self) -> &'static str {
        match self.direction {
            Direction::Restricting => "Servers synced",
            Direction::Expanding => "Activity turned on",
        }
    }

    /// What the card of servers says they do now (`e6Kpa7` and `cy4G4y`).
    pub fn card_state(&self) -> &'static str {
        match self.direction {
            Direction::Restricting => "Sharing",
            Direction::Expanding => "Not Sharing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: u64, joined: Option<i64>, members: Option<u64>) -> Server {
        Server { id: Snowflake(id), joined_at: joined, member_count: members }
    }

    fn ids(ids: &[u64]) -> Vec<Snowflake> {
        ids.iter().copied().map(Snowflake).collect()
    }

    use ProfileVisibility::{FriendsAndAllServers as All, FriendsAndSmallServers as Small, FriendsOnly as Only};

    #[test]
    fn the_default_changing_asks_for_the_servers_that_would_differ() {
        let servers = [server(1, Some(10), Some(500)), server(2, Some(20), Some(50)), server(3, Some(30), None)];
        // From sharing in all servers to sharing in none.
        let change = after_default(ActivityDefault::Off, ActivityDefault::On, &[], &servers).unwrap();
        assert_eq!(change.cause, Cause::Default);
        assert_eq!((change.direction, change.servers.clone()), (Direction::Restricting, ids(&[3, 2, 1])));
        assert_eq!(
            (change.title(), change.confirm(), change.setting_name),
            ("Turn Off Activity In Your Servers?", "Turn Off", "friends only")
        );
        assert_eq!(
            change.question("<b>x</b>"),
            "You changed your default sharing settings to <b>x</b>. Update your servers to match?"
        );
        assert_eq!(change.toast(), "Servers synced");
        // Only the large ones, and those of unknown size, stop between "all" and "small servers".
        let change = after_default(ActivityDefault::Off, ActivityDefault::LargeServers, &[], &servers).unwrap();
        assert_eq!((change.direction, change.servers.clone()), (Direction::Restricting, ids(&[3, 1])));
        let back = after_default(ActivityDefault::On, ActivityDefault::Off, &ids(&[1, 2, 3]), &servers).unwrap();
        assert_eq!(
            (back.direction, back.servers.clone(), back.title()),
            (Direction::Expanding, ids(&[3, 2, 1]), "Turn On Activity In Your Servers?")
        );
        assert_eq!(
            (back.confirm(), back.setting_name, back.toast()),
            ("Turn On", "friends & all servers", "Activity turned on")
        );
        assert!(after_default(ActivityDefault::On, ActivityDefault::On, &[], &servers).is_none());
        assert!(
            after_default(ActivityDefault::Off, ActivityDefault::On, &ids(&[1, 2, 3]), &servers).is_none(),
            "all are already without sharing"
        );
    }

    #[test]
    fn the_list_is_searched_and_sorted_as_the_client_does() {
        let listed_server = |id, name: &str, joined| Listed { id: Snowflake(id), name: name.into(), joined_at: joined };
        let all = [
            listed_server(1, "Alpha", Some(30)),
            listed_server(2, "beta club", None),
            listed_server(3, "Gamma", Some(10)),
        ];
        let order = |o, query: &str, restricted: &[u64]| {
            listed(&all, query, o, &restricted.iter().copied().map(Snowflake).collect::<Vec<_>>())
                .iter()
                .map(|s| s.id.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(Order::Server, "", &[]), [1, 2, 3]);
        assert_eq!(order(Order::Newest, "", &[]), [2, 1, 3], "no date first, then the newest");
        assert_eq!(order(Order::SharingOn, "", &[1]), [2, 3, 1], "those sharing first, the rest as they were");
        assert_eq!(order(Order::SharingOff, "", &[3, 2]), [2, 3, 1]);
        assert_eq!(order(Order::Server, "BE", &[]), [2], "any case, anywhere in the name");
        assert!(order(Order::Server, "zzz", &[]).is_empty());
        assert_eq!(toggle_all(&[], &ids(&[1, 2])), ids(&[1, 2]));
        assert_eq!(toggle_all(&ids(&[2]), &ids(&[1, 2])), ids(&[]));
        assert_eq!((toggle_all_label(&[]), toggle_all_label(&ids(&[2]))), ("Toggle All Off", "Toggle All On"));
    }

    #[test]
    fn nothing_is_asked_where_the_default_already_is_what_the_visibility_maps_to() {
        let servers = [server(1, Some(10), Some(5))];
        assert_eq!(after_profile_privacy(All, Only, ActivityDefault::On, &[], &servers), None);
    }

    #[test]
    fn nothing_is_asked_where_the_default_did_not_follow_the_old_visibility() {
        // Friends only to small servers, with the default set to nothing shared: it was never the old one.
        let servers = [server(1, Some(10), Some(500))];
        assert_eq!(after_profile_privacy(Only, Small, ActivityDefault::Off, &[], &servers), None);
        assert!(after_profile_privacy(All, Small, ActivityDefault::Off, &[], &servers).is_some());
    }

    #[test]
    fn restricting_asks_for_the_servers_not_yet_listed_in_the_order_they_were_joined() {
        let servers = [
            server(1, Some(10), Some(5)),
            server(2, None, Some(5)),
            server(3, Some(30), Some(5)),
            server(4, Some(20), Some(5)),
            server(5, Some(40), Some(5)),
        ];
        let change = after_profile_privacy(All, Only, ActivityDefault::Off, &ids(&[5]), &servers).unwrap();
        assert_eq!(change.servers, ids(&[3, 4, 1, 2]), "5 is not shared already; the one with no date last");
        assert_eq!(
            (change.direction, change.default, change.setting_name),
            (Direction::Restricting, ActivityDefault::On, "friends only")
        );
        assert_eq!(change.restricted_after(&ids(&[5, 3])), ids(&[5, 3, 4, 1, 2]), "added once, after those held");
    }

    #[test]
    fn expanding_asks_for_the_servers_listed_and_removes_them() {
        let servers = [server(1, Some(10), Some(5)), server(2, Some(20), Some(5))];
        let change = after_profile_privacy(Only, All, ActivityDefault::On, &ids(&[1, 9]), &servers).unwrap();
        assert_eq!(change.servers, ids(&[1]));
        assert_eq!(change.direction, Direction::Expanding);
        assert_eq!(change.restricted_after(&ids(&[1, 9])), ids(&[9]));
        assert_eq!(
            after_profile_privacy(Only, All, ActivityDefault::On, &[], &servers),
            None,
            "none listed: none to ask"
        );
    }

    #[test]
    fn between_nothing_and_large_servers_only_the_large_ones_count() {
        let servers = [server(1, Some(1), Some(200)), server(2, Some(2), Some(201)), server(3, Some(3), None)];
        let change = after_profile_privacy(All, Small, ActivityDefault::Off, &[], &servers).unwrap();
        assert_eq!(change.servers, ids(&[3, 2]), "restricting to large ones: above 200, or unknown");
    }

    #[test]
    fn between_large_servers_and_everyone_only_the_small_ones_count() {
        let servers = [server(1, Some(1), Some(200)), server(2, Some(2), Some(201)), server(3, Some(3), None)];
        // Restricting what was shared in large servers: the small ones, and one of unknown size.
        let change = after_profile_privacy(Small, Only, ActivityDefault::LargeServers, &[], &servers).unwrap();
        assert_eq!(change.servers, ids(&[3, 1]));
    }

    #[test]
    fn expanding_to_small_servers_asks_for_the_listed_small_ones() {
        let servers = [server(1, Some(1), Some(200)), server(2, Some(2), Some(201)), server(3, Some(3), None)];
        let change = after_profile_privacy(Only, Small, ActivityDefault::On, &ids(&[1, 2, 3]), &servers).unwrap();
        assert_eq!(change.direction, Direction::Expanding);
        assert_eq!(change.servers, ids(&[1]), "up to 200; one of unknown size is not asked for");
    }

    #[test]
    fn the_dialog_says_what_it_does() {
        let servers = [server(1, Some(1), Some(1))];
        let stop = after_profile_privacy(All, Only, ActivityDefault::Off, &[], &servers).unwrap();
        assert_eq!(
            (stop.title(), stop.confirm(), stop.toast()),
            ("Stop sharing activity too?", "Stop sharing", "Servers synced")
        );
        let start = after_profile_privacy(Only, All, ActivityDefault::On, &ids(&[1]), &servers).unwrap();
        assert_eq!(
            (start.title(), start.confirm(), start.toast()),
            ("Share activity too?", "Share", "Activity turned on")
        );
    }
}
