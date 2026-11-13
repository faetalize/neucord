//! The app launcher behind the message box's Apps button (621377), as far as
//! the indexes Neucord already fetches carry it: the home's Recents and Apps
//! in this Server, the search over apps and commands, and an app's page.
//! The client's Recommended, directory results, activities and authorized
//! apps each need a request Neucord doesn't send, so they never show: the
//! home is what the client draws for an account whose requests for them
//! came back empty. Apps whose only commands launch an activity
//! (`PRIMARY_ENTRY_POINT`) aren't listed, since Neucord can't launch one.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::Snowflake;
use crate::commands::{App, Command, CommandIndex, Origin, Place, RunFrom, Used, command_type, natural};

/// The most apps Recents shows before View More (`lb`).
pub const RECENTS: usize = 8;
/// The most apps Apps in this Server shows before View More (`lL`).
pub const SERVER_APPS: usize = 4;
/// The commands search shows before View More (`lA`).
pub const SEARCH_COMMANDS_SHOWN: usize = 4;

/// The launcher's entrypoint from the message box (`s4.TEXT` in 500049),
/// sent as a run's `source`.
pub const SOURCE_TEXT: &str = "TEXT";

/// "Learn More" under Have Questions? (984516), in the account's language.
pub fn help_url(locale: &str) -> String {
    format!(
        "https://support-apps.discord.com/hc/{}/articles/26592957841303-How-to-Discover-and-Add-Apps",
        locale.to_lowercase()
    )
}

/// The section an app page was opened from, sent with its Send as
/// `section_name` (`yK` in 500049).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    RecentApps,
    AppsInThisServer,
    Search,
}

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Section::RecentApps => "recent_apps",
            Section::AppsInThisServer => "apps_in_this_server",
            Section::Search => "search",
        }
    }
}

/// What a command picked in the launcher runs as (§ `A` in 545152): a search
/// result fills the box with no origin, an app page's command with
/// `APPLICATION_LAUNCHER`, both from the TEXT entrypoint.
pub fn filled_in(origin: Origin) -> RunFrom {
    RunFrom { origin, section: None, source: Some(SOURCE_TEXT) }
}

/// An app page's Send (`MJ` in 735991): run at once, with the section the
/// page was opened from.
pub fn sent(section: Option<Section>) -> RunFrom {
    RunFrom { origin: Origin::LauncherView, section: section.map(Section::name), source: Some(SOURCE_TEXT) }
}

/// How much the account uses each app (`ApplicationFrecencyStore`, 630248),
/// by [`app_key`].
#[derive(Clone, Debug, Default)]
pub struct AppUsage {
    /// `getScore`.
    pub scores: HashMap<String, f64>,
    /// Each app's latest use, in milliseconds (`max(recentUses)`).
    pub latest: HashMap<String, u64>,
}

impl AppUsage {
    fn score(&self, app: &App) -> f64 {
        self.scores.get(&app_key(app)).copied().unwrap_or(0.0)
    }
}

/// What an app's use is filed under: its id, the built-ins' `"-1"`.
pub fn app_key(app: &App) -> String {
    if app.is_built_in() { "-1".into() } else { app.id.0.to_string() }
}

/// `String.localeCompare` without a locale's tailoring: letters before case,
/// a lowercase letter before its capital.
fn locale_compare(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

/// `t4` in 621377 for the apps of this place: by use, then name, and then
/// the one used last moved first (the first of `apps` when none was used).
pub fn order_apps<'a>(mut apps: Vec<&'a App>, usage: &AppUsage) -> Vec<&'a App> {
    // The latest use before the sort, over the apps as given: a strict `>`,
    // so the first wins a tie, and an app never used counts as none.
    let mut newest: Option<(Snowflake, Option<u64>)> = None;
    for app in &apps {
        let latest = usage.latest.get(&app_key(app)).copied();
        let later = match newest {
            None => true,
            Some((_, best)) => latest > best,
        };
        if later {
            newest = Some((app.id, latest));
        }
    }
    apps.sort_by(|a, b| {
        usage
            .score(b)
            .partial_cmp(&usage.score(a))
            .unwrap_or(Ordering::Equal)
            .then_with(|| locale_compare(&a.name, &b.name))
    });
    if let Some((id, _)) = newest
        && let Some(at) = apps.iter().position(|a| a.id == id)
    {
        let app = apps.remove(at);
        apps.insert(0, app);
    }
    apps
}

/// The home without a search.
#[derive(Debug)]
pub struct Home<'a> {
    /// Every app with a command here, in Recents' order.
    pub recents: Vec<&'a App>,
    /// Apps in this Server, in a server's channel only.
    pub server: Option<Vec<&'a App>>,
    /// An index is still coming: the sections wait for it.
    pub loading: bool,
}

impl Home<'_> {
    /// "Go find some things to do in search!" (`HOME_EMPTY`), without Have
    /// Questions? below it.
    pub fn empty(&self) -> bool {
        !self.loading && self.recents.is_empty() && self.server.as_ref().is_none_or(Vec::is_empty)
    }
}

pub fn home<'a>(index: &'a CommandIndex, place: &Place, usage: &AppUsage) -> Home<'a> {
    let kinds = [command_type::CHAT];
    Home {
        recents: order_apps(index.apps(place, &kinds), usage),
        server: place.guild.map(|_| order_apps(index.server_apps(place, &kinds), usage)),
        loading: index.loading(place),
    }
}

/// What the search box is searched for: the text without a leading `/`.
pub fn search_text(typed: &str) -> &str {
    typed.strip_prefix('/').unwrap_or(typed)
}

/// A command the search found, under its app.
#[derive(Clone, Copy, Debug)]
pub struct Hit<'a> {
    pub command: &'a Command,
    pub app: &'a App,
}

/// Each item in the first bucket it fits, each bucket sorted, in bucket order
/// (`n9` with no limit); what fits none is left out.
fn buckets<T>(items: Vec<T>, fits: &[&dyn Fn(&T) -> bool], order: impl Fn(&T, &T) -> Ordering) -> Vec<T> {
    let mut sorted: Vec<Vec<T>> = fits.iter().map(|_| Vec::new()).collect();
    for item in items {
        if let Some(at) = fits.iter().position(|fits| fits(&item)) {
            sorted[at].push(item);
        }
    }
    sorted
        .into_iter()
        .flat_map(|mut bucket| {
            bucket.sort_by(&order);
            bucket
        })
        .collect()
}

/// The Commands section (`lA`): the slash commands that may be used here,
/// and `builtins` under [`App::discord_built_in`], matched as typed (case
/// counts) on the name, then a name's words, inside the name, an option's
/// name leading, inside an option's name; each match by use, then name. The
/// use is looked up without the server, so a server's own command scores
/// nothing here.
pub fn search_commands<'a>(
    index: &'a CommandIndex,
    place: &Place,
    typed: &str,
    used: &Used,
    builtins: &'a [Command],
) -> Vec<Hit<'a>> {
    let query = search_text(typed);
    let builtins =
        builtins.iter().filter(|c| c.kind == command_type::CHAT).map(|command| (App::discord_built_in(), command));
    let hits: Vec<Hit> = index
        .usable(place, &[command_type::CHAT])
        .into_iter()
        .chain(builtins)
        .map(|(app, command)| Hit { command, app })
        .collect();
    let (first, rest) = {
        let mut words = query.split(' ');
        (words.next().unwrap_or_default(), words.collect::<Vec<_>>().join(" "))
    };
    let tail = |name: &str| name.split(' ').skip(1).collect::<Vec<_>>().join(" ");
    fn names<'h>(hit: &Hit<'h>) -> [&'h str; 2] {
        [hit.command.name.as_str(), hit.command.display_name.as_str()]
    }
    let leads = |hit: &Hit| names(hit).iter().any(|n| n.starts_with(query));
    let words = |hit: &Hit| names(hit).iter().any(|n| n.starts_with(first) && tail(n).starts_with(&rest));
    let inside = |hit: &Hit| names(hit).iter().any(|n| n.contains(query));
    let option_leads = |hit: &Hit| {
        hit.command.options.iter().any(|o| {
            [Some(&o.name), o.server_name.as_ref()].into_iter().flatten().any(|option| {
                option.starts_with(query)
                    || format!("{} {option}", hit.command.name).starts_with(query)
                    || format!("{} {option}", hit.command.display_name).starts_with(query)
            })
        })
    };
    let option_inside = |hit: &Hit| {
        hit.command
            .options
            .iter()
            .any(|o| o.name.contains(query) || o.server_name.as_ref().is_some_and(|n| n.contains(query)))
    };
    buckets(hits, &[&leads, &words, &inside, &option_leads, &option_inside], |a, b| {
        used.score(b.command, None)
            .partial_cmp(&used.score(a.command, None))
            .unwrap_or(Ordering::Equal)
            .then_with(|| natural(&a.command.display_name, &b.command.display_name))
    })
}

/// The Apps section (`lA`): the apps with a slash command here, and the
/// built-ins' when there are any, found by their own name leading, inside
/// their name, their description leading, inside it, ignoring case; each
/// match by use, then name.
pub fn search_apps<'a>(
    index: &'a CommandIndex,
    place: &Place,
    typed: &str,
    usage: &AppUsage,
    builtins: &[Command],
) -> Vec<&'a App> {
    let query = search_text(typed).to_lowercase();
    let mut apps = index.apps(place, &[command_type::CHAT]);
    if builtins.iter().any(|c| c.kind == command_type::CHAT) {
        apps.push(App::discord_built_in());
    }
    let name = |app: &&App| app.app_name.to_lowercase();
    let description = |app: &&App| app.description.to_lowercase();
    buckets(
        apps,
        &[
            &|app| name(app).starts_with(&query),
            &|app| name(app).contains(&query),
            &|app| description(app).starts_with(&query),
            &|app| description(app).contains(&query),
        ],
        |a, b| {
            usage
                .score(b)
                .partial_cmp(&usage.score(a))
                .unwrap_or(Ordering::Equal)
                .then_with(|| natural(&a.app_name, &b.app_name))
        },
    )
}

/// The commands on an app's page (`eA` in 621377).
#[derive(Debug)]
pub struct Page<'a> {
    /// "Recents": those among the commands used most here, by use.
    pub recents: Vec<&'a Command>,
    /// "Use this app", by name.
    pub commands: Vec<&'a Command>,
    /// The same by Discord's popularity rank, ranked first, when there are
    /// two or more and one is ranked: the page then offers Sort By and
    /// starts on it.
    pub popular: Option<Vec<&'a Command>>,
}

/// `application`'s page; the built-ins' for [`App::discord_built_in`].
pub fn page<'a>(
    index: &'a CommandIndex,
    place: &Place,
    application: Snowflake,
    used: &Used,
    builtins: &'a [Command],
) -> Page<'a> {
    let commands = if application == App::discord_built_in().id {
        let mut commands: Vec<&Command> = builtins.iter().filter(|c| c.kind == command_type::CHAT).collect();
        commands.sort_by(|a, b| natural(&a.display_name, &b.display_name));
        commands
    } else {
        index.app_commands(place, application)
    };
    let recents = used.frequently_used(&commands, place.guild);
    let popular = (commands.len() >= 2 && commands.iter().any(|c| c.popularity.is_some())).then(|| {
        let mut popular = commands.clone();
        // Stable: the unranked, and equal ranks, stay by name.
        popular.sort_by_key(|c| (c.popularity.is_none(), c.popularity));
        popular
    });
    Page { recents, commands, popular }
}

/// What Copy Link copies (`V` in 342384, with nothing about the app fetched
/// beyond the index): an activity's page, else the app's install link.
pub fn link(app: &App, me: Snowflake) -> String {
    if app.activity {
        format!("https://discord.com/activities/{}?referrer_id={}", app.id.0, me.0)
    } else {
        format!("https://discord.com/oauth2/authorize?client_id={}", app.id.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::commands::Target;
    use crate::permissions::Permissions;

    const ME: Snowflake = Snowflake(100_000_000_000_000_001);
    const GUILD: Snowflake = Snowflake(100_000_000_000_000_010);
    const CHANNEL: Snowflake = Snowflake(100_000_000_000_000_020);
    const DICE: &str = "200000000000000001";
    const PINGER: &str = "200000000000000002";
    const LAUNCHY: &str = "200000000000000003";
    const SHY: &str = "200000000000000004";
    const MINE: &str = "200000000000000005";

    fn chat(id: &str, app: &str, name: &str, extra: Value) -> Value {
        let mut command = json!({"id": id, "application_id": app, "version": "1", "type": 1, "name": name,
                                 "description": format!("{name} things"), "contexts": [0, 1, 2], "integration_types": [0, 1]});
        command.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        command
    }

    /// Dicey: three slash commands, one ranked, and a user command; Pinger:
    /// only a message command; Launchy: only an activity's launch; Shy: a
    /// slash command this channel denies.
    fn server_index() -> Value {
        json!({
            "applications": [
                {"id": DICE, "name": "Dicey", "description": "Rolls dice", "flags": (1u64 << 30) | (1u64 << 29),
                 "bot": {"id": "300000000000000001", "username": "Dice Bot"}, "icon": "abc"},
                {"id": PINGER, "name": "Pinger", "description": "Pings"},
                {"id": LAUNCHY, "name": "Launchy", "description": "Plays", "embedded_surfaces": [0]},
                {"id": SHY, "name": "Shy", "description": "Hides",
                 "permissions": {"channels": {"100000000000000020": false}}},
            ],
            "application_commands": [
                chat("400000000000000001", DICE, "roll", json!({"global_popularity_rank": 3})),
                chat("400000000000000002", DICE, "remind", json!({"options": [{"type": 3, "name": "when", "description": "When"}]})),
                chat("400000000000000003", DICE, "config", json!({"options": [
                    {"type": 1, "name": "show", "description": "Show"},
                    {"type": 1, "name": "reset", "description": "Reset"},
                ]})),
                {"id": "400000000000000004", "application_id": DICE, "version": "1", "type": 2, "name": "Inspect", "description": ""},
                {"id": "400000000000000005", "application_id": PINGER, "version": "1", "type": 3, "name": "Ping it", "description": ""},
                {"id": "400000000000000006", "application_id": LAUNCHY, "version": "1", "type": 4, "name": "launch", "description": ""},
                chat("400000000000000007", SHY, "hide", json!({})),
            ]
        })
    }

    fn user_index() -> Value {
        json!({
            "applications": [{"id": MINE, "name": "Translator", "description": "Translate text"}],
            "application_commands": [chat("400000000000000008", MINE, "translate", json!({"integration_types": [1]}))],
        })
    }

    fn place(guild: Option<Snowflake>) -> Place {
        Place {
            channel: CHANNEL,
            parent: None,
            guild,
            recipient: None,
            is_thread: false,
            me: ME,
            roles: vec![],
            permissions: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::USE_APPLICATION_COMMANDS,
            base_access: true,
            can_send: true,
            allow_nsfw: false,
        }
    }

    fn index() -> CommandIndex {
        let mut index = CommandIndex::default();
        let now = std::time::Instant::now();
        assert!(index.wants(Target::Guild(GUILD), now) && index.wants(Target::User, now));
        index.loaded(Target::Guild(GUILD), &server_index(), ME);
        index.loaded(Target::User, &user_index(), ME);
        index
    }

    fn ids(apps: &[&App]) -> Vec<String> {
        apps.iter().map(|a| a.id.0.to_string()).collect()
    }

    fn names<'a>(hits: &[Hit<'a>]) -> Vec<&'a str> {
        hits.iter().map(|h| h.command.display_name.as_str()).collect()
    }

    fn usage(uses: &[(&str, f64, u64)]) -> AppUsage {
        AppUsage {
            scores: uses.iter().map(|(k, s, _)| (k.to_string(), *s)).collect(),
            latest: uses.iter().map(|(k, _, at)| (k.to_string(), *at)).collect(),
        }
    }

    fn used(scores: &[(&str, f64)]) -> Used {
        Used {
            scores: scores.iter().map(|(k, s)| (k.to_string(), *s)).collect(),
            top: scores.iter().map(|(k, _)| k.to_string()).collect(),
        }
    }

    #[test]
    fn the_apps_with_a_slash_command_here_and_the_servers_own() {
        let index = index();
        let here = place(Some(GUILD));
        assert_eq!(
            ids(&index.apps(&here, &[command_type::CHAT])),
            [DICE, MINE],
            "by the bot's name: Dice Bot, Translator"
        );
        assert_eq!(ids(&index.server_apps(&here, &[command_type::CHAT])), [DICE]);
        assert!(index.server_apps(&place(None), &[command_type::CHAT]).is_empty(), "none in a DM");
        let dice = index.app(&here, Snowflake(DICE.parse().unwrap())).unwrap();
        assert_eq!((dice.name.as_str(), dice.app_name.as_str()), ("Dice Bot", "Dicey"));
        assert!(dice.partner() && dice.promoted());
        let names: Vec<&str> = index.app_commands(&here, dice.id).iter().map(|c| c.display_name.as_str()).collect();
        assert_eq!(names, ["config reset", "config show", "remind", "roll"]);
    }

    #[test]
    fn recents_go_by_use_with_the_app_used_last_first() {
        let index = index();
        let apps = index.apps(&place(Some(GUILD)), &[command_type::CHAT]);
        // Dicey scores more, Translator was used last.
        let order = order_apps(apps.clone(), &usage(&[(DICE, 200.0, 1_000), (MINE, 100.0, 2_000)]));
        assert_eq!(ids(&order), [MINE, DICE]);
        let order = order_apps(apps.clone(), &usage(&[(DICE, 100.0, 2_000), (MINE, 200.0, 1_000)]));
        assert_eq!(ids(&order), [DICE, MINE]);
        // Equal scores go by the listed name; none used leaves the first first.
        let order = order_apps(apps.clone(), &AppUsage::default());
        assert_eq!(ids(&order), [DICE, MINE]);
        let order = order_apps(apps.into_iter().rev().collect(), &AppUsage::default());
        assert_eq!(ids(&order), [MINE, DICE], "the first given, ahead of the name order");
    }

    #[test]
    fn the_home_lists_this_servers_apps_only_in_a_server() {
        let index = index();
        let home = home(&index, &place(Some(GUILD)), &AppUsage::default());
        assert_eq!(ids(&home.recents), [DICE, MINE]);
        assert_eq!(ids(home.server.as_deref().unwrap()), [DICE]);
        assert!(!home.empty());
        let dm = super::home(&index, &place(None), &AppUsage::default());
        assert!(dm.server.is_none());
        assert_eq!(ids(&dm.recents), [MINE]);
        let none = CommandIndex::default();
        assert!(super::home(&none, &place(Some(GUILD)), &AppUsage::default()).empty());
    }

    #[test]
    fn commands_are_found_bucket_by_bucket_as_typed() {
        let index = index();
        let here = place(Some(GUILD));
        let nobody = Used::default();
        // A name leading, then inside one.
        assert_eq!(names(&search_commands(&index, &here, "re", &nobody, &[])), ["remind", "config reset"]);
        assert_eq!(names(&search_commands(&index, &here, "/ro", &nobody, &[])), ["roll"], "a leading / goes");
        assert!(search_commands(&index, &here, "Ro", &nobody, &[]).is_empty(), "case counts");
        // A name's words.
        assert_eq!(names(&search_commands(&index, &here, "config s", &nobody, &[])), ["config show"]);
        assert_eq!(names(&search_commands(&index, &here, "o", &nobody, &[]))[..2], ["config reset", "config show"]);
        assert_eq!(names(&search_commands(&index, &here, "how", &nobody, &[])), ["config show"]);
        // An option's name leading, then inside one.
        assert_eq!(names(&search_commands(&index, &here, "remind w", &nobody, &[])), ["remind"]);
        assert_eq!(names(&search_commands(&index, &here, "he", &nobody, &[])), ["remind"]);
        // Not a denied command, nor another kind's.
        assert!(search_commands(&index, &here, "hide", &nobody, &[]).is_empty());
        assert!(search_commands(&index, &here, "Inspect", &nobody, &[]).is_empty());
    }

    #[test]
    fn equal_matches_go_by_use_without_the_server() {
        let index = index();
        let here = place(Some(GUILD));
        let found = search_commands(&index, &here, "r", &used(&[("400000000000000001", 2.0)]), &[]);
        assert_eq!(names(&found), ["roll", "remind", "config reset", "translate"]);
        // A use filed under the server doesn't count here.
        let mut remind = Command::made_up("400000000000000002", command_type::CHAT, 1, Some("100000000000000010"));
        remind.display_name = "remind".into();
        let key = crate::commands::usage_key(&remind, Some(GUILD));
        assert_eq!(key, "400000000000000002:100000000000000010");
        let found = search_commands(&index, &here, "r", &used(&[(&key, 9.0)]), &[]);
        assert_eq!(names(&found), ["remind", "roll", "config reset", "translate"], "by name, as nothing scores");
    }

    #[test]
    fn built_ins_are_found_under_their_own_app() {
        let index = index();
        let here = place(Some(GUILD));
        let mut shrug = Command::made_up("-1", command_type::CHAT, 0, None);
        (shrug.name, shrug.display_name) = ("shrug".into(), "shrug".into());
        let builtins = [shrug];
        let found = search_commands(&index, &here, "shr", &Used::default(), &builtins);
        assert_eq!(names(&found), ["shrug"]);
        assert_eq!(found[0].app.app_name, "Discord Built-In");
        let apps = search_apps(&index, &here, "built", &AppUsage::default(), &builtins);
        assert_eq!(apps.iter().map(|a| a.app_name.as_str()).collect::<Vec<_>>(), ["Discord Built-In"]);
        assert!(search_apps(&index, &here, "built", &AppUsage::default(), &[]).is_empty());
        let page = page(&index, &here, App::discord_built_in().id, &Used::default(), &builtins);
        assert_eq!(page.commands.len(), 1);
        assert_eq!(app_key(App::discord_built_in()), "-1");
    }

    #[test]
    fn apps_are_found_by_their_own_name_then_description() {
        let index = index();
        let here = place(Some(GUILD));
        let found = |q| ids(&search_apps(&index, &here, q, &AppUsage::default(), &[]));
        assert_eq!(found("DICE"), [DICE], "the app's own name, any case");
        assert!(found("dice bot").is_empty(), "not the bot's");
        assert_eq!(found("r"), [MINE, DICE], "inside a name before a description leading");
        assert_eq!(found("text"), [MINE]);
        assert!(found("ping").is_empty(), "an app with no slash command");
        assert!(found("launch").is_empty(), "nor one that only launches");
        let by_use = ids(&search_apps(&index, &here, "", &usage(&[(DICE, 1.0, 0), (MINE, 5.0, 0)]), &[]));
        assert_eq!(by_use, [MINE, DICE]);
    }

    #[test]
    fn an_app_page_lists_recents_and_both_orders() {
        let index = index();
        let here = place(Some(GUILD));
        let dice = Snowflake(DICE.parse().unwrap());
        let page = page(&index, &here, dice, &used(&[("400000000000000002", 3.0), ("400000000000000008", 9.0)]), &[]);
        let names = |c: &[&Command]| c.iter().map(|c| c.display_name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&page.recents), ["remind"], "this app's only");
        assert_eq!(names(&page.commands), ["config reset", "config show", "remind", "roll"]);
        assert_eq!(names(page.popular.as_deref().unwrap()), ["roll", "config reset", "config show", "remind"]);
        // Another server's use of a server's own command doesn't count.
        let elsewhere = super::page(&index, &here, dice, &used(&[("400000000000000002:5", 3.0)]), &[]);
        assert!(elsewhere.recents.is_empty());
        let mine = super::page(&index, &here, Snowflake(MINE.parse().unwrap()), &Used::default(), &[]);
        assert!(mine.popular.is_none(), "one command");
        let mut index = index;
        let mut unranked = server_index();
        unranked["application_commands"][0].as_object_mut().unwrap().remove("global_popularity_rank");
        index.loaded(Target::Guild(GUILD), &unranked, ME);
        assert!(super::page(&index, &here, dice, &Used::default(), &[]).popular.is_none(), "none ranked");
    }

    #[test]
    fn a_page_keeps_at_most_five_recents() {
        let index = index();
        let here = place(Some(GUILD));
        let mut many = Vec::new();
        for n in 0..7 {
            let mut command = Command::made_up(&format!("-{}", n + 1), command_type::CHAT, 0, None);
            command.display_name = format!("b{n}");
            many.push(command);
        }
        let scores: Vec<(String, f64)> = (1..=7).map(|n| (format!("-{n}"), n as f64)).collect();
        let scores: Vec<(&str, f64)> = scores.iter().map(|(k, s)| (k.as_str(), *s)).collect();
        let page = page(&index, &here, App::discord_built_in().id, &used(&scores), &many);
        let names: Vec<&str> = page.recents.iter().map(|c| c.display_name.as_str()).collect();
        assert_eq!(names, ["b6", "b5", "b4", "b3", "b2"]);
    }

    #[test]
    fn a_run_from_the_launcher_says_where_it_came_from() {
        let run = |from| crate::commands::Run {
            application: Snowflake(1),
            guild: None,
            channel: CHANNEL,
            session: "s",
            data: json!({}),
            nonce: Snowflake(2),
            from,
        };
        let body = run(filled_in(Origin::Chat)).body(&[]);
        assert_eq!((body["analytics_location"].as_str(), body["source"].as_str()), (Some("slash_ui"), Some("TEXT")));
        assert!(body.get("section_name").is_none() && body.get("guild_id").is_none());
        let body = run(filled_in(Origin::Launcher)).body(&[]);
        assert_eq!(body["analytics_location"], "app_launcher");
        let body = run(sent(Some(Section::RecentApps))).body(&[]);
        assert_eq!(body["analytics_location"], "app_launcher_application_view");
        assert_eq!((body["section_name"].as_str(), body["source"].as_str()), (Some("recent_apps"), Some("TEXT")));
        let body = run(RunFrom::CHAT).body(&[]);
        assert!(body.get("section_name").is_none() && body.get("source").is_none());
        assert!(run(sent(None)).autocomplete_body().get("source").is_none());
    }

    #[test]
    fn links_and_help() {
        let index = index();
        let dice = index.app(&place(Some(GUILD)), Snowflake(DICE.parse().unwrap())).unwrap();
        assert_eq!(link(dice, ME), "https://discord.com/oauth2/authorize?client_id=200000000000000001");
        let mut activity = dice.clone();
        activity.activity = true;
        assert_eq!(
            link(&activity, ME),
            "https://discord.com/activities/200000000000000001?referrer_id=100000000000000001"
        );
        assert!(help_url("en-US").contains("/hc/en-us/articles/"));
    }
}
