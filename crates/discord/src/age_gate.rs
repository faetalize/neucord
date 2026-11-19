//! The age gate on age-restricted channels and servers: whether opening one
//! shows the gate instead, and what the gate says. The client's rules are
//! `P` and `M` in 323073 (whether), `k$` there (what it says) and
//! `GuildNSFWAgreeStore` (517019), with `nsfwReturnToSafety` (778858) for
//! going back.
//!
//! Not copied: the server-specific wording the experiments of 112417 put in
//! it (off unless the account is in them). The age verification the confirm
//! button starts is [`crate::age_verification`].

use crate::Snowflake;
use crate::gates::Gates;
use crate::verification::Account;

/// What decides, of the account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    /// Discord says the account is not an adult's (`nsfwAllowed === false`).
    pub underage: bool,
    /// The region age-gates spaces and the account isn't an adult
    /// ([`Gates::spaces_age_gated`]): it is asked again whatever it said.
    pub assurance: bool,
    /// Its age group is unknown ([`Gates::age_unverified`]).
    pub unverified: bool,
}

impl Standing {
    pub fn of(account: &Account, gates: &Gates) -> Standing {
        Standing {
            underage: account.nsfw_set && !account.nsfw_allowed,
            assurance: gates.spaces_age_gated,
            unverified: gates.age_unverified,
        }
    }
}

/// Whether opening a channel shows the gate (`P`/`M` in 323073): the channel
/// is age-restricted, or its server is, and we haven't agreed to go in for
/// good, which an underage account never has and one the region gates
/// always starts again from.
pub fn gated(standing: Standing, agreed: bool, channel_restricted: bool, guild_restricted: bool) -> bool {
    let agreed = agreed && !standing.assurance;
    (channel_restricted || guild_restricted) && (!agreed || standing.underage || standing.assurance)
}

/// Whether we agreed to go into a server for good (`didAgree` in 517019): a
/// server we agreed to, unless the region asks the account again.
pub fn did_agree(standing: Standing, agreed: bool) -> bool {
    agreed && !standing.assurance
}

/// What `include_nsfw` says of a search in a server we agreed to go into
/// (`L5` in 822382): whether the account is an adult's, which an account
/// Discord says nothing of counts as. `None` for a server we didn't agree to.
pub fn include_nsfw(account: &Account, standing: Standing, agreed: bool) -> Option<bool> {
    did_agree(standing, agreed).then_some(!account.nsfw_set || account.nsfw_allowed)
}

/// What the gate asks of us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Confirm the age group: the account's isn't known, or is a teen's.
    Verify,
    /// An account that is not an adult's: only going back.
    Underage,
    /// An adult, asked whether to go in.
    Verified,
}

/// The gate's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub title: &'static str,
    /// Pango markup: the help article is a link, the age group bold.
    pub description: String,
    /// The button that goes on, if there is one.
    pub agreement: Option<&'static str>,
    pub kind: Kind,
}

/// Where the help articles are (`getArticleURL`).
fn article(id: &str) -> String {
    format!("https://support.discord.com/hc/en-us/articles/{id}")
}

/// `k$` in 323073, for a channel or, with `guild`, for a server.
pub fn screen(standing: Standing, guild: bool) -> Screen {
    let title = if guild { "Age-Restricted Server" } else { "Age-Restricted Channel" };
    if standing.assurance {
        if !standing.unverified {
            let description = if guild {
                "This server contains sensitive content marked as age-restricted. We determined you're in the <b>teen</b> age group. If we made a mistake, you can update your age."
            } else {
                "This channel contains adult content marked as age-restricted. We determined you're in the <b>teen</b> age group. If we made a mistake, you can update your age."
            };
            return Screen {
                title,
                description: description.to_owned(),
                agreement: Some("Update Age"),
                kind: Kind::Verify,
            };
        }
        let description = if guild {
            "This server is marked as age-restricted. It may contain sensitive content. Do you want to confirm your age group to access this server?"
        } else {
            "This channel is marked as age-restricted because it contains adult content. Do you want to confirm your age group to access this channel?"
        };
        return Screen {
            title,
            description: description.to_owned(),
            agreement: Some("Confirm Age"),
            kind: Kind::Verify,
        };
    }
    if standing.underage {
        let (title, before, link, after, article_id) = if guild {
            (
                "You do not have access to this age-restricted server",
                "You must be over legal age in order to view age-restricted servers. Learn more about our ",
                "age verification process here",
                ".",
                "360040724612",
            )
        } else {
            (
                "Sorry, you're not old enough to view this age-restricted channel",
                "You need to be 18 or older in order to view age-restricted channels on Discord. ",
                "View our help article",
                " on age verification.",
                "115000084051#h_5206f3f2-0ee4-4380-b50a-25319e45bc7c",
            )
        };
        let description = format!("{before}<a href=\"{}\">{link}</a>{after}", article(article_id));
        return Screen { title, description, agreement: None, kind: Kind::Underage };
    }
    let description = if guild {
        "This server contains sensitive content marked as age-restricted. Do you wish to proceed?"
    } else {
        "This channel contains adult content marked as age-restricted. Do you wish to proceed?"
    };
    Screen { title, description: description.to_owned(), agreement: Some("Continue"), kind: Kind::Verified }
}

/// The button that goes back.
pub const DISAGREEMENT: &str = "Go Back";

/// The servers we agreed to go into, as what is kept says (`GuildNSFWAgreeStore`).
pub fn agreed_from(entries: &[(String, u64)]) -> Vec<Snowflake> {
    entries.iter().filter(|(_, agreed)| *agreed == 1).filter_map(|(id, _)| id.parse().ok().map(Snowflake)).collect()
}

/// What to keep once `guild` is agreed to.
pub fn agreed_entries(agreed: &[Snowflake]) -> Vec<(String, u64)> {
    agreed.iter().map(|id| (id.0.to_string(), 1)).collect()
}

/// Where going back takes us (`nsfwReturnToSafety`): the last place we were
/// that isn't gated, else the server's default channel if that isn't, else
/// the home screen (`None`). `visited` is newest first.
pub fn safety(
    visited: &[Snowflake],
    leaving: Snowflake,
    default_channel: Option<Snowflake>,
    open: &dyn Fn(Snowflake) -> bool,
) -> Option<Snowflake> {
    visited.iter().copied().find(|&c| c != leaving && open(c)).or(default_channel.filter(|&c| open(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADULT: Standing = Standing { underage: false, assurance: false, unverified: false };

    #[test]
    fn an_adult_is_asked_once_for_each_server() {
        assert!(gated(ADULT, false, true, false), "an age-restricted channel");
        assert!(gated(ADULT, false, false, true), "a channel of an age-restricted server");
        assert!(!gated(ADULT, true, true, true), "agreed");
        assert!(!gated(ADULT, false, false, false), "nothing restricted");
    }

    #[test]
    fn nothing_agreed_lets_in_an_underage_account_or_one_the_region_gates() {
        let underage = Standing { underage: true, ..ADULT };
        assert!(gated(underage, true, true, false));
        let assured = Standing { assurance: true, unverified: true, ..ADULT };
        assert!(gated(assured, true, true, false));
        assert!(!gated(assured, true, false, false));
    }

    #[test]
    fn a_search_in_a_server_we_agreed_to_says_whether_the_account_is_an_adult() {
        let account = |allowed, set| Account { nsfw_allowed: allowed, nsfw_set: set, ..Account::default() };
        assert_eq!(include_nsfw(&account(true, true), ADULT, true), Some(true));
        assert_eq!(include_nsfw(&account(false, true), ADULT, true), Some(false), "an account that isn't an adult's");
        assert_eq!(include_nsfw(&account(false, false), ADULT, true), Some(true), "null says nothing: allowed");
        assert_eq!(include_nsfw(&account(true, true), ADULT, false), None, "not agreed to");
        let assured = Standing { assurance: true, ..ADULT };
        assert_eq!(include_nsfw(&account(true, true), assured, true), None, "the region asks again each time");
    }

    #[test]
    fn only_a_false_nsfw_allowed_is_underage() {
        let account = |allowed, set| Account { nsfw_allowed: allowed, nsfw_set: set, ..Account::default() };
        let gates = Gates::default();
        assert!(Standing::of(&account(false, true), &gates).underage);
        assert!(!Standing::of(&account(false, false), &gates).underage, "null says nothing");
        assert!(!Standing::of(&account(true, true), &gates).underage);
        let region = Gates { spaces_age_gated: true, age_unverified: true, ..Gates::default() };
        let standing = Standing::of(&account(true, true), &region);
        assert!(standing.assurance && standing.unverified);
    }

    #[test]
    fn the_screen_says_what_it_asks() {
        let channel = screen(ADULT, false);
        assert_eq!(
            (channel.title, channel.kind, channel.agreement),
            ("Age-Restricted Channel", Kind::Verified, Some("Continue"))
        );
        assert!(channel.description.starts_with("This channel contains adult content"));
        let server = screen(ADULT, true);
        assert_eq!((server.title, server.agreement), ("Age-Restricted Server", Some("Continue")));

        let underage = screen(Standing { underage: true, ..ADULT }, false);
        assert_eq!((underage.kind, underage.agreement), (Kind::Underage, None));
        assert!(underage.title.starts_with("Sorry, you're not old enough"));
        assert!(underage.description.contains("<a href=\"https://support.discord.com/hc/en-us/articles/115000084051#"));
        assert!(screen(Standing { underage: true, ..ADULT }, true).description.contains("articles/360040724612\""));

        let teen = screen(Standing { assurance: true, ..ADULT }, false);
        assert_eq!((teen.kind, teen.agreement), (Kind::Verify, Some("Update Age")));
        assert!(teen.description.contains("<b>teen</b>"));
        let unknown = screen(Standing { assurance: true, unverified: true, ..ADULT }, true);
        assert_eq!(unknown.agreement, Some("Confirm Age"));
        assert!(unknown.description.starts_with("This server is marked as age-restricted."));
        // The region's gate wins over an underage answer.
        assert_eq!(screen(Standing { assurance: true, underage: true, unverified: true }, false).kind, Kind::Verify);
    }

    #[test]
    fn what_is_agreed_is_kept_and_read_back() {
        let ids = [Snowflake(5), Snowflake(9)];
        assert_eq!(agreed_from(&agreed_entries(&ids)), ids);
        assert!(agreed_from(&[("5".into(), 0), ("x".into(), 1)]).is_empty(), "not agreed, not an id");
    }

    #[test]
    fn going_back_takes_the_last_open_place_then_the_default_channel() {
        let open = |c: Snowflake| c != Snowflake(2);
        let at = |v: &[u64], default: Option<u64>| {
            let visited: Vec<_> = v.iter().map(|&i| Snowflake(i)).collect();
            safety(&visited, Snowflake(1), default.map(Snowflake), &open)
        };
        assert_eq!(at(&[1, 2, 3, 4], None), Some(Snowflake(3)), "not here, not a gated one");
        assert_eq!(at(&[1, 2], Some(7)), Some(Snowflake(7)));
        assert_eq!(at(&[1, 2], Some(2)), None, "the default is gated too: home");
        assert_eq!(at(&[], None), None);
    }
}
