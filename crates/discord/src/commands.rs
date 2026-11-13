//! Application commands, as the official client offers them: the index it
//! fetches and keeps (`ApplicationCommandIndexStore`, 264322), which commands
//! it lets us run where (`zl` in 46477), how it finds them for what was typed
//! after `/` (`er` and `ea` in 264322), and what running one sends (`A` in
//! 545152).
//!
//! The index is Discord's list of the apps in a server (or a DM with a bot)
//! and the apps installed to the account, with their commands. Neucord keeps
//! the commands it can run as slash commands (type 1); the user and message
//! commands of the Apps menus are the same index.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::builtins::{BuiltIn, Input, Integration};
use crate::gates::Gates;
use crate::limits;
use crate::limits::Limits;
use crate::permissions::{self, Permissions};
use crate::settings::Settings;
use crate::uploads::Sent;
use crate::verification::Account;
use crate::{Channel, ChannelKind, Guild, Snowflake};

/// `ApplicationCommandOptionType` in 155718.
pub mod option_type {
    pub const SUB_COMMAND: u8 = 1;
    pub const SUB_COMMAND_GROUP: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTEGER: u8 = 4;
    pub const BOOLEAN: u8 = 5;
    pub const USER: u8 = 6;
    pub const CHANNEL: u8 = 7;
    pub const ROLE: u8 = 8;
    pub const MENTIONABLE: u8 = 9;
    pub const NUMBER: u8 = 10;
    pub const ATTACHMENT: u8 = 11;
}

/// `ApplicationCommandType` in 155718.
pub mod command_type {
    pub const CHAT: u8 = 1;
    pub const USER: u8 = 2;
    pub const MESSAGE: u8 = 3;
    /// An activity's launch command (878014).
    pub const PRIMARY_ENTRY_POINT: u8 = 4;
}

/// `APPLICATION_COMMAND` and `APPLICATION_COMMAND_AUTOCOMPLETE` in 127258's
/// interaction types.
const APPLICATION_COMMAND: u8 = 2;
const APPLICATION_COMMAND_AUTOCOMPLETE: u8 = 4;

/// `ApplicationIntegrationType` (485845): how an app was installed.
const GUILD_INSTALL: u64 = 0;
const USER_INSTALL: u64 = 1;

/// `ApplicationCommandContext`'s places in 155718: a server, a DM with the
/// app's bot, any other DM or group DM.
const GUILD: u64 = 0;
const BOT_DM: u64 = 1;
const PRIVATE_CHANNEL: u64 = 2;

/// How long a failed fetch waits before the next (`retryAfter` in 264322).
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// The most commands the suggestions list (`limit` in 496533).
pub const SUGGESTIONS: usize = 20;

/// What a request for the index asks: the apps and commands of a server, of a
/// DM with a bot (by the DM's channel), or of the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Guild(Snowflake),
    Channel(Snowflake),
    User,
}

/// Which index holds a place's apps: its server's, else its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Context(Snowflake),
    User,
}

impl Target {
    fn key(self) -> Key {
        match self {
            Target::Guild(id) | Target::Channel(id) => Key::Context(id),
            Target::User => Key::User,
        }
    }
}

/// An app in an index (`descriptor` in 264322, of what `q` keeps:
/// `description, icon, id, name, bot, flags, embedded_surfaces`).
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub id: Snowflake,
    /// The bot's name, else the app's: what the suggestions show.
    pub name: String,
    /// The app's own name, which the launcher shows and searches.
    pub app_name: String,
    pub description: String,
    pub icon: Option<String>,
    /// The bot user the app's messages come from.
    pub bot: Option<Snowflake>,
    /// That bot's avatar, when the index gives the bot whole.
    pub bot_avatar: Option<String>,
    /// `ApplicationFlags`.
    pub flags: u64,
    /// It runs as an activity: `embedded_surfaces` has `MAIN` (0; `W` in
    /// 878014).
    pub activity: bool,
    /// Who may use the app, where the server limited it.
    permissions: Vec<Rule>,
}

/// The section built-ins are listed under (`G` in 12115): no bot, no rules,
/// and a description no text can be found in.
static BUILT_IN: LazyLock<App> = LazyLock::new(|| App {
    id: Snowflake(0),
    name: "Built-In".into(),
    app_name: "Built-In".into(),
    description: String::new(),
    icon: None,
    bot: None,
    bot_avatar: None,
    flags: 0,
    activity: false,
    permissions: Vec::new(),
});

impl App {
    /// `ApplicationFlags` (`PROMOTED` 1 << 29, `PARTNER` 1 << 30).
    const PROMOTED: u64 = 1 << 29;
    const PARTNER: u64 = 1 << 30;

    pub fn partner(&self) -> bool {
        self.flags & Self::PARTNER != 0
    }

    pub fn promoted(&self) -> bool {
        self.flags & Self::PROMOTED != 0
    }

    /// What built-ins are listed as.
    pub fn built_in() -> &'static App {
        &BUILT_IN
    }
}

/// One allow or deny of a command or app (`ec` in 264322).
#[derive(Clone, Debug, PartialEq)]
struct Rule {
    kind: RuleKind,
    id: Snowflake,
    permission: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleKind {
    Role = 1,
    User = 2,
    Channel = 3,
}

/// `Rule`s from the index's `permissions` object: the account's own, then
/// each channel's, then each role's.
fn rules(permissions: &Value, me: Snowflake) -> Vec<Rule> {
    let mut rules = Vec::new();
    if let Some(permission) = permissions["user"].as_bool() {
        rules.push(Rule { kind: RuleKind::User, id: me, permission });
    }
    for (kind, key) in [(RuleKind::Channel, "channels"), (RuleKind::Role, "roles")] {
        for (id, permission) in permissions[key].as_object().into_iter().flatten() {
            if let (Ok(id), Some(permission)) = (id.parse(), permission.as_bool()) {
                rules.push(Rule { kind, id: Snowflake(id), permission });
            }
        }
    }
    rules
}

/// The rules as the client sends them inside a command (`ec` again).
fn rules_json(rules: &[Rule]) -> Value {
    Value::Array(
        rules
            .iter()
            .map(|r| json!({ "type": r.kind as u8, "id": r.id.to_string(), "permission": r.permission }))
            .collect(),
    )
}

fn rule(rules: &[Rule], kind: RuleKind, id: Snowflake) -> Option<&Rule> {
    rules.iter().find(|r| r.kind == kind && r.id == id)
}

/// A choice an option offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// What the user sees and types.
    pub name: String,
    /// What is sent: text, or a number.
    pub value: Value,
}

/// A command's option, as the form needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandOption {
    pub name: String,
    pub display_name: String,
    /// The name the server localized, which searching also reads.
    pub server_name: Option<String>,
    pub description: String,
    pub kind: u8,
    pub required: bool,
    pub choices: Option<Vec<Choice>>,
    pub autocomplete: bool,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    /// The kinds of channel a `CHANNEL` option takes (`channelTypes`).
    pub channel_types: Option<Vec<u64>>,
    /// What an `ATTACHMENT` option takes: extensions with their dot, or
    /// `image`, `video`, `audio`; empty for anything (`p` in 168186 reads it
    /// only for that type).
    pub file_types: Vec<String>,
}

/// A step from a command to its sub-command.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub name: String,
    pub kind: u8,
    pub display_name: String,
}

/// A command Neucord can offer: a root command, or a sub-command of one with
/// the options it takes (`Lz` and `p` in 168186).
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// The root's id, then each step's name, joined by NUL.
    pub id: String,
    pub application: Snowflake,
    pub kind: u8,
    pub version: Option<Value>,
    pub guild: Option<Value>,
    /// `/name sub-command`, in English.
    pub name: String,
    /// The same as the account's language has it.
    pub display_name: String,
    pub description: String,
    pub display_description: String,
    pub options: Vec<CommandOption>,
    pub path: Vec<Step>,
    /// The root's place among Discord's most used (`global_popularity_rank`).
    pub popularity: Option<u64>,
    /// The client's own command this is, which runs without an app.
    pub builtin: Option<BuiltIn>,
    /// The root as the client sends it back with a run: `application_command`.
    root: Value,
    root_id: Value,
    root_name: String,
    nsfw: bool,
    dm_permission: Option<bool>,
    contexts: Option<Vec<u64>>,
    integration_types: Option<Vec<u64>>,
    default_member_permissions: Option<Permissions>,
    permissions: Vec<Rule>,
}

/// The apps of an index and their commands (`result.sections`).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub app: App,
    pub commands: Vec<Command>,
}

/// A fetched index.
#[derive(Clone, Debug, PartialEq)]
struct Loaded {
    /// `version`, or none when the response had none.
    version: Option<String>,
    sections: Vec<Section>,
}

/// What Discord says the index is at (`serverVersion`).
#[derive(Clone, Debug, PartialEq)]
enum Server {
    /// Whatever was fetched is current until something says otherwise.
    Current,
    /// Unknown: fetch again.
    Stale,
    Version(String),
}

#[derive(Clone, Debug)]
struct Slot {
    server: Server,
    loaded: Option<Loaded>,
    fetching: bool,
    retry_after: Option<Instant>,
}

impl Slot {
    /// `el` in 264322: what was fetched isn't what Discord has, and no
    /// fetch is under way or waiting out a failure.
    fn wants_fetch(&self, now: Instant) -> bool {
        let same = self.loaded.as_ref().is_some_and(|l| match (&l.version, &self.server) {
            (None, Server::Current) => true,
            (Some(v), Server::Version(s)) => v == s,
            _ => false,
        });
        !same && !self.fetching && self.retry_after.is_none_or(|t| now >= t)
    }
}

/// What the client keeps of the application command indexes
/// (`ApplicationCommandIndexStore`): for a place, its server's (or DM's) apps
/// and the account's own, each fetched when something first needs it and
/// again when Discord says it changed.
#[derive(Debug, Default)]
pub struct CommandIndex {
    slots: HashMap<Key, Slot>,
}

impl CommandIndex {
    /// Whether to fetch `target` now (`APPLICATION_COMMAND_INDEX_FETCH_REQUEST`
    /// and `z`): it hasn't been, or has gone stale. Say so before sending, so
    /// that the same fetch isn't sent twice.
    pub fn wants(&mut self, target: Target, now: Instant) -> bool {
        let slot = self.slots.entry(target.key()).or_insert(Slot {
            server: Server::Stale,
            loaded: None,
            fetching: false,
            retry_after: None,
        });
        let wants = slot.wants_fetch(now);
        if wants {
            slot.fetching = true;
        }
        wants
    }

    /// The fetch succeeded (`q`). `me` is the account, whose rules the index
    /// lists as `user`.
    pub fn loaded(&mut self, target: Target, index: &Value, me: Snowflake) {
        let loaded = parse_index(index, me);
        let slot = self.slots.entry(target.key()).or_insert(Slot {
            server: Server::Stale,
            loaded: None,
            fetching: false,
            retry_after: None,
        });
        slot.server = loaded.version.clone().map_or(Server::Current, Server::Version);
        slot.loaded = Some(loaded);
        slot.fetching = false;
    }

    /// The fetch failed (`APPLICATION_COMMAND_INDEX_FETCH_FAILURE`): not again
    /// for five seconds.
    pub fn failed(&mut self, target: Target, now: Instant) {
        if let Some(slot) = self.slots.get_mut(&target.key()) {
            slot.fetching = false;
            slot.retry_after = Some(now + RETRY_AFTER);
        }
    }

    /// READY: a new session, which every index is out of date for
    /// (`CONNECTION_OPEN`). What was fetched stays until the new one arrives.
    pub fn session_started(&mut self) {
        for slot in self.slots.values_mut() {
            slot.server = Server::Stale;
        }
    }

    /// A server's apps or commands changed (`GUILD_APPLICATION_COMMAND_INDEX_UPDATE`).
    /// Returns the bots it had, whose DMs' indexes are out of date as well.
    pub fn guild_changed(&mut self, guild: Snowflake, version: Option<String>) -> Vec<Snowflake> {
        let Some(slot) = self.slots.get_mut(&Key::Context(guild)) else { return Vec::new() };
        slot.server = version.map_or(Server::Stale, Server::Version);
        let bots = slot.loaded.iter().flat_map(|l| &l.sections).filter_map(|s| s.app.bot);
        bots.collect()
    }

    /// The DM `channel` with a bot is out of date.
    pub fn channel_stale(&mut self, channel: Snowflake) {
        if let Some(slot) = self.slots.get_mut(&Key::Context(channel)) {
            slot.server = Server::Stale;
        }
    }

    /// An app was installed to or removed from the account
    /// (`USER_APPLICATION_UPDATE` and `USER_APPLICATION_REMOVE`).
    pub fn user_changed(&mut self) {
        if let Some(slot) = self.slots.get_mut(&Key::User) {
            slot.server = Server::Stale;
        }
    }

    /// Discord refused a command as out of date (`APPLICATION_COMMAND_EXECUTE_BAD_VERSION`):
    /// the indexes that hold the app are stale.
    pub fn out_of_date(&mut self, app: Snowflake, context: Snowflake) {
        for key in [Key::Context(context), Key::User] {
            if let Some(slot) = self.slots.get_mut(&key)
                && slot.loaded.iter().flat_map(|l| &l.sections).any(|s| s.app.id == app)
            {
                slot.server = Server::Stale;
            }
        }
    }

    /// A server or channel is gone.
    pub fn forget(&mut self, context: Snowflake) {
        self.slots.remove(&Key::Context(context));
    }

    /// Logging out.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// Whether any index is being fetched for `place`.
    pub fn loading(&self, place: &Place) -> bool {
        [place.context().map(Key::Context), Some(Key::User)]
            .into_iter()
            .flatten()
            .any(|key| self.slots.get(&key).is_some_and(|s| s.fetching))
    }

    /// What to fetch to list commands for `place` (264322 with `allowFetch`):
    /// its server's or DM's index where that has apps, and the account's,
    /// only where apps' commands are offered. That is the bare `/`'s browser
    /// (`ee` and `et`), and the Apps menus', with nothing `typed`; the typed
    /// list (`query`) asks for the account's index whatever is offered, and
    /// for the server's only with View Channel and Use Application Commands.
    pub fn to_fetch(&mut self, place: &Place, now: Instant, offer: Offer, typed: &str) -> Vec<Target> {
        let mut targets = Vec::new();
        if offer.apps && (typed.is_empty() || place.base_access) && place.has_context_index() {
            let target = place.guild.map_or(Target::Channel(place.channel), Target::Guild);
            if self.wants(target, now) {
                targets.push(target);
            }
        }
        if (offer.apps || !typed.is_empty()) && self.wants(Target::User, now) {
            targets.push(Target::User);
        }
        targets
    }

    fn sections(&self, key: Key) -> &[Section] {
        self.slots.get(&key).and_then(|s| s.loaded.as_ref()).map_or(&[], |l| &l.sections)
    }

    /// The commands of `kinds` that may be used in `place`, each with the app
    /// it is listed under (`er` in 264322, through `zl`): the server's (or
    /// DM's) index only with View Channel and Use Application Commands, and
    /// an app the account has too under the account's listing, with its
    /// commands first and then those only the place's index has.
    fn usable(&self, place: &Place, kinds: &[u8]) -> Vec<(&App, &Command)> {
        let context = if place.base_access && place.has_context_index() {
            place.context().map_or(&[][..], |c| self.sections(Key::Context(c)))
        } else {
            &[]
        };
        let user = self.sections(Key::User);
        let mut apps: Vec<Snowflake> = context.iter().chain(user).map(|s| s.app.id).collect();
        apps.sort();
        apps.dedup();
        let mut usable = Vec::new();
        for app in apps {
            let (in_context, in_user) =
                (context.iter().find(|s| s.app.id == app), user.iter().find(|s| s.app.id == app));
            let (descriptor, commands): (&App, Vec<&Command>) = match (in_context, in_user) {
                (Some(c), Some(u)) => {
                    let rest = c.commands.iter().filter(|c| !u.commands.iter().any(|u| u.id == c.id));
                    (&u.app, u.commands.iter().chain(rest).collect())
                }
                (Some(s), None) | (None, Some(s)) => (&s.app, s.commands.iter().collect()),
                (None, None) => continue,
            };
            let allowed_for_user = descriptor_allows(&descriptor.permissions, place);
            let allowed_in_channel = channel_allows(&descriptor.permissions, place);
            let installed = Installed { guild: in_context.is_some(), user: in_user.is_some() };
            for command in commands {
                let allowed = kinds.contains(&command.kind)
                    && place.access(
                        command,
                        command.kind,
                        installed,
                        allowed_for_user,
                        allowed_in_channel,
                        descriptor.bot,
                    ) == Access::Allowed;
                if allowed {
                    usable.push((descriptor, command));
                }
            }
        }
        usable
    }

    /// What `query` finds in `place`, best first: the apps' commands where
    /// they are offered, and the built-ins it names.
    pub fn query<'a>(&'a self, place: &Place, query: Query<'_, 'a>) -> Vec<Found<'a>> {
        let text = query.text.to_lowercase();
        let words: Vec<&str> = text.split(' ').collect();
        let apps = if query.offer.apps { self.usable(place, &[query.kind]) } else { Vec::new() };
        let builtins = query.builtins.iter().filter(|c| c.kind == query.kind).map(|c| (App::built_in(), c));
        let mut found: Vec<Found<'a>> = apps
            .into_iter()
            .chain(builtins)
            .filter_map(|(app, command)| score(command, app, &text, &words).map(|score| Found { command, app, score }))
            .collect();
        let used = query.used.filter(|_| !query.text.is_empty());
        found.sort_by(|a, b| order(a, b, used, place.guild));
        found
    }

    /// The apps' commands of `kind` that may be run in `place`, matching
    /// `text` (the words after `/`), best first.
    pub fn find(&self, place: &Place, kind: u8, text: &str) -> Vec<Found<'_>> {
        self.query(place, Query { kind, text, offer: Offer::ALL, builtins: &[], used: None })
    }

    /// The command `id` of `kind` that may be used in `place`, with its app.
    pub fn get(&self, place: &Place, kind: u8, id: &str) -> Option<(&Command, &App)> {
        self.find(place, kind, "").into_iter().find(|f| f.command.id == id).map(|f| (f.command, f.app))
    }

    /// The app listed for `application` in `place`, wherever it is installed.
    pub fn app(&self, place: &Place, application: Snowflake) -> Option<&App> {
        let contexts = place.context().map(Key::Context);
        [Some(Key::User), contexts]
            .into_iter()
            .flatten()
            .find_map(|key| self.sections(key).iter().find(|s| s.app.id == application).map(|s| &s.app))
    }
}

/// What a list of commands asks for (the filters and query of `er` in
/// 264322).
#[derive(Clone, Copy, Debug)]
pub struct Query<'t, 'b> {
    pub kind: u8,
    /// The words after `/`, as [`query_of`] leaves them; empty for all.
    pub text: &'t str,
    pub offer: Offer,
    /// The built-ins offered here, listed beside the apps' commands.
    pub builtins: &'b [Command],
    /// How much each command is used, which orders equal matches once
    /// something is typed.
    pub used: Option<&'t Used>,
}

/// The order of every list of commands: the better match, then, with
/// something typed, the one used more (`er` in 264322 sorts by use only
/// with a `scoreMethod`, which every caller passes only for a query), then
/// the name, then an app's command before a built-in of the same name (the
/// client sorts each section, then lists Built-In last), then the app's
/// name.
fn order(a: &Found<'_>, b: &Found<'_>, used: Option<&Used>, guild: Option<Snowflake>) -> Ordering {
    let by_use = || match used {
        Some(used) => {
            used.score(b.command, guild).partial_cmp(&used.score(a.command, guild)).unwrap_or(Ordering::Equal)
        }
        None => Ordering::Equal,
    };
    a.score
        .cmp(&b.score)
        .then_with(by_use)
        .then_with(|| natural(&a.command.display_name, &b.command.display_name))
        .then_with(|| a.command.is_built_in().cmp(&b.command.is_built_in()))
        .then_with(|| natural(&a.app.name, &b.app.name))
}

/// How many commands a Frequently Used section lists (`XC` in 264322).
pub const FREQUENTLY_USED: usize = 5;

/// What a command's use is filed under (`_` in 26909): a built-in by its
/// (negative) id, a command registered to a server and run in one as
/// `"<id>:<server>"`, any other by its id. A sub-command's id holds its path,
/// NUL-separated, as `Lz` (168186) makes it.
pub fn usage_key(command: &Command, guild: Option<Snowflake>) -> String {
    // `0 > Number(id)`: a NUL-joined id is NaN, never negative.
    let built_in = command.id.strip_prefix('-').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let registered = command.guild.as_ref().is_some_and(|g| !g.is_null());
    match guild {
        Some(guild) if !built_in && registered => format!("{}:{}", command.id, guild.0),
        _ => command.id.clone(),
    }
}

/// Of the keys used most, those that count in `guild` (`RG` in 26909): a
/// server's own commands only there, the rest anywhere, without the server.
pub fn place_keys(top: &[String], guild: Option<Snowflake>) -> Vec<String> {
    top.iter()
        .filter(|key| {
            !key.contains(':') || guild.is_some_and(|g| key.split(':').nth(1) == Some(g.0.to_string().as_str()))
        })
        .map(|key| key.split(':').next().unwrap_or_default().to_owned())
        .collect()
}

/// How much the account uses each command (`ApplicationCommandFrecencyStore`).
#[derive(Clone, Debug, Default)]
pub struct Used {
    /// `getScore`, by [`usage_key`].
    pub scores: HashMap<String, f64>,
    /// `frequently`: the 100 keys with the best frecency, best first.
    pub top: Vec<String>,
}

impl Used {
    /// `getScoreWithoutLoadingLatest`: 0 for what was never used.
    pub fn score(&self, command: &Command, guild: Option<Snowflake>) -> f64 {
        self.scores.get(&usage_key(command, guild)).copied().unwrap_or(0.0)
    }

    /// The Frequently Used section (`XC` in 264322): of `listed` (what the
    /// place offers, in the order shown unsearched), those among the keys
    /// used most that count here, by score (not rank), at most
    /// [`FREQUENTLY_USED`].
    pub fn frequently_used<'a>(&self, listed: &[&'a Command], guild: Option<Snowflake>) -> Vec<&'a Command> {
        let keys = place_keys(&self.top, guild);
        let mut frequent: Vec<&Command> = listed.iter().copied().filter(|c| keys.contains(&c.id)).collect();
        frequent.sort_by(|a, b| self.score(b, guild).partial_cmp(&self.score(a, guild)).unwrap_or(Ordering::Equal));
        frequent.truncate(FREQUENTLY_USED);
        frequent
    }
}

/// A command that matches what was typed.
#[derive(Clone, Copy, Debug)]
pub struct Found<'a> {
    pub command: &'a Command,
    pub app: &'a App,
    /// Lower is better.
    pub score: u8,
}

/// Whether a channel's menus have an Apps item (173500): the client's
/// textual channels (`TEXTUAL` in 478437), but not a media thread.
pub fn offers_apps(kind: ChannelKind) -> bool {
    !matches!(
        kind,
        ChannelKind::Category
            | ChannelKind::Forum
            | ChannelKind::Media
            | ChannelKind::MediaThread
            | ChannelKind::Other(_)
    )
}

/// Found commands under their apps, as the Apps menu lists them when nothing
/// is searched for: the apps by name, and each one's commands by name (`ea`
/// in 264322 sorts a section that isn't scored).
pub fn by_app<'a>(found: Vec<Found<'a>>) -> Vec<(&'a App, Vec<&'a Command>)> {
    let mut apps: Vec<(&App, Vec<&Command>)> = Vec::new();
    for Found { command, app, .. } in found {
        match apps.iter_mut().find(|(a, _)| a.id == app.id) {
            Some((_, commands)) => commands.push(command),
            None => apps.push((app, vec![command])),
        }
    }
    for (_, commands) in &mut apps {
        commands.sort_by(|a, b| natural(&a.display_name, &b.display_name));
    }
    apps.sort_by(|(a, _), (b, _)| natural(&a.name, &b.name));
    apps
}

/// `Intl.Collator` with `sensitivity: accent, numeric` (264322's `H`): case
/// doesn't matter and digits count as numbers.
fn natural(a: &str, b: &str) -> Ordering {
    fn chunks(text: &str) -> Vec<(bool, String)> {
        let mut chunks: Vec<(bool, String)> = Vec::new();
        for c in text.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match chunks.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => chunks.push((digit, c.to_string())),
            }
        }
        chunks
    }
    let (a, b) = (chunks(a), chunks(b));
    for ((a_digit, a), (b_digit, b)) in a.iter().zip(&b) {
        let order = if *a_digit && *b_digit {
            let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        } else {
            a.cmp(b)
        };
        if order.is_ne() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// Whether `needle`'s letters appear in order in `hay` (the `fuzzysearch`
/// package the client uses).
fn fuzzy(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|c| hay.any(|h| h == c))
}

/// How well `command` matches `query`, lower being better, or none
/// (`COMMAND_OR_APPLICATION` in `ea`): the best of its own score and its
/// app's.
fn score(command: &Command, app: &App, query: &str, words: &[&str]) -> Option<u8> {
    let app_score = {
        let name = app.name.to_lowercase();
        if name.starts_with(query) {
            Some(5)
        } else if name.contains(query) {
            Some(6)
        } else if app.description.to_lowercase().contains(query) {
            Some(8)
        } else if fuzzy(query, &name) {
            Some(11)
        } else {
            None
        }
    };
    let first = words[0];
    let rest = words[1..].join(" ");
    let own = command_score(command, query, first, &rest);
    match (own, app_score) {
        (None, s) => s,
        (n, Some(s)) if n.is_some_and(|n| s < n) => Some(s),
        (n, _) => n,
    }
}

fn command_score(command: &Command, query: &str, first: &str, rest: &str) -> Option<u8> {
    let name = command.name.to_lowercase();
    let display = command.display_name.to_lowercase();
    if name.starts_with(query) || display.starts_with(query) {
        return Some(0);
    }
    let tail = |name: &str| name.split(' ').skip(1).collect::<Vec<_>>().join(" ");
    if (name.starts_with(first) && tail(&name).starts_with(rest))
        || (display.starts_with(first) && tail(&display).starts_with(rest))
    {
        return Some(1);
    }
    if name.contains(query) || display.contains(query) {
        return Some(2);
    }
    let mut in_option_names = false;
    for option in &command.options {
        let local = option.server_name.as_deref();
        let leads = |text: &str| {
            text.starts_with(query)
                || format!("{name} {text}").starts_with(query)
                || format!("{display} {text}").starts_with(query)
        };
        if leads(&option.name) || local.is_some_and(leads) {
            return Some(3);
        }
        if option.name.contains(query) || local.is_some_and(|l| l.contains(query)) {
            in_option_names = true;
        }
    }
    if in_option_names {
        return Some(4);
    }
    let (description, display_description) =
        (command.description.to_lowercase(), command.display_description.to_lowercase());
    if description.contains(query) || display_description.contains(query) {
        return Some(7);
    }
    if fuzzy(query, &name) || fuzzy(query, &display) {
        return Some(9);
    }
    if command
        .options
        .iter()
        .any(|o| fuzzy(query, &o.name) || o.server_name.as_deref().is_some_and(|l| fuzzy(query, l)))
    {
        return Some(10);
    }
    if fuzzy(query, &description) || fuzzy(query, &display_description) {
        return Some(12);
    }
    None
}

/// The words after `/` the suggestions are looked up by, and whether the
/// typing has moved on from the command's name (`Yn` in 664929): the text
/// stops at the first option (`name:`) and after three words.
pub fn query_of(typed: &str) -> (String, bool) {
    let mut text = typed;
    let mut terminated = false;
    if let Some(colon) = typed.find(':') {
        match typed[..colon].rfind(' ') {
            Some(space) => {
                text = &typed[..space];
                terminated = true;
            }
            None => text = &typed[..colon],
        }
    }
    let mut parts: Vec<&str> = text.splitn(4, ' ').collect();
    // JavaScript's limit drops the rest; three words and the beginning of a fourth.
    if parts.len() > 3 {
        terminated = true;
        parts.pop();
    }
    let mut joined = parts.join(" ");
    if typed.len() > joined.len() || joined.ends_with(' ') {
        terminated = true;
        joined = joined.trim_end().to_owned();
    }
    (joined, terminated)
}

/// Keep the commands the typing has settled on: with `terminated`, those
/// named exactly `text` or starting with it and a space (`496533`).
pub fn settled<'a>(found: Vec<Found<'a>>, text: &str, terminated: bool) -> Vec<Found<'a>> {
    if !terminated {
        return found;
    }
    let (text, spaced) = (text.trim(), format!("{} ", text.trim()));
    found
        .into_iter()
        .filter(|f| f.command.display_name == text || f.command.display_name.starts_with(&spaced))
        .collect()
}

/// Which command typed text names (870748, with no command active): once
/// the name is ended ([`query_of`]'s `terminated`), the one
/// command named exactly so, when nothing else starts with that name and a
/// space.
pub fn typed_command<'a>(found: Vec<Found<'a>>, text: &str, terminated: bool) -> Option<&'a Command> {
    if !terminated {
        return None;
    }
    let text = text.trim();
    let spaced = format!("{text} ");
    let named: Vec<&Command> = found
        .into_iter()
        .map(|f| f.command)
        .filter(|c| c.display_name == text || c.display_name.starts_with(&spaced))
        .collect();
    match named[..] {
        [only] if only.display_name == text => Some(only),
        _ => None,
    }
}

/// When a popup list selects its first row by itself (`focusMode` in
/// 133746's navigator).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// As soon as it shows (no `focusMode`): the suggestions, a pill's
    /// choices and mentions, `/gif`'s and `/sticker`'s results.
    #[default]
    Auto,
    /// Once something is typed (`AUTO_WHEN_FILTERED`): the options list.
    WhenFiltered,
}

/// What Enter or Tab does with the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Enter on a row (`selectResult(index, true)`: `SEND` for the
    /// integrations).
    Send(usize),
    /// Tab on a row (`INSERT`).
    Insert(usize),
    /// Tab with no row selected on a list that waits for typing: its first
    /// row is selected, and nothing else happens.
    Selected,
}

/// Which row of the box's popup is selected (31498 with 133746's
/// navigator), one model for every list it shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    focus: Focus,
    index: Option<usize>,
    /// What the navigator effect depends on: the list's type, its query,
    /// whether it is loading and whether it shows.
    last: Option<(String, String, bool, bool)>,
}

impl Selection {
    /// The popup now shows `count` rows of `list` (its type) for `query`:
    /// when the type, the query, the loading or whether it shows changed,
    /// the first row is selected as `focus` allows (none while loading);
    /// otherwise the selection stays, clamped to the rows.
    pub fn listed(&mut self, list: &str, focus: Focus, query: &str, loading: bool, count: usize) {
        self.focus = focus;
        let shows = loading || count > 0;
        let key = (list.to_owned(), query.to_owned(), loading, shows);
        if self.last.as_ref() != Some(&key) {
            let allowed = focus == Focus::Auto || !query.is_empty();
            self.index = (shows && allowed && count > 0).then_some(0);
            self.last = Some(key);
        } else if let Some(index) = self.index {
            self.index = count.checked_sub(1).map(|last| index.min(last));
        }
    }

    /// Up (`by` -1) or Down (+1), wrapping at either end; with nothing
    /// selected, Down takes the first and Up the last.
    pub fn moved(&mut self, by: i32, count: usize) {
        if count == 0 {
            return;
        }
        self.index = Some(match (self.index, by < 0) {
            (None, false) => 0,
            (None, true) => count - 1,
            (Some(i), false) => (i + 1) % count,
            (Some(i), true) => i.checked_sub(1).unwrap_or(count - 1),
        });
    }

    /// The pointer moved onto a row that isn't selected: none is
    /// (`onResultHover`).
    pub fn hovered(&mut self) {
        self.index = None;
    }

    pub fn selected(&self) -> Option<usize> {
        self.index
    }

    /// Enter or Tab (`onTabOrEnter`): the selected row, if it is one of
    /// `count`; with none, Tab selects the first of a list that waits for
    /// typing. `None` leaves the key to the box.
    pub fn key(&mut self, enter: bool, count: usize) -> Option<Pick> {
        match self.index {
            Some(i) if i < count => Some(if enter { Pick::Send(i) } else { Pick::Insert(i) }),
            Some(_) => None,
            None if !enter && self.focus == Focus::WhenFiltered && count > 0 => {
                self.index = Some(0);
                Some(Pick::Selected)
            }
            None => None,
        }
    }
}

/// What the `/` suggestions may list (`applicationCommands` and `builtIns`
/// in 264322's filters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Apps' commands.
    pub apps: bool,
    pub builtins: BuiltIns,
}

impl Offer {
    /// The filters' defaults, which the Apps menus keep.
    pub const ALL: Offer = Offer { apps: true, builtins: BuiltIns::All };
}

/// Which built-ins are listed (`n` in 210978).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltIns {
    All,
    /// Those that write the message, and the two that search
    /// (`ONLY_TEXT`).
    TextOnly,
}

/// What the message box's `/` offers (`ta` in 496533, and the bare `/`'s
/// browser, `ed` in 989161): while a reply is pending
/// (`canOnlyUseTextCommands`), no app's commands and only the built-ins
/// that write text (`NEW_TEXT_ONLY`).
pub fn offer(replying: bool) -> Offer {
    if replying { Offer { apps: false, builtins: BuiltIns::TextOnly } } else { Offer::ALL }
}

/// The suggestions for `typed`, the text after `/` (`queryResults` of `B`
/// in 496533): the apps' commands as `offer` says and `builtins`, equal
/// matches by `used`, settled on the command's name once typing moved on,
/// at most [`SUGGESTIONS`].
pub fn suggestions<'a>(
    index: &'a CommandIndex,
    place: &Place,
    typed: &str,
    offer: Offer,
    builtins: &'a [Command],
    used: &Used,
) -> Vec<Found<'a>> {
    let (query, terminated) = query_of(typed);
    let query_text = query.as_str();
    let found =
        index.query(place, Query { kind: command_type::CHAT, text: query_text, offer, builtins, used: Some(used) });
    let mut found = settled(found, &query, terminated);
    found.truncate(SUGGESTIONS);
    found
}

/// What decides, apart from the place, whether age-restricted commands are
/// offered (`p` in 240591).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NsfwRules {
    /// In DMs and group DMs: `view_nsfw_commands`, unless the region holds
    /// it back (`S9` in 386171).
    pub in_dms: bool,
    /// A server is age-restricted by its `nsfw_level` (see
    /// [`Guild::age_restricted`]).
    pub by_nsfw_level: bool,
}

impl NsfwRules {
    pub fn new(settings: &Settings, gates: &Gates, limits: &Limits<'_>) -> NsfwRules {
        NsfwRules {
            in_dms: settings.nsfw_commands_in_dms && !gates.nsfw_commands_age_gated,
            by_nsfw_level: limits.servers_by_nsfw_level(),
        }
    }
}

/// Whether listing `typed` (the text after `/`) needs how much commands are
/// used, which the settings proto of type 2 holds: the bare `/`'s browser
/// loads it as it opens (`F` in 56494), a typed list once an app's command
/// matches (`er` in 264322 loads it only then).
pub fn loads_use(index: &CommandIndex, place: &Place, typed: &str, offer: Offer) -> bool {
    typed.is_empty() || (offer.apps && !index.find(place, command_type::CHAT, &query_of(typed).0).is_empty())
}

/// Where a command would run, as far as its availability goes (`Bh` in 240591).
#[derive(Clone, Debug)]
pub struct Place {
    pub channel: Snowflake,
    /// A thread's parent: the permissions are the parent's.
    pub parent: Option<Snowflake>,
    pub guild: Option<Snowflake>,
    /// For a DM, its other person, and whether they are a bot.
    pub recipient: Option<(Snowflake, bool)>,
    pub is_thread: bool,
    pub me: Snowflake,
    pub roles: Vec<Snowflake>,
    pub permissions: Permissions,
    /// View Channel and Use Application Commands, or a DM.
    pub base_access: bool,
    /// Send Messages (in Threads, in a thread), or a DM.
    pub can_send: bool,
    /// Whether NSFW commands may be offered (`p` in 240591): to an adult's
    /// account, in an `nsfw` channel (a thread's parent) or an age-restricted
    /// server, or in a DM where [`NsfwRules::in_dms`] says.
    pub allow_nsfw: bool,
}

impl Place {
    /// Where `channel` is, one of `guild`'s or a DM (`Bh` and `m` in 240591).
    /// A thread takes its parent's permissions, with Send Messages in
    /// Threads in place of Send Messages. A DM has every permission that
    /// counts.
    pub fn of(guild: Option<&Guild>, channel: &Channel, me: &Account, nsfw: NsfwRules, now: i64) -> Place {
        let is_thread = channel.kind.is_thread();
        let base = Place {
            channel: channel.id,
            parent: None,
            guild: None,
            recipient: None,
            is_thread,
            me: me.id,
            roles: Vec::new(),
            permissions: Permissions::NONE,
            base_access: true,
            can_send: true,
            allow_nsfw: false,
        };
        let Some(guild) = guild else {
            let recipient = (channel.kind == ChannelKind::Dm).then(|| channel.recipients.first()).flatten();
            let allow_nsfw = me.nsfw_allowed && nsfw.in_dms;
            return Place { recipient: recipient.map(|u| (u.id, u.bot)), allow_nsfw, ..base };
        };
        let parent = channel.parent_id.filter(|_| is_thread);
        let counted = parent.and_then(|p| guild.channel(p)).unwrap_or(channel);
        let permissions = permissions::in_channel(guild, counted, me, now);
        let send = if is_thread { Permissions::SEND_MESSAGES_IN_THREADS } else { Permissions::SEND_MESSAGES };
        Place {
            guild: Some(guild.id),
            parent,
            roles: guild.my_roles.clone(),
            permissions,
            base_access: permissions.contains(Permissions::VIEW_CHANNEL | Permissions::USE_APPLICATION_COMMANDS),
            can_send: permissions.contains(send),
            allow_nsfw: me.nsfw_allowed && (counted.nsfw || guild.age_restricted(nsfw.by_nsfw_level)),
            ..base
        }
    }

    /// The server or channel whose index lists the apps here, if any
    /// (`es`): a server's channels, and a DM with a bot.
    fn context(&self) -> Option<Snowflake> {
        self.guild.or(self.recipient.filter(|(_, bot)| *bot).map(|_| self.channel))
    }

    fn has_context_index(&self) -> bool {
        self.context().is_some()
    }
}

/// Why a command can't be used, or [`Access::Allowed`] (`CA` in 46477).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Allowed,
    NsfwNotAllowed,
    WrongCommandType,
    ContextNotAllowed,
    MissingBasePermissions,
    ChannelDenied,
    UserDenied,
}

/// Where the app is installed, from the indexes that list it.
#[derive(Clone, Copy)]
struct Installed {
    guild: bool,
    user: bool,
}

/// `we` in 46477: whether the app's own rules allow us, if they say.
fn descriptor_allows(rules: &[Rule], place: &Place) -> Option<bool> {
    let guild = place.guild?;
    if rules.is_empty() {
        return None;
    }
    allows(rules, guild, place.me, &place.roles)
}

/// `_W`: whether the app's own rules allow the channel, if they say.
fn channel_allows(rules: &[Rule], place: &Place) -> Option<bool> {
    let guild = place.guild?;
    if rules.is_empty() {
        return None;
    }
    channel_rule(rules, place, guild)
}

/// A rule for the channel (a thread's parent), else for all channels, which
/// is the server's id minus one.
fn channel_rule(rules: &[Rule], place: &Place, guild: Snowflake) -> Option<bool> {
    let channel = place.parent.filter(|_| place.is_thread).unwrap_or(place.channel);
    rule(rules, RuleKind::Channel, channel)
        .or_else(|| rule(rules, RuleKind::Channel, Snowflake(guild.0.wrapping_sub(1))))
        .map(|r| r.permission)
}

/// `T` in 46477 and in 168186: our own rule, else our roles' (any allow
/// wins; a rule on any role denies), else `@everyone`'s.
fn allows(rules: &[Rule], guild: Snowflake, me: Snowflake, roles: &[Snowflake]) -> Option<bool> {
    if let Some(user) = rule(rules, RuleKind::User, me) {
        return Some(user.permission);
    }
    let mut denied = false;
    for role in roles {
        if let Some(rule) = rule(rules, RuleKind::Role, *role) {
            if rule.permission {
                return Some(true);
            }
            denied = true;
        }
    }
    if denied {
        return Some(false);
    }
    rule(rules, RuleKind::Role, guild).map(|r| r.permission)
}

impl Place {
    /// Whether `command` of `kind` may be used here (`zl`).
    fn access(
        &self,
        command: &Command,
        kind: u8,
        installed: Installed,
        app_allows_user: Option<bool>,
        app_allows_channel: Option<bool>,
        bot: Option<Snowflake>,
    ) -> Access {
        if command.kind != kind {
            return Access::WrongCommandType;
        }
        // The client also treats an app it has classified as adult (`d.A` in
        // 46477, from ApplicationStore) as NSFW; Neucord keeps no
        // applications, so only `nsfw` counts.
        if command.nsfw && !self.allow_nsfw {
            return Access::NsfwNotAllowed;
        }
        let place = match (self.guild, self.recipient) {
            (Some(_), _) => GUILD,
            (None, Some((who, _))) if Some(who) == bot => BOT_DM,
            _ => PRIVATE_CHANNEL,
        };
        match &command.contexts {
            Some(contexts) => {
                if !contexts.contains(&place) {
                    return Access::ContextNotAllowed;
                }
            }
            None => {
                if (command.dm_permission == Some(false) && place == BOT_DM) || place == PRIVATE_CHANNEL {
                    return Access::ContextNotAllowed;
                }
            }
        }
        let Some(guild) = self.guild else { return Access::Allowed };
        let administrator = self.permissions.contains(Permissions::ADMINISTRATOR);
        let user_installed = command.integration_types.as_ref().is_some_and(|t| t.contains(&USER_INSTALL));
        if administrator || (installed.user && user_installed) {
            return Access::Allowed;
        }
        let guild_install = command.integration_types.as_ref().is_none_or(|t| t.contains(&GUILD_INSTALL));
        if (!self.base_access || !self.can_send) && installed.guild && guild_install {
            return Access::MissingBasePermissions;
        }
        let channel = channel_rule(&command.permissions, self, guild);
        if channel == Some(false) || (channel != Some(true) && app_allows_channel == Some(false)) {
            return Access::ChannelDenied;
        }
        match allows(&command.permissions, guild, self.me, &self.roles) {
            Some(true) => return Access::Allowed,
            Some(false) => return Access::UserDenied,
            None => {}
        }
        if app_allows_user == Some(false) {
            return Access::UserDenied;
        }
        match command.default_member_permissions {
            Some(needed) if needed == Permissions::NONE || !self.permissions.contains(needed) => Access::UserDenied,
            _ => Access::Allowed,
        }
    }
}

// ---- Parsing the index ----

fn text(value: &Value) -> Option<&str> {
    value.as_str()
}

/// JavaScript's `a ?? b`: `a` unless it is missing or null.
fn coalesce(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    [a, b].into_iter().flatten().find(|v| !v.is_null()).cloned()
}

/// Set `key` to `value`, or drop it where JavaScript would have `undefined`.
fn put(object: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(value) => object.insert(key.to_owned(), value),
        None => object.remove(key),
    };
}

/// A name or description with its default: `name` is what the account's
/// language has, and `name_default` the original. The client keeps the
/// original and notes the other as `*_localized`, which is how `eo`, `ed` and
/// `q` in 264322 rewrite every name and description.
fn defaults(object: &mut Map<String, Value>, field: &str) {
    let default_key = format!("{field}_default");
    let (shown, default) = (object.get(field).cloned(), object.get(&default_key).cloned());
    put(object, field, coalesce(default.as_ref(), shown.as_ref()));
    if shown != default {
        put(object, &format!("{field}_localized"), shown);
    }
}

fn process_choice(choice: &Value) -> Value {
    let mut choice = choice.as_object().cloned().unwrap_or_default();
    defaults_name(&mut choice);
    Value::Object(choice)
}

/// A choice has a name and no description.
fn defaults_name(object: &mut Map<String, Value>) {
    defaults(object, "name");
}

fn process_option(option: &Value) -> Value {
    let mut object = option.as_object().cloned().unwrap_or_default();
    if let Some(choices) = option["choices"].as_array() {
        object.insert("choices".into(), Value::Array(choices.iter().map(process_choice).collect()));
    }
    defaults(&mut object, "description");
    defaults(&mut object, "name");
    if let Some(options) = option["options"].as_array() {
        object.insert("options".into(), Value::Array(options.iter().map(process_option).collect()));
    }
    Value::Object(object)
}

/// A root command as the client keeps and sends it (`q` in 264322).
fn process_root(command: &Value, me: Snowflake) -> Value {
    let mut object = command.as_object().cloned().unwrap_or_default();
    defaults(&mut object, "description");
    if object.get("description").is_none_or(Value::is_null) {
        object.insert("description".into(), json!(""));
    }
    defaults(&mut object, "name");
    let options = command["options"].as_array().map(|o| o.iter().map(process_option).collect()).unwrap_or_default();
    object.insert("options".into(), Value::Array(options));
    if object.get("permissions").is_some_and(|p| !p.is_null()) {
        let rules = rules(&command["permissions"], me);
        object.insert("permissions".into(), rules_json(&rules));
    } else {
        object.remove("permissions");
    }
    Value::Object(object)
}

fn option_of(option: &Value) -> CommandOption {
    let choices = option["choices"].as_array().map(|choices| {
        choices
            .iter()
            .map(|c| Choice {
                name: text(&c["name_localized"]).or_else(|| text(&c["name"])).unwrap_or_default().to_owned(),
                value: c["value"].clone(),
            })
            .collect()
    });
    CommandOption {
        name: text(&option["name"]).unwrap_or_default().to_owned(),
        display_name: text(&option["name_localized"]).or_else(|| text(&option["name"])).unwrap_or_default().to_owned(),
        server_name: text(&option["name_localized"]).map(str::to_owned),
        description: text(&option["description_localized"])
            .or_else(|| text(&option["description"]))
            .unwrap_or_default()
            .to_owned(),
        kind: option["type"].as_u64().unwrap_or(0) as u8,
        required: option["required"].as_bool().unwrap_or(false),
        choices,
        autocomplete: option["autocomplete"].as_bool().unwrap_or(false),
        min_value: option["min_value"].as_f64(),
        max_value: option["max_value"].as_f64(),
        min_length: option["min_length"].as_u64(),
        max_length: option["max_length"].as_u64(),
        channel_types: option["channel_types"].as_array().map(|t| t.iter().filter_map(Value::as_u64).collect()),
        file_types: if option["type"].as_u64() == Some(u64::from(option_type::ATTACHMENT)) {
            option["file_types"].as_array().into_iter().flatten().filter_map(text).map(str::to_owned).collect()
        } else {
            Vec::new()
        },
    }
}

fn leaf(root: &Value, command: &Value, path: Vec<Step>) -> Command {
    let id_parts = std::iter::once(root["id"].as_str().unwrap_or_default().to_owned())
        .chain(path.iter().map(|s| s.name.clone()))
        .collect::<Vec<_>>();
    let name_parts = std::iter::once(text(&root["name"]).unwrap_or_default().to_owned())
        .chain(path.iter().map(|s| s.name.clone()))
        .collect::<Vec<_>>();
    let display_parts =
        std::iter::once(text(&root["name_localized"]).or_else(|| text(&root["name"])).unwrap_or_default().to_owned())
            .chain(path.iter().map(|s| s.display_name.clone()))
            .collect::<Vec<_>>();
    let ids = |key: &str| root[key].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect::<Vec<_>>());
    let permissions = root["permissions"].as_array().map_or_else(Vec::new, |list| {
        list.iter()
            .filter_map(|r| {
                let kind = match r["type"].as_u64()? {
                    1 => RuleKind::Role,
                    2 => RuleKind::User,
                    3 => RuleKind::Channel,
                    _ => return None,
                };
                Some(Rule {
                    kind,
                    id: Snowflake(r["id"].as_str()?.parse().ok()?),
                    permission: r["permission"].as_bool()?,
                })
            })
            .collect()
    });
    Command {
        id: id_parts.join("\0"),
        application: root["application_id"].as_str().and_then(|s| s.parse().ok()).map(Snowflake).unwrap_or_default(),
        kind: root["type"].as_u64().unwrap_or(u64::from(command_type::CHAT)) as u8,
        version: root.get("version").cloned(),
        guild: root.get("guild_id").cloned(),
        name: name_parts.join(" "),
        display_name: display_parts.join(" "),
        description: text(&command["description"]).unwrap_or_default().to_owned(),
        display_description: text(&command["description_localized"])
            .or_else(|| text(&command["description"]))
            .unwrap_or_default()
            .to_owned(),
        options: command["options"].as_array().map(|o| o.iter().map(option_of).collect()).unwrap_or_default(),
        path,
        popularity: root["global_popularity_rank"].as_u64(),
        builtin: None,
        root: root.clone(),
        root_id: root["id"].clone(),
        root_name: text(&root["name"]).unwrap_or_default().to_owned(),
        nsfw: root["nsfw"].as_bool().unwrap_or(false),
        dm_permission: root["dm_permission"].as_bool(),
        contexts: ids("contexts"),
        integration_types: ids("integration_types"),
        default_member_permissions: root["default_member_permissions"]
            .as_str()
            .and_then(|p| p.parse().ok())
            .map(Permissions),
        permissions,
    }
}

fn step(command: &Value, kind: u8) -> Step {
    Step {
        name: text(&command["name"]).unwrap_or_default().to_owned(),
        kind,
        display_name: text(&command["name_localized"])
            .or_else(|| text(&command["name"]))
            .unwrap_or_default()
            .to_owned(),
    }
}

/// The commands one command stands for (`Lz` in 168186): itself, or each of
/// its sub-commands.
fn flatten(root: &Value, command: &Value, path: Vec<Step>) -> Vec<Command> {
    let options = command["options"].as_array().filter(|o| !o.is_empty());
    let is_root = command.get("id").is_some();
    let kind = command["type"].as_u64().unwrap_or(0) as u8;
    let sub = |o: &&Value| o["type"].as_u64() == Some(u64::from(option_type::SUB_COMMAND));
    let group = |o: &&Value| o["type"].as_u64() == Some(u64::from(option_type::SUB_COMMAND_GROUP));
    let plain = options.is_none();
    if plain && (is_root || (kind != option_type::SUB_COMMAND && kind != option_type::SUB_COMMAND_GROUP)) {
        return vec![leaf(root, command, path)];
    }
    let Some(options) = command["options"].as_array() else { return Vec::new() };
    let mut found = Vec::new();
    let groups: Vec<&Value> = options.iter().filter(group).collect();
    for g in &groups {
        let mut path = path.clone();
        path.push(step(g, option_type::SUB_COMMAND_GROUP));
        found.extend(flatten(root, g, path));
    }
    let subs: Vec<&Value> = options.iter().filter(sub).collect();
    for s in &subs {
        let mut path = path.clone();
        path.push(step(s, option_type::SUB_COMMAND));
        found.push(leaf(root, s, path));
    }
    if groups.is_empty() && subs.is_empty() {
        found.push(leaf(root, command, path));
    }
    found
}

fn parse_index(index: &Value, me: Snowflake) -> Loaded {
    let mut sections: Vec<Section> = Vec::new();
    for app in index["applications"].as_array().into_iter().flatten() {
        let Some(id) = app["id"].as_str().and_then(|s| s.parse().ok()).map(Snowflake) else { continue };
        let bot = app["bot"]["id"].as_str().or_else(|| app["bot_id"].as_str()).and_then(|s| s.parse().ok());
        let name = text(&app["bot"]["username"]).or_else(|| text(&app["name"])).unwrap_or_default().to_owned();
        let surfaces = app["embedded_surfaces"].as_array().into_iter().flatten();
        sections.push(Section {
            app: App {
                id,
                name,
                app_name: text(&app["name"]).unwrap_or_default().to_owned(),
                description: text(&app["description"]).unwrap_or_default().to_owned(),
                icon: text(&app["icon"]).map(str::to_owned),
                bot: bot.map(Snowflake),
                bot_avatar: text(&app["bot"]["avatar"]).map(str::to_owned),
                flags: app["flags"].as_u64().unwrap_or(0),
                activity: surfaces.into_iter().any(|s| s.as_u64() == Some(0)),
                permissions: if app["permissions"].is_null() { Vec::new() } else { rules(&app["permissions"], me) },
            },
            commands: Vec::new(),
        });
    }
    for raw in index["application_commands"].as_array().into_iter().flatten() {
        if raw["id"].is_null() {
            continue;
        }
        let root = process_root(raw, me);
        let Some(application) = raw["application_id"].as_str().and_then(|s| s.parse().ok()).map(Snowflake) else {
            continue;
        };
        // A command with no app in the index has nowhere to be listed.
        let Some(section) = sections.iter_mut().find(|s| s.app.id == application) else { continue };
        section.commands.extend(flatten(&root, &root, Vec::new()));
    }
    Loaded { version: index["version"].as_str().map(str::to_owned), sections }
}

// ---- Running a command ----

/// What can be told apart in the names people type (`@name`, `#name`).
pub trait Names {
    fn user(&self, name: &str) -> Option<Snowflake>;
    fn role(&self, name: &str) -> Option<Snowflake>;
    fn channel(&self, name: &str) -> Option<Snowflake>;
    /// The server, which `@everyone` stands for.
    fn guild(&self) -> Option<Snowflake>;
}

/// `Ut1` in 652215: an id typed as it is.
fn is_snowflake(text: &str) -> bool {
    (17..=19).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// What `text` names for an option of `kind` (`A.f` in 224868, and the cases
/// of `A` in 545152): the mention the box writes for a pick (`<@id>`,
/// `<@!id>`, `<@&id>`, `<#id>`, of the kinds the option takes), an id as it
/// is, else a `@user`, `@role` or `#channel` by its name.
fn resolve(kind: u8, typed: &str, names: &dyn Names) -> Option<Snowflake> {
    use option_type::*;
    let typed = typed.trim();
    let mention = |prefix: &str| {
        let id = typed.strip_prefix(prefix)?.strip_suffix('>')?;
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.parse().ok().map(Snowflake))?
    };
    let user = || mention("<@!").or_else(|| mention("<@"));
    let marked = match kind {
        USER => user(),
        ROLE => mention("<@&"),
        MENTIONABLE => user().or_else(|| mention("<@&")),
        CHANNEL => mention("<#"),
        _ => None,
    };
    if marked.is_some() {
        return marked;
    }
    if is_snowflake(typed) {
        return typed.parse().ok().map(Snowflake);
    }
    let by_user = |n: &str| names.user(n);
    let by_role = |n: &str| names.role(n);
    match (kind, typed.chars().next()) {
        (CHANNEL, Some('#')) => names.channel(typed[1..].trim_matches('"')),
        (USER, Some('@')) => by_user(&typed[1..]),
        (ROLE, Some('@')) if typed == "@everyone" => names.guild(),
        (ROLE, Some('@')) => by_role(&typed[1..]),
        (MENTIONABLE, Some('@')) if typed == "@everyone" => names.guild(),
        (MENTIONABLE, Some('@')) => by_role(&typed[1..]).or_else(|| by_user(&typed[1..])),
        _ => None,
    }
}

/// JavaScript's `Number(text)` after the account's thousands separators are
/// taken out (`ib` in 151596, for the English the client is set to).
fn number(typed: &str) -> Option<f64> {
    let typed = typed.replace(',', "");
    let typed = typed.trim();
    let parsed: f64 = if typed.is_empty() { 0.0 } else { typed.parse().ok()? };
    parsed.is_finite().then_some(parsed)
}

/// A number as JSON would carry it from JavaScript: whole numbers without a
/// fraction.
fn number_json(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 { json!(n as i64) } else { json!(n) }
}

fn bool_of(typed: &str) -> Option<bool> {
    match typed.to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn choice_named<'a>(choices: &'a [Choice], typed: &str) -> Option<&'a Choice> {
    choices.iter().find(|c| c.name == typed)
}

const REQUIRED: &str = "This option is required. Specify a value.";

/// What is wrong with an option's value, as the client says it (`J` in
/// 300541). `content` is the pill's value, none without a pill, which only
/// a required option fails. An empty value passes with `allow_empty` (as a
/// command starts), else fails as required, or, for an optional one, as its
/// type ([`type_error`]).
pub fn validate(
    option: &CommandOption,
    content: Option<&str>,
    allow_empty: bool,
    names: &dyn Names,
    auto: &Autocompleted,
) -> Result<(), String> {
    use option_type::*;
    let Some(content) = content else { return if option.required { Err(REQUIRED.into()) } else { Ok(()) } };
    let typed = content.trim();
    if typed.is_empty() {
        return match (allow_empty, option.required) {
            (true, _) => Ok(()),
            (false, true) => Err(REQUIRED.into()),
            (false, false) => Err(type_error(option)),
        };
    }
    let invalid = || Err(type_error(option));
    match option.kind {
        BOOLEAN => bool_of(typed).map(|_| ()).ok_or_else(|| type_error(option)),
        STRING => {
            if let Some(choices) = &option.choices {
                return choice_named(choices, typed).map(|_| ()).ok_or_else(|| type_error(option));
            }
            if option.autocomplete && auto.value_of(&option.name, typed).is_some() {
                return Ok(());
            }
            // JavaScript's `.length`: UTF-16 units, as a message counts.
            let length = limits::message_length(typed) as u64;
            match (option.min_length, option.max_length) {
                (Some(min), Some(max)) if min == max && length != min => Err(format!("Enter {min} characters.")),
                (Some(min), Some(max)) if length < min || length > max => {
                    Err(format!("Enter between {min} and {max} characters."))
                }
                (Some(min), _) if length < min => Err(format!("Enter {min} or more characters.")),
                (_, Some(max)) if length > max => Err(format!("Enter {max} or fewer characters.")),
                _ => Ok(()),
            }
        }
        INTEGER | NUMBER => {
            if let Some(choices) = &option.choices {
                return choices
                    .iter()
                    .any(|c| c.name == typed && c.value.is_number())
                    .then_some(())
                    .ok_or_else(|| type_error(option));
            }
            if option.autocomplete && auto.value_of(&option.name, typed).is_some_and(Value::is_number) {
                return Ok(());
            }
            let Some(n) = number(typed) else { return invalid() };
            if option.kind == INTEGER && (n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0) {
                return invalid();
            }
            if option.kind == NUMBER && n.abs() > 9_007_199_254_740_991.0 {
                return invalid();
            }
            match (option.min_value, option.max_value) {
                (Some(min), Some(max)) if n < min || n > max => {
                    Err(format!("Enter a number between {} and {}.", plain(min), plain(max)))
                }
                (Some(min), _) if n < min => Err(format!("Enter a number greater than or equal to {}.", plain(min))),
                (_, Some(max)) if n > max => Err(format!("Enter a number less than or equal to {}.", plain(max))),
                _ => Ok(()),
            }
        }
        USER | CHANNEL | ROLE | MENTIONABLE => {
            resolve(option.kind, typed, names).map(|_| ()).ok_or_else(|| type_error(option))
        }
        _ => invalid(),
    }
}

/// What a value of the option's type that isn't one is told (`tE` in
/// 73510).
pub fn type_error(option: &CommandOption) -> String {
    use option_type::*;
    let words = match option.kind {
        _ if option.choices.is_some() => "Not a valid choice.",
        BOOLEAN => "Pick either true or false.",
        CHANNEL => "Not a valid channel.",
        INTEGER => "Input a valid integer.",
        NUMBER => "Input a valid number.",
        ROLE => "Not a valid role.",
        USER => "Not a valid user.",
        MENTIONABLE => "Not a valid user or role.",
        _ => "Not a valid value.",
    };
    words.to_owned()
}

/// The choices a BOOLEAN option offers (`Ss` in 73510).
pub fn boolean_choices() -> [Choice; 2] {
    [Choice { name: "True".into(), value: json!(true) }, Choice { name: "False".into(), value: json!(false) }]
}

/// The choices that match `query`, best first (`queryChoice` in 997625):
/// the name itself, then starting with it, containing it, containing each
/// of its words, its letters in order; equals keep their order. Nothing
/// typed lists them all.
pub fn match_choices<'c>(choices: &'c [Choice], query: &str) -> Vec<&'c Choice> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split([',', ' ']).collect();
    let mut scored: Vec<(u8, &Choice)> = choices
        .iter()
        .filter_map(|choice| {
            let name = choice.name.to_lowercase();
            let score = if name.starts_with(&query) {
                if name == query { 10 } else { 7 }
            } else if name.contains(&query) {
                5
            } else if words.iter().all(|w| name.contains(w)) {
                3
            } else if fuzzy(&query, &name) {
                1
            } else {
                0
            };
            (score > 0).then_some((score, choice))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, choice)| choice).collect()
}

/// A limit as the client writes it (`x` in 300541, `toLocaleString` without
/// grouping): the number's shortest decimal form, rounded half away from zero
/// to at most three decimals, with its sign even where that leaves a zero.
fn plain(n: f64) -> String {
    let shortest = n.abs().to_string();
    let (whole, fraction) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let kept = fraction.len().min(3);
    let mut digits: Vec<u8> = whole.bytes().chain(fraction.bytes().take(3)).collect();
    if fraction.as_bytes().get(3).is_some_and(|d| *d >= b'5') {
        match digits.iter().rposition(|d| *d != b'9') {
            Some(at) => {
                digits[at] += 1;
                digits[at + 1..].fill(b'0');
            }
            None => {
                digits.fill(b'0');
                digits.insert(0, b'1');
            }
        }
    }
    let mut text: String = digits.into_iter().map(char::from).collect();
    let point = text.len() - kept;
    let decimals = text[point..].trim_end_matches('0').len();
    text.truncate(point + decimals);
    if decimals > 0 {
        text.insert(point, '.');
    }
    if n.is_sign_negative() {
        text.insert(0, '-');
    }
    text
}

/// What the user entered, option name to text.
pub type Values = BTreeMap<String, String>;

/// The options of a run in the form interactions carry (`T` in `A`, 545152):
/// each option that has text, with the value its type makes of it, inside the
/// sub-command groups that lead to it. With `focused`, the option being
/// autocompleted goes out marked, with what was typed so far as it is,
/// spaces and all (`autocomplete.query`); the others are trimmed.
pub fn run_options(
    command: &Command,
    values: &Values,
    names: &dyn Names,
    auto: &Autocompleted,
    focused: Option<&str>,
) -> Result<Vec<Value>, String> {
    use option_type::*;
    let mut options = Vec::new();
    for option in &command.options {
        if matches!(option.kind, SUB_COMMAND | SUB_COMMAND_GROUP | ATTACHMENT) {
            continue;
        }
        let is_focus = focused == Some(option.name.as_str());
        let Some(as_typed) = values.get(&option.name) else {
            if is_focus {
                options.push(json!({ "type": option.kind, "name": option.name, "value": "", "focused": true }));
            }
            continue;
        };
        let typed = as_typed.trim();
        let value: Value = match option.kind {
            STRING => {
                let mut value = option
                    .choices
                    .as_deref()
                    .and_then(|c| choice_named(c, typed))
                    .map(|c| c.value.clone())
                    .filter(Value::is_string);
                if value.is_none() && option.choices.is_none() && option.autocomplete {
                    value = if is_focus { Some(json!(as_typed)) } else { auto.value_of(&option.name, typed).cloned() };
                }
                if value.is_none() {
                    value = Some(json!(typed));
                }
                if focused.is_some() && !is_focus && typed.is_empty() {
                    continue;
                }
                value.unwrap_or_default()
            }
            CHANNEL | ROLE | USER | MENTIONABLE => {
                if typed.is_empty() && !is_focus {
                    continue;
                }
                match resolve(option.kind, typed, names) {
                    Some(id) => json!(id.to_string()),
                    None if is_focus => json!(typed),
                    None => return Err(format!("Failed to resolve {typed}")),
                }
            }
            BOOLEAN => match bool_of(typed) {
                Some(b) => json!(b),
                None if typed.is_empty() && !is_focus => continue,
                None => return Err(format!("Unexpected value for option \"{}\"", option.name)),
            },
            INTEGER | NUMBER => {
                if typed.is_empty() && !is_focus {
                    continue;
                }
                let by_choice = option
                    .choices
                    .as_deref()
                    .and_then(|c| choice_named(c, typed).map(|c| c.value.clone()).filter(Value::is_number));
                let by_autocomplete = || {
                    if option.autocomplete {
                        if is_focus { Some(json!(as_typed)) } else { auto.value_of(&option.name, typed).cloned() }
                    } else {
                        None
                    }
                };
                by_choice
                    .or_else(|| option.choices.is_none().then(by_autocomplete).flatten())
                    .or_else(|| number(typed).map(number_json))
                    .ok_or_else(|| format!("Unexpected value for option \"{}\"", option.name))?
            }
            _ => return Err(format!("Unsupported option type: {}", option.kind)),
        };
        let mut entry = json!({ "type": option.kind, "name": option.name, "value": value });
        if is_focus {
            entry["focused"] = json!(true);
        }
        options.push(entry);
    }
    // Sub-command groups wrap the options, innermost first.
    for step in command.path.iter().rev() {
        options = vec![json!({ "type": step.kind, "name": step.name, "options": options })];
    }
    Ok(options)
}

/// The options of a run without the sub-command groups around them
/// (`Ez` in 168186).
pub fn leaf_options(mut options: &[Value]) -> &[Value] {
    while let [only] = options
        && matches!(only["type"].as_u64(), Some(1 | 2))
    {
        options = only["options"].as_array().map_or(&[], Vec::as_slice);
    }
    options
}

/// What the other options held when an option asked for choices, which the
/// answers depend on (`WL` in 721768): `name=value` and a NUL for each.
pub fn context_key(options: &[Value]) -> String {
    let mut key = String::new();
    for option in leaf_options(options) {
        if option["focused"].as_bool() == Some(true) {
            continue;
        }
        let value = match &option["value"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        key.push_str(&format!("{}={value}\0", option["name"].as_str().unwrap_or_default()));
    }
    key
}

impl Command {
    /// The root command's id, which Copy Command ID copies; none for a
    /// built-in.
    pub fn root_id(&self) -> Option<Snowflake> {
        self.root_id.as_str()?.parse().ok().map(Snowflake)
    }

    pub fn is_built_in(&self) -> bool {
        self.builtin.is_some()
    }

    /// The search a built-in runs, for `/gif` and `/sticker`.
    pub fn integration(&self) -> Option<Integration> {
        match self.builtin?.input() {
            Input::Integration(integration) => Some(integration),
            Input::Text | Input::Action => None,
        }
    }

    /// The command's `data` in an interaction (`R` in 545152): its root's id,
    /// version and name, the options of the run, and the root itself.
    pub fn data(&self, options: Vec<Value>, target: Option<Snowflake>) -> Value {
        let mut data = Map::new();
        if let Some(version) = &self.version {
            data.insert("version".into(), version.clone());
        }
        data.insert("id".into(), self.root_id.clone());
        if let Some(guild) = &self.guild {
            data.insert("guild_id".into(), guild.clone());
        }
        data.insert("name".into(), json!(self.root_name));
        data.insert("type".into(), json!(self.kind));
        data.insert("options".into(), Value::Array(options));
        data.insert("application_command".into(), self.root.clone());
        if let Some(target) = target {
            data.insert("target_id".into(), json!(target.to_string()));
        }
        Value::Object(data)
    }

    /// Whether the form can't take an option: files, which Neucord doesn't
    /// attach to a command yet.
    pub fn needs_attachment(&self) -> bool {
        self.options.iter().any(|o| o.kind == option_type::ATTACHMENT && o.required)
    }
}

/// Where a run is sent from.
pub struct Run<'a> {
    pub application: Snowflake,
    pub guild: Option<Snowflake>,
    pub channel: Snowflake,
    pub session: &'a str,
    pub data: Value,
    pub nonce: Snowflake,
}

impl Run<'_> {
    /// `POST /interactions` for running the command (`handleCommand` in
    /// 870801): `analytics_location` says it came from the suggestions
    /// (`slash_ui`). A DM has no `guild_id`. `data.attachments` names the
    /// run's uploaded files, `[]` when there are none, since `A` in 545152
    /// always passes its list on.
    pub fn body(&self, files: &[Sent<'_>]) -> Value {
        let mut body = self.body_of(APPLICATION_COMMAND, true);
        body["data"]["attachments"] = files.iter().enumerate().map(|(i, file)| file.body(i)).collect();
        body
    }

    /// The same for asking an app to autocomplete an option (`WL` in
    /// 721768), which has no analytics fields and no `attachments`.
    pub fn autocomplete_body(&self) -> Value {
        self.body_of(APPLICATION_COMMAND_AUTOCOMPLETE, false)
    }

    fn body_of(&self, kind: u8, analytics: bool) -> Value {
        let mut body = json!({
            "type": kind,
            "application_id": self.application.to_string(),
            "channel_id": self.channel.to_string(),
            "session_id": self.session,
            "data": self.data,
            "nonce": self.nonce.to_string(),
        });
        if let Some(guild) = self.guild {
            body["guild_id"] = json!(guild.to_string());
        }
        if analytics {
            body["analytics_location"] = json!("slash_ui");
        }
        body
    }
}

#[cfg(test)]
impl Command {
    /// A command as an index would give it, for other modules' tests.
    pub(crate) fn made_up(id: &str, kind: u8, application: u64, guild: Option<&str>) -> Command {
        let root = json!({"id": id, "application_id": application.to_string(), "type": kind, "name": "made-up",
                          "description": "", "guild_id": guild});
        leaf(&root, &root, Vec::new())
    }

    /// A root command from its JSON, without sub-commands.
    pub(crate) fn of_json(root: &Value) -> Command {
        leaf(root, root, Vec::new())
    }
}

// ---- Autocomplete ----

/// The choices an app gave for the option typed in (`ApplicationCommandAutocompleteStore`,
/// 166862), by what was typed.
#[derive(Clone, Debug, Default)]
pub struct Autocompleted {
    /// The app's own answers: option name, then the query.
    answers: HashMap<String, HashMap<String, Vec<Choice>>>,
    /// The latest choices for each option, which running the command reads.
    last: HashMap<String, Vec<Choice>>,
}

impl Autocompleted {
    /// The value of the choice named `typed` among the latest for `option`
    /// (`Xf` and `lM` in 360161).
    pub fn value_of(&self, option: &str, typed: &str) -> Option<&Value> {
        self.last.get(option)?.iter().find(|c| c.name == typed).map(|c| &c.value)
    }

    /// The choices to show for `query` in `option`, if they are known.
    pub fn choices(&self, option: &str, query: &str) -> Option<&[Choice]> {
        self.answers.get(option)?.get(query).map(Vec::as_slice)
    }

    /// The latest choices for `option`.
    pub fn latest(&self, option: &str) -> &[Choice] {
        self.last.get(option).map_or(&[], Vec::as_slice)
    }
}

/// A request for choices in flight.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    option: String,
    query: String,
}

/// The autocomplete requests of one command being filled in.
#[derive(Debug, Default)]
pub struct Autocomplete {
    answered: Autocompleted,
    asked: HashMap<Snowflake, Asked>,
    /// The request waiting for each option, which a newer one replaces.
    waiting: HashMap<String, Snowflake>,
    /// What the other options held when each option last asked: a change
    /// forgets its answers.
    context: HashMap<String, String>,
    last_query: HashMap<String, String>,
    pub errored: bool,
}

impl Autocomplete {
    pub fn answers(&self) -> &Autocompleted {
        &self.answered
    }

    /// Whether to send a request for `query` in `option` as `nonce`
    /// (`APPLICATION_COMMAND_AUTOCOMPLETE_REQUEST` in 166862, and `WL` in
    /// 721768, which sends only if the answer isn't known). `context` is the
    /// other options' values.
    pub fn request(&mut self, nonce: Snowflake, option: &str, query: &str, context: &str) -> bool {
        if self.context.get(option).map(String::as_str) != Some(context) {
            self.context.insert(option.to_owned(), context.to_owned());
            self.answered.answers.remove(option);
            self.answered.last.remove(option);
            self.last_query.remove(option);
        }
        if self.last_query.get(option).map(String::as_str) == Some(query) {
            return false;
        }
        self.last_query.insert(option.to_owned(), query.to_owned());
        if let Some(known) = self.answered.answers.get(option).and_then(|a| a.get(query)).cloned() {
            self.errored = false;
            self.answered.last.insert(option.to_owned(), known);
            return false;
        }
        if let Some(old) = self.waiting.insert(option.to_owned(), nonce) {
            self.asked.remove(&old);
        }
        self.asked.insert(nonce, Asked { option: option.to_owned(), query: query.to_owned() });
        self.errored = false;
        true
    }

    /// The app answered (`APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE`).
    /// Returns the option, if the answer was for us.
    pub fn answer(&mut self, nonce: Snowflake, choices: &Value, integer: impl Fn(&str) -> bool) -> Option<String> {
        let asked = self.asked.remove(&nonce)?;
        let whole = integer(&asked.option);
        let list: Vec<Choice> = choices
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let value = if whole {
                    c["value"]
                        .as_f64()
                        .or_else(|| c["value"].as_str().and_then(number))
                        .map_or(c["value"].clone(), number_json)
                } else {
                    c["value"].clone()
                };
                Choice {
                    name: text(&c["name_localized"]).or_else(|| text(&c["name"])).unwrap_or_default().to_owned(),
                    value,
                }
            })
            .collect();
        self.answered.answers.entry(asked.option.clone()).or_default().insert(asked.query.clone(), list.clone());
        if self.last_query.get(&asked.option) == Some(&asked.query) {
            self.errored = false;
            self.answered.last.insert(asked.option.clone(), list);
        }
        Some(asked.option)
    }

    /// The request failed (`INTERACTION_FAILURE`). Returns the option it was
    /// for, if it was ours.
    pub fn failed(&mut self, nonce: Snowflake) -> Option<String> {
        let asked = self.asked.remove(&nonce)?;
        self.errored = true;
        Some(asked.option)
    }
}

/// What the client says when an app didn't answer (`s` in 911929): the
/// `TIMEOUT` reason is 2.
pub fn reason_text(reason: Option<u64>, app: Option<&str>) -> Option<String> {
    match reason {
        Some(2) => Some(match app {
            Some(app) => format!("{app} didn't respond in time"),
            None => "The application didn't respond in time".into(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Snowflake = Snowflake(100_000_000_000_000_001);
    const GUILD_ID: Snowflake = Snowflake(100_000_000_000_000_010);
    const CHANNEL: Snowflake = Snowflake(100_000_000_000_000_020);
    const APP: &str = "200000000000000001";

    fn index() -> Value {
        json!({
            "version": "7",
            "applications": [
                {"id": APP, "name": "Dicey", "description": "rolls dice", "bot_id": "300000000000000001", "flags": 0},
                {"id": "200000000000000002", "name": "Zed", "description": "other things", "bot": {"id": "300000000000000002", "username": "Zed Bot"}},
            ],
            "application_commands": [
                {"id": "400000000000000001", "application_id": APP, "version": "500000000000000001", "type": 1,
                 "name": "roll", "name_default": "roll", "description": "Roll some dice", "description_default": "Roll some dice",
                 "dm_permission": true, "contexts": [0, 1, 2], "integration_types": [0, 1],
                 "options": [
                    {"type": 4, "name": "count", "description": "How many", "required": true, "min_value": 1, "max_value": 10},
                    {"type": 3, "name": "size", "description": "Which die", "choices": [{"name": "D6", "value": "6"}, {"name": "D20", "value": "20"}]},
                 ]},
                {"id": "400000000000000002", "application_id": APP, "version": "500000000000000002", "type": 1,
                 "name": "stats", "name_default": "stats", "description": "Stats", "description_default": "Stats",
                 "options": [
                    {"type": 1, "name": "show", "description": "Show", "options": [{"type": 6, "name": "who", "description": "Whose", "required": true}]},
                    {"type": 2, "name": "reset", "description": "Reset", "options": [{"type": 1, "name": "all", "description": "Everything"}]},
                 ]},
                {"id": "400000000000000003", "application_id": "200000000000000002", "version": "5", "type": 2,
                 "name": "Inspect", "description": ""},
                {"id": "400000000000000004", "application_id": APP, "version": "6", "type": 3,
                 "name": "Reroll", "description": "", "contexts": [0, 1, 2]},
                {"id": "400000000000000005", "application_id": "200000000000000002", "version": "7", "type": 3,
                 "name": "Annotate", "description": ""},
                {"id": "400000000000000006", "application_id": APP, "version": "8", "type": 3,
                 "name": "Roll 10 times", "description": ""},
                {"id": "400000000000000007", "application_id": APP, "version": "9", "type": 3,
                 "name": "Roll 9 times", "description": ""},
            ]
        })
    }

    fn place() -> Place {
        Place {
            channel: CHANNEL,
            parent: None,
            guild: Some(GUILD_ID),
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

    fn loaded(index: &Value) -> CommandIndex {
        let mut commands = CommandIndex::default();
        assert!(commands.wants(Target::Guild(GUILD_ID), Instant::now()));
        commands.loaded(Target::Guild(GUILD_ID), index, ME);
        commands
    }

    struct Nobody;
    impl Names for Nobody {
        fn user(&self, _: &str) -> Option<Snowflake> {
            None
        }
        fn role(&self, _: &str) -> Option<Snowflake> {
            None
        }
        fn channel(&self, _: &str) -> Option<Snowflake> {
            None
        }
        fn guild(&self) -> Option<Snowflake> {
            None
        }
    }

    struct Known;
    impl Names for Known {
        fn user(&self, name: &str) -> Option<Snowflake> {
            (name == "ann").then_some(Snowflake(600_000_000_000_000_001))
        }
        fn role(&self, name: &str) -> Option<Snowflake> {
            (name == "mods").then_some(Snowflake(600_000_000_000_000_002))
        }
        fn channel(&self, name: &str) -> Option<Snowflake> {
            (name == "general").then_some(Snowflake(600_000_000_000_000_003))
        }
        fn guild(&self) -> Option<Snowflake> {
            Some(GUILD_ID)
        }
    }

    fn names<'a>(found: &[Found<'a>]) -> Vec<&'a str> {
        found.iter().map(|f| f.command.display_name.as_str()).collect()
    }

    #[test]
    fn sub_commands_are_listed_one_by_one_with_their_own_options() {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "");
        assert_eq!(names(&found), ["roll", "stats reset all", "stats show"]);
        let show = found.iter().find(|f| f.command.name == "stats show").unwrap().command;
        assert_eq!(show.options.len(), 1);
        assert_eq!(show.options[0].name, "who");
        assert_eq!(show.id, "400000000000000002\0show");
    }

    #[test]
    fn user_commands_are_not_slash_commands() {
        let commands = loaded(&index());
        assert!(names(&commands.find(&place(), command_type::CHAT, "inspect")).is_empty());
        assert_eq!(names(&commands.find(&place(), command_type::USER, "")), ["Inspect"]);
    }

    #[test]
    fn apps_list_their_commands_by_name_under_their_own_names() {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::MESSAGE, "");
        let listed: Vec<(&str, Vec<&str>)> = by_app(found)
            .into_iter()
            .map(|(app, commands)| (app.name.as_str(), commands.iter().map(|c| c.display_name.as_str()).collect()))
            .collect();
        // Numbers count as numbers; the app with the first name is first.
        assert_eq!(listed, [("Dicey", vec!["Reroll", "Roll 9 times", "Roll 10 times"]), ("Zed Bot", vec!["Annotate"])]);
        // A search lists them flat, best first, as `find` has it.
        assert_eq!(names(&commands.find(&place(), command_type::MESSAGE, "annot")), ["Annotate"]);
    }

    #[test]
    fn the_apps_item_is_in_textual_channels_but_not_media_threads() {
        for kind in [
            ChannelKind::Text,
            ChannelKind::Dm,
            ChannelKind::Voice,
            ChannelKind::GroupDm,
            ChannelKind::Announcement,
            ChannelKind::AnnouncementThread,
            ChannelKind::PublicThread,
            ChannelKind::PrivateThread,
            ChannelKind::Stage,
        ] {
            assert!(offers_apps(kind), "{kind:?}");
        }
        for kind in [ChannelKind::Category, ChannelKind::Forum, ChannelKind::Media, ChannelKind::MediaThread] {
            assert!(!offers_apps(kind), "{kind:?}");
        }
    }

    #[test]
    fn a_user_or_message_command_runs_on_its_target() {
        let commands = loaded(&index());
        let place = place();
        let (command, app) = {
            let found = commands.find(&place, command_type::MESSAGE, "reroll");
            (found[0].command.clone(), found[0].app.clone())
        };
        let target = Snowflake(700_000_000_000_000_001);
        let run = Run {
            application: command.application,
            guild: Some(GUILD_ID),
            channel: CHANNEL,
            session: "s",
            data: command.data(Vec::new(), Some(target)),
            nonce: Snowflake(5),
        };
        let body = run.body(&[]);
        assert_eq!(body["application_id"], app.id.to_string());
        let data = &body["data"];
        assert_eq!((data["type"].as_u64(), data["name"].as_str()), (Some(3), Some("Reroll")));
        assert_eq!(data["target_id"], target.to_string());
        assert_eq!(data["options"], json!([]));
        assert_eq!(data["id"], "400000000000000004");
        assert_eq!(data["application_command"]["id"], "400000000000000004");
        assert!(commands.find(&place, command_type::CHAT, "reroll").is_empty());
        // A slash command has no target.
        let (roll, _) = commands.get(&place, command_type::CHAT, "400000000000000001").unwrap();
        assert!(roll.data(Vec::new(), None).get("target_id").is_none());
    }

    #[test]
    fn the_best_match_comes_first() {
        let commands = loaded(&index());
        // A name that starts with the text beats one that has it inside, which beats the app's name.
        let found = commands.find(&place(), command_type::CHAT, "stats");
        assert_eq!(names(&found), ["stats reset all", "stats show"]);
        let found = commands.find(&place(), command_type::CHAT, "some dice");
        assert_eq!(names(&found), ["roll"], "the description");
        let found = commands.find(&place(), command_type::CHAT, "dice");
        assert_eq!(
            names(&found),
            ["roll", "stats reset all", "stats show"],
            "the app's name counts for all of its commands"
        );
        let found = commands.find(&place(), command_type::CHAT, "stats re");
        assert_eq!(names(&found), ["stats reset all"]);
        assert!(commands.find(&place(), command_type::CHAT, "qqq").is_empty());
        // Letters in order, anywhere: the last resort.
        assert_eq!(names(&commands.find(&place(), command_type::CHAT, "rl")), ["roll", "stats reset all"]);
    }

    #[test]
    fn the_index_gives_what_the_launcher_pills_and_files_read() {
        let commands = loaded(&json!({
            "applications": [
                {"id": APP, "name": "Dicey", "description": "", "flags": (1u64 << 29) | (1 << 30),
                 "embedded_surfaces": [0], "bot": {"id": "300000000000000001", "username": "Dice Roller", "avatar": "abc"}},
                {"id": "200000000000000002", "name": "Zed", "description": "", "bot_id": "300000000000000002"},
            ],
            "application_commands": [
                {"id": "400000000000000001", "application_id": APP, "type": 1, "name": "roll", "description": "",
                 "global_popularity_rank": 1, "options": [
                    {"type": 7, "name": "where", "description": "", "channel_types": [0, 5]},
                    {"type": 11, "name": "file", "description": "", "file_types": ["image", ".pdf"]},
                    {"type": 3, "name": "note", "description": "", "file_types": [".txt"]}]},
                {"id": "400000000000000002", "application_id": "200000000000000002", "type": 1, "name": "zap",
                 "description": ""},
            ]
        }));
        let found = commands.find(&place(), command_type::CHAT, "");
        let (roll, dicey) = (found[0].command, found[0].app);
        assert_eq!((dicey.name.as_str(), dicey.app_name.as_str()), ("Dice Roller", "Dicey"));
        assert_eq!(dicey.bot_avatar.as_deref(), Some("abc"));
        assert!(dicey.partner() && dicey.promoted() && dicey.activity);
        let zed = found[1].app;
        assert!(!zed.partner() && !zed.promoted() && !zed.activity && zed.bot_avatar.is_none());
        assert_eq!((roll.popularity, found[1].command.popularity), (Some(1), None));
        assert_eq!(roll.root_id(), Some(Snowflake(400_000_000_000_000_001)));
        assert_eq!(roll.options[0].channel_types, Some(vec![0, 5]));
        assert_eq!(roll.options[1].file_types, ["image", ".pdf"]);
        assert!(roll.options[2].file_types.is_empty(), "only a file option takes file types");
        assert!(!roll.is_built_in() && roll.integration().is_none());
    }

    #[test]
    fn an_apps_command_comes_before_a_built_in_of_the_same_name() {
        let commands = loaded(&index());
        let base = commands.find(&place(), command_type::CHAT, "roll")[0].command.clone();
        let builtin = |b: BuiltIn, name: &str| Command {
            id: b.id().into(),
            name: name.into(),
            display_name: name.into(),
            description: String::new(),
            display_description: String::new(),
            options: Vec::new(),
            builtin: Some(b),
            ..base.clone()
        };
        let builtins = [builtin(BuiltIn::Shrug, "roll"), builtin(BuiltIn::Gif, "gif")];
        let query = |text| Query { kind: command_type::CHAT, text, offer: Offer::ALL, builtins: &builtins, used: None };
        let found = commands.query(&place(), query("roll"));
        assert_eq!(names(&found), ["roll", "roll", "stats reset all", "stats show"], "the app's description has roll");
        assert_eq!((found[0].command.builtin, found[1].command.builtin), (None, Some(BuiltIn::Shrug)));
        assert_eq!(found[1].app.name, "Built-In");
        let found = commands.query(&place(), query("gi"));
        assert_eq!(found[0].command.integration(), Some(Integration::Gif));
        // Without apps' commands, only the built-ins.
        let none = Offer { apps: false, builtins: BuiltIns::TextOnly };
        let found = commands.query(&place(), Query { offer: none, ..query("") });
        assert_eq!(names(&found), ["gif", "roll"]);
        // The menus' `find` lists no built-ins.
        assert_eq!(names(&commands.find(&place(), command_type::CHAT, "gi")), [""; 0]);
    }

    #[test]
    fn a_command_is_filed_under_its_id_or_its_server() {
        let server = Some(Snowflake(500));
        let global = Command::made_up("910", 1, 900, None);
        assert_eq!(usage_key(&global, server), "910");
        let own = Command::made_up("913", 1, 900, Some("500"));
        assert_eq!(usage_key(&own, server), "913:500");
        assert_eq!(usage_key(&own, None), "913", "in a DM");
        let sub = Command { id: "913\0show".into(), ..own.clone() };
        assert_eq!(usage_key(&sub, server), "913\0show:500");
        let builtin = Command { id: "-1".into(), builtin: Some(BuiltIn::Shrug), ..own };
        assert_eq!(usage_key(&builtin, server), "-1");
    }

    #[test]
    fn only_this_servers_and_global_uses_count_here() {
        let top = ["1", "2:500", "3:600", "-1"].map(String::from);
        assert_eq!(place_keys(&top, Some(Snowflake(500))), ["1", "2", "-1"]);
        assert_eq!(place_keys(&top, None), ["1", "-1"]);
    }

    fn used(scores: &[(&str, f64)], top: &[&str]) -> Used {
        Used {
            scores: scores.iter().map(|(k, s)| (k.to_string(), *s)).collect(),
            top: top.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn equal_matches_go_by_use() {
        let commands = loaded(&index());
        let query = |text, used| Query { kind: command_type::CHAT, text, offer: Offer::ALL, builtins: &[], used };
        // Both sub-commands of stats start with "stats": a tie the use breaks.
        let reset = "400000000000000002\0reset\0all";
        let more = used(&[(reset, 0.5), ("400000000000000002\0show", 0.2)], &[]);
        assert_eq!(names(&commands.query(&place(), query("stats", None))), ["stats reset all", "stats show"]);
        let less = used(&[("400000000000000002\0show", 0.5)], &[]);
        assert_eq!(names(&commands.query(&place(), query("stats", Some(&less)))), ["stats show", "stats reset all"]);
        assert_eq!(names(&commands.query(&place(), query("stats", Some(&more)))), ["stats reset all", "stats show"]);
        // A better match beats use: "roll" starts with "ro"; the stats are only Dicey's description ("rolls").
        let stats = used(&[("400000000000000002\0show", 9.0)], &[]);
        assert_eq!(names(&commands.query(&place(), query("ro", Some(&stats))))[0], "roll");
        // Nothing typed: by name, whatever is used.
        assert_eq!(names(&commands.query(&place(), query("", Some(&less)))), ["roll", "stats reset all", "stats show"]);
    }

    #[test]
    fn a_list_needs_the_use_when_it_opens_bare_or_finds_an_apps_command() {
        let commands = loaded(&index());
        assert!(loads_use(&commands, &place(), "", offer(false)), "the bare / browser");
        assert!(loads_use(&commands, &place(), "", offer(true)), "replying, too");
        assert!(loads_use(&commands, &place(), "ro", offer(false)));
        assert!(!loads_use(&commands, &place(), "qqq", offer(false)), "nothing matched");
        assert!(!loads_use(&commands, &place(), "ro", offer(true)), "no app's commands while replying");
    }

    #[test]
    fn frequently_used_goes_by_score_not_rank() {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "");
        let listed: Vec<&Command> = found.iter().map(|f| f.command).collect();
        let (roll, show) = ("400000000000000001", "400000000000000002\0show");
        // Roll ranks first (its frecency is higher), but show scores more.
        let use_of = used(&[(roll, 1.0), (show, 3.0)], &[roll, show]);
        let frequent = use_of.frequently_used(&listed, Some(GUILD_ID));
        assert_eq!(frequent.iter().map(|c| c.display_name.as_str()).collect::<Vec<_>>(), ["stats show", "roll"]);
        // Not used: not there. Ties keep the listed order.
        let tie = used(&[(roll, 1.0), (show, 1.0)], &[show, roll]);
        let frequent = tie.frequently_used(&listed, Some(GUILD_ID));
        assert_eq!(frequent.iter().map(|c| c.display_name.as_str()).collect::<Vec<_>>(), ["roll", "stats show"]);
        // A server's own command used in another server isn't offered here.
        let elsewhere = used(&[], &["400000000000000001:600"]);
        assert!(elsewhere.frequently_used(&listed, Some(GUILD_ID)).is_empty());
        // At most five.
        let many: Vec<Command> = (0..7).map(|n| Command::made_up(&n.to_string(), 1, 1, None)).collect();
        let many_listed: Vec<&Command> = many.iter().collect();
        let keys: Vec<String> = (0..7).map(|n| n.to_string()).collect();
        let all = Used { scores: keys.iter().map(|k| (k.clone(), 1.0)).collect(), top: keys };
        assert_eq!(all.frequently_used(&many_listed, None).len(), FREQUENTLY_USED);
        // Used but not offered here (a message command) isn't listed.
        let reroll = used(&[("400000000000000004", 5.0)], &["400000000000000004"]);
        assert!(reroll.frequently_used(&listed, Some(GUILD_ID)).is_empty());
    }

    #[test]
    fn typing_moves_on_from_the_name() {
        assert_eq!(query_of("roll"), ("roll".into(), false));
        assert_eq!(query_of("roll "), ("roll".into(), true));
        assert_eq!(query_of("roll count:5"), ("roll".into(), true));
        assert_eq!(query_of("count:5"), ("count".into(), true));
        assert_eq!(query_of("a b c d"), ("a b c".into(), true));
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "stats");
        let (text, done) = query_of("stats show ");
        assert_eq!(names(&settled(found, &text, done)), ["stats show"]);
        let found = commands.find(&place(), command_type::CHAT, "stats");
        assert_eq!(names(&settled(found.clone(), "stats", true)), ["stats reset all", "stats show"]);
        assert!(settled(found, "stat", true).is_empty(), "a space ends the name: it must be the whole of it");
    }

    #[test]
    fn a_reply_offers_no_apps_commands() {
        let commands = loaded(&index());
        assert_eq!(
            names(&suggestions(&commands, &place(), "sta", offer(false), &[], &Used::default())),
            ["stats reset all", "stats show"]
        );
        for typed in ["", "sta", "roll ", "dice"] {
            assert_eq!(
                names(&suggestions(&commands, &place(), typed, offer(true), &[], &Used::default())),
                [""; 0],
                "/{typed}"
            );
        }
        assert_eq!(offer(true), Offer { apps: false, builtins: BuiltIns::TextOnly });
    }

    #[test]
    fn a_reply_fetches_no_server_index() {
        const NOTHING: [Target; 0] = [];
        let now = Instant::now();
        let mut dm = place();
        dm.guild = None;
        dm.recipient = Some((Snowflake(1), true));
        for place in [place(), dm] {
            let mut commands = CommandIndex::default();
            assert_eq!(commands.to_fetch(&place, now, offer(true), ""), NOTHING, "a bare / fetches nothing");
            assert_eq!(commands.to_fetch(&place, now, offer(true), "ro"), [Target::User], "only the account's");
            assert_eq!(commands.to_fetch(&place, now, offer(true), "rol"), NOTHING, "under way");
            commands.loaded(Target::User, &json!({"applications": [], "application_commands": []}), ME);
            assert_eq!(commands.to_fetch(&place, now, offer(true), "roll"), NOTHING, "current");
        }
    }

    #[test]
    fn without_use_application_commands_only_a_bare_slash_fetches_the_servers_index() {
        let now = Instant::now();
        let mut place = place();
        place.base_access = false;
        let mut commands = CommandIndex::default();
        assert_eq!(commands.to_fetch(&place, now, Offer::ALL, "ro"), [Target::User]);
        let mut commands = CommandIndex::default();
        let both = [Target::Guild(GUILD_ID), Target::User];
        assert_eq!(commands.to_fetch(&place, now, Offer::ALL, ""), both, "the browser's fetch doesn't look");
    }

    #[test]
    fn a_thread_and_a_dm_without_a_bot_have_no_server_index() {
        let mut place = place();
        place.guild = None;
        place.recipient = Some((Snowflake(1), false));
        assert!(!place.has_context_index());
        place.recipient = Some((Snowflake(1), true));
        assert_eq!(place.context(), Some(CHANNEL));
        let mut commands = CommandIndex::default();
        let now = Instant::now();
        assert_eq!(commands.to_fetch(&place, now, Offer::ALL, ""), [Target::Channel(CHANNEL), Target::User]);
        assert!(commands.to_fetch(&place, now, Offer::ALL, "").is_empty(), "both are under way");
    }

    #[test]
    fn an_index_is_fetched_once_until_something_says_it_changed() {
        let mut commands = CommandIndex::default();
        let now = Instant::now();
        let guild = Target::Guild(GUILD_ID);
        assert!(commands.wants(guild, now));
        assert!(!commands.wants(guild, now), "being fetched");
        commands.loaded(guild, &index(), ME);
        assert!(!commands.wants(guild, now));
        commands.session_started();
        assert!(commands.wants(guild, now), "a new session");
        commands.loaded(guild, &index(), ME);
        // Discord names the version it has: the same one is no reason to fetch.
        commands.guild_changed(GUILD_ID, Some("7".into()));
        assert!(!commands.wants(guild, now));
        commands.guild_changed(GUILD_ID, Some("8".into()));
        assert!(commands.wants(guild, now));
        commands.loaded(guild, &index(), ME);
        commands.guild_changed(GUILD_ID, None);
        assert!(commands.wants(guild, now), "no version means stale");
    }

    #[test]
    fn an_index_without_a_version_stays_current() {
        let mut commands = CommandIndex::default();
        let now = Instant::now();
        let user = Target::User;
        assert!(commands.wants(user, now));
        commands.loaded(user, &json!({"applications": [], "application_commands": []}), ME);
        assert!(!commands.wants(user, now));
        commands.user_changed();
        assert!(commands.wants(user, now));
    }

    #[test]
    fn a_failed_fetch_waits_five_seconds() {
        let mut commands = CommandIndex::default();
        let now = Instant::now();
        assert!(commands.wants(Target::User, now));
        commands.failed(Target::User, now);
        assert!(!commands.wants(Target::User, now + Duration::from_secs(4)));
        assert!(commands.wants(Target::User, now + Duration::from_secs(5)));
    }

    #[test]
    fn a_refused_run_marks_only_the_indexes_with_the_app_stale() {
        let mut commands = loaded(&index());
        let now = Instant::now();
        commands.out_of_date(Snowflake(999), GUILD_ID);
        assert!(!commands.wants(Target::Guild(GUILD_ID), now));
        commands.out_of_date(Snowflake(APP.parse().unwrap()), GUILD_ID);
        assert!(commands.wants(Target::Guild(GUILD_ID), now));
    }

    #[test]
    fn a_servers_change_reports_the_bots_whose_dms_have_it_too() {
        let mut commands = loaded(&index());
        let bots = commands.guild_changed(GUILD_ID, None);
        assert_eq!(bots, [Snowflake(300_000_000_000_000_001), Snowflake(300_000_000_000_000_002)]);
    }

    fn with(command: Value) -> CommandIndex {
        let mut command = command;
        command["id"] = json!("400000000000000009");
        command["application_id"] = json!(APP);
        command["type"] = json!(1);
        command["name"] = json!("test");
        command["description"] = json!("Test");
        loaded(&json!({"applications": [{"id": APP, "name": "Dicey"}], "application_commands": [command]}))
    }

    fn count(commands: &CommandIndex, place: &Place) -> usize {
        commands.find(place, command_type::CHAT, "").len()
    }

    fn guild_of(everyone: Permissions) -> Guild {
        let mut guild: Guild = serde_json::from_value(json!({
            "id": "100000000000000010", "name": "G", "owner_id": "2",
            "roles": [{"id": "100000000000000010", "name": "@everyone", "permissions": everyone.0.to_string()}],
            "channels": [{"id": "100000000000000020", "type": 0}, {"id": "100000000000000021", "type": 11, "parent_id": "100000000000000020"}],
        }))
        .unwrap();
        guild.my_roles = vec![];
        guild
    }

    fn account() -> Account {
        Account { id: ME, verified: true, email: true, phone: false, mfa: false, nsfw_allowed: false }
    }

    #[test]
    fn a_place_follows_the_permissions_of_its_channel_and_a_threads_parent() {
        let seen = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::USE_APPLICATION_COMMANDS;
        let guild = guild_of(seen);
        let channel = guild.channel(CHANNEL).unwrap();
        let place = Place::of(Some(&guild), channel, &account(), NsfwRules::default(), 0);
        assert!(place.base_access && place.can_send && place.guild == Some(GUILD_ID));
        // A thread asks for Send Messages in Threads, which this server's members lack.
        let thread =
            guild.thread(Snowflake(100_000_000_000_000_021)).or(guild.channel(Snowflake(100_000_000_000_000_021)));
        let place = Place::of(Some(&guild), thread.unwrap(), &account(), NsfwRules::default(), 0);
        assert!(place.is_thread && place.parent == Some(CHANNEL));
        assert!(place.base_access && !place.can_send);
        // Without Use Application Commands the server's apps stay out of reach.
        let guild = guild_of(Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES);
        assert!(
            !Place::of(Some(&guild), guild.channel(CHANNEL).unwrap(), &account(), NsfwRules::default(), 0).base_access
        );
    }

    #[test]
    fn a_dm_is_a_place_with_every_permission_that_counts() {
        let dm: Channel = serde_json::from_value(json!({
            "id": "100000000000000020", "type": 1, "recipients": [{"id": "300000000000000001", "username": "bot", "bot": true}],
        }))
        .unwrap();
        let place = Place::of(None, &dm, &account(), NsfwRules::default(), 0);
        assert!(place.guild.is_none() && place.base_access && place.can_send);
        assert_eq!(place.recipient, Some((Snowflake(300_000_000_000_000_001), true)));
        let group: Channel = serde_json::from_value(json!({"id": "9", "type": 3})).unwrap();
        assert_eq!(Place::of(None, &group, &account(), NsfwRules::default(), 0).recipient, None);
    }

    #[test]
    fn dm_only_and_server_only_commands() {
        let mut dm = place();
        dm.guild = None;
        dm.recipient = Some((Snowflake(300_000_000_000_000_001), true));
        let mut group = dm.clone();
        group.recipient = Some((Snowflake(7), false));
        // No contexts: a server and the app's own DM, but not other DMs.
        let plain = with(json!({"dm_permission": true}));
        assert_eq!(count(&plain, &place()), 1);
        let mut private = loaded(&index());
        private.slots.insert(Key::Context(CHANNEL), private.slots[&Key::Context(GUILD_ID)].clone());
        assert_eq!(private.find(&dm, command_type::CHAT, "").len(), 3, "the app's own DM");
        assert_eq!(private.find(&group, command_type::CHAT, "").len(), 0, "other DMs have no apps of the server's");
        let no_dm = with(json!({"dm_permission": false}));
        assert_eq!(count(&no_dm, &place()), 1);
        // Contexts, when the command has them, decide alone.
        let server_only = with(json!({"contexts": [0]}));
        assert_eq!(count(&server_only, &place()), 1);
        let dm_only = with(json!({"contexts": [1, 2]}));
        assert_eq!(count(&dm_only, &place()), 0);
    }

    #[test]
    fn nsfw_commands_wait_for_an_account_that_may() {
        let nsfw = with(json!({"nsfw": true}));
        assert_eq!(count(&nsfw, &place()), 0);
        let mut allowed = place();
        allowed.allow_nsfw = true;
        assert_eq!(count(&nsfw, &allowed), 1);
    }

    #[test]
    fn nsfw_commands_are_offered_in_an_age_restricted_channel_to_an_adult() {
        let seen = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::USE_APPLICATION_COMMANDS;
        let mut guild = guild_of(seen);
        guild.channels[0].nsfw = true;
        let channel = guild.channel(CHANNEL).unwrap();
        let adult = Account { nsfw_allowed: true, ..account() };
        let place = Place::of(Some(&guild), channel, &adult, NsfwRules::default(), 0);
        assert_eq!(count(&with(json!({"nsfw": true})), &place), 1);
    }

    #[test]
    fn allow_nsfw_follows_the_channel_the_server_and_the_dm_setting() {
        let seen = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::USE_APPLICATION_COMMANDS;
        let adult = Account { nsfw_allowed: true, ..account() };
        let rules = |in_dms, by_nsfw_level| NsfwRules { in_dms, by_nsfw_level };
        let allowed = |guild: &Guild, id: Snowflake, me: &Account, nsfw| {
            let channel = guild.channel(id).or(guild.thread(id)).unwrap();
            Place::of(Some(guild), channel, me, nsfw, 0).allow_nsfw
        };
        let mut guild = guild_of(seen);
        guild.channels[0].nsfw = true;
        assert!(allowed(&guild, CHANNEL, &adult, rules(false, false)), "an nsfw channel");
        assert!(!allowed(&guild, CHANNEL, &account(), rules(true, true)), "not to an account Discord holds back");
        let thread = Snowflake(100_000_000_000_000_021);
        assert!(allowed(&guild, thread, &adult, rules(false, false)), "a thread under an nsfw parent");
        let mut plain = guild_of(seen);
        plain.nsfw_level = 3;
        assert!(allowed(&plain, CHANNEL, &adult, rules(false, true)), "an age-restricted server");
        assert!(!allowed(&plain, CHANNEL, &adult, rules(false, false)), "outside the experiment the owner decides");
        plain.owner_configured_content_level = 1;
        assert!(allowed(&plain, CHANNEL, &adult, rules(false, false)));
        plain.nsfw_level = 2;
        assert!(!allowed(&plain, CHANNEL, &adult, rules(false, true)), "SAFE");
        let dm: Channel = serde_json::from_value(json!({
            "id": "100000000000000020", "type": 1, "recipients": [{"id": "300000000000000001", "username": "bot", "bot": true}],
        }))
        .unwrap();
        let group: Channel = serde_json::from_value(json!({"id": "9", "type": 3})).unwrap();
        for private in [&dm, &group] {
            assert!(Place::of(None, private, &adult, rules(true, false), 0).allow_nsfw);
            assert!(!Place::of(None, private, &adult, rules(false, true), 0).allow_nsfw);
            assert!(!Place::of(None, private, &account(), rules(true, false), 0).allow_nsfw);
        }
    }

    #[test]
    fn nsfw_rules_from_the_setting_the_region_and_the_experiment() {
        use crate::experiments::{Experiments, murmur3};
        let assigned = json!({"apex_experiments": {"assignments": {"1": {ME.to_string(): {"assignments": [
            [murmur3("2025-09-server-nsfw-level"), 1, 0, 1]]}}}}});
        let (nitro, none, by_level) =
            (crate::limits::Nitro::default(), Experiments::default(), Experiments::from_ready(&assigned));
        let (off, on) = (Settings::default(), Settings { nsfw_commands_in_dms: true, ..Settings::default() });
        let (open, gated) = (Gates::default(), Gates { nsfw_commands_age_gated: true, ..Gates::default() });
        let rules = |settings: &Settings, gates: &Gates, experiments: &Experiments| {
            NsfwRules::new(settings, gates, &Limits { nitro: &nitro, experiments, me: ME })
        };
        assert_eq!(rules(&off, &open, &none), NsfwRules::default());
        assert!(rules(&on, &open, &none).in_dms);
        assert!(!rules(&on, &gated, &none).in_dms, "the region holds it back");
        assert!(rules(&off, &open, &by_level).by_nsfw_level);
    }

    #[test]
    fn the_apps_menus_hide_nsfw_user_and_message_commands_too() {
        for kind in [command_type::USER, command_type::MESSAGE] {
            let mut command = json!({"id": "400000000000000009", "application_id": APP, "type": kind,
                "name": "Spicy", "description": "", "nsfw": true});
            let commands = loaded(&json!({"applications": [{"id": APP, "name": "Dicey"}],
                "application_commands": [command.take()]}));
            assert!(commands.find(&place(), kind, "").is_empty(), "type {kind}");
            let mut allowed = place();
            allowed.allow_nsfw = true;
            assert_eq!(commands.find(&allowed, kind, "").len(), 1, "type {kind}");
        }
    }

    #[test]
    fn default_member_permissions_and_overwrites_decide_who_sees_a_command() {
        let moderators = with(json!({"default_member_permissions": "8192"}));
        assert_eq!(count(&moderators, &place()), 0, "no Manage Messages");
        let mut manager = place();
        manager.permissions = manager.permissions | Permissions::MANAGE_MESSAGES;
        assert_eq!(count(&moderators, &manager), 1);
        let nobody = with(json!({"default_member_permissions": "0"}));
        assert_eq!(count(&nobody, &manager), 0, "0 is administrators only");
        let mut admin = place();
        admin.permissions = Permissions::ADMINISTRATOR;
        assert_eq!(count(&nobody, &admin), 1);
        // A rule for our role beats the default.
        let roled = with(json!({"default_member_permissions": "8192",
            "permissions": {"roles": {"500000000000000001": true}}}));
        let mut member = place();
        member.roles = vec![Snowflake(500_000_000_000_000_001)];
        assert_eq!(count(&roled, &member), 1);
        // A rule against us beats it too, and a rule for the channel counts first.
        let denied = with(json!({"permissions": {"user": false}}));
        assert_eq!(count(&denied, &member), 0);
        let channel_off = with(json!({"permissions": {"channels": {"100000000000000020": false}}}));
        assert_eq!(count(&channel_off, &member), 0);
        let all_channels_off = with(json!({"permissions": {"channels": {"100000000000000009": false}}}));
        assert_eq!(count(&all_channels_off, &member), 0, "the server's id minus one is every channel");
    }

    #[test]
    fn missing_use_application_commands_hides_an_apps_commands_from_the_server() {
        let commands = with(json!({}));
        let mut place = place();
        place.base_access = false;
        assert_eq!(count(&commands, &place), 0);
    }

    #[test]
    fn user_installed_apps_work_where_the_server_has_not_got_them() {
        let mut commands = CommandIndex::default();
        commands.wants(Target::User, Instant::now());
        commands.loaded(
            Target::User,
            &json!({"applications": [{"id": APP, "name": "Mine"}], "application_commands": [
                {"id": "400000000000000009", "application_id": APP, "type": 1, "name": "hi", "description": "Hi",
                 "contexts": [0, 1, 2], "integration_types": [1]}]}),
            ME,
        );
        let mut elsewhere = place();
        elsewhere.base_access = false;
        elsewhere.can_send = false;
        assert_eq!(commands.find(&elsewhere, command_type::CHAT, "").len(), 1, "no Use Application Commands needed");
    }

    #[test]
    fn what_the_client_sends_back_of_a_command() {
        let commands = loaded(&json!({
            "applications": [{"id": APP, "name": "Dicey"}],
            "application_commands": [{
                "id": "400000000000000001", "application_id": APP, "version": "9", "type": 1,
                "name": "rola", "name_default": "roll", "description": "Tira", "description_default": "Roll",
                "dm_permission": true, "guild_id": "100000000000000010",
                "permissions": {"user": true, "roles": {"7": false}},
                "options": [{"type": 3, "name": "size", "name_default": "size", "description": "d", "description_default": "d",
                    "choices": [{"name": "Seis", "name_default": "Six", "value": "6"}]}]
            }]
        }));
        let found = commands.find(&place(), command_type::CHAT, "");
        let command = found[0].command;
        assert_eq!(command.display_name, "rola");
        assert_eq!(command.name, "roll");
        let data = command.data(vec![], None);
        assert_eq!(data["name"], "roll");
        assert_eq!(data["id"], "400000000000000001");
        assert_eq!(data["version"], "9");
        assert_eq!(data["guild_id"], "100000000000000010");
        let root = &data["application_command"];
        // The English name and description are kept, and what the account's language
        // calls them is noted next to them.
        assert_eq!((&root["name"], &root["name_localized"]), (&json!("roll"), &json!("rola")));
        assert_eq!((&root["description"], &root["description_localized"]), (&json!("Roll"), &json!("Tira")));
        assert_eq!(
            root["options"][0]["choices"][0],
            json!({"name": "Six", "name_default": "Six", "name_localized": "Seis", "value": "6"})
        );
        assert!(root["options"][0].get("name_localized").is_none(), "unchanged names note nothing");
        // Rules become a list: ours, channels, roles.
        assert_eq!(
            root["permissions"],
            json!([{"type": 2, "id": "100000000000000001", "permission": true},
                   {"type": 1, "id": "7", "permission": false}])
        );
        assert!(data.get("target_id").is_none());
        assert_eq!(command.data(vec![], Some(Snowflake(5)))["target_id"], "5");
    }

    #[test]
    fn a_command_without_a_default_name_notes_a_translation_that_is_not_one() {
        // The client compares with `!==`: nothing to compare to counts as different.
        let commands = loaded(&json!({
            "applications": [{"id": APP, "name": "Dicey"}],
            "application_commands": [{"id": "1", "application_id": APP, "type": 1, "name": "roll", "description": "Roll"}]
        }));
        let root = commands.find(&place(), command_type::CHAT, "")[0].command.data(vec![], None)["application_command"]
            .clone();
        assert_eq!(root["name"], "roll");
        assert_eq!(root["name_localized"], "roll");
        assert_eq!(root["description_localized"], "Roll");
        assert_eq!(root["options"], json!([]));
        assert!(root.get("dm_permission").is_none() && root.get("permissions").is_none());
    }

    fn run(command: &str, values: &[(&str, &str)]) -> Result<Vec<Value>, String> {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "");
        let command = found.iter().find(|f| f.command.name == command).unwrap().command;
        let values: Values = values.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        run_options(command, &values, &Known, &Autocompleted::default(), None)
    }

    #[test]
    fn options_go_out_with_their_values_as_typed_for_their_type() {
        assert_eq!(
            run("roll", &[("count", " 3 "), ("size", "D20")]).unwrap(),
            [json!({"type": 4, "name": "count", "value": 3}), json!({"type": 3, "name": "size", "value": "20"})]
        );
        assert_eq!(run("roll", &[("count", "1,000")]).unwrap(), [json!({"type": 4, "name": "count", "value": 1000})]);
        assert_eq!(
            run("stats show", &[("who", "@ann")]).unwrap(),
            [
                json!({"type": 1, "name": "show", "options": [{"type": 6, "name": "who", "value": "600000000000000001"}]})
            ]
        );
        assert_eq!(
            run("stats reset all", &[]).unwrap(),
            [json!({"type": 2, "name": "reset", "options": [{"type": 1, "name": "all", "options": []}]})]
        );
    }

    #[test]
    fn a_name_nobody_has_stops_the_run() {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "stats show");
        let command = found[0].command;
        let values: Values = [("who".to_owned(), "@nobody".to_owned())].into();
        let error = run_options(command, &values, &Nobody, &Autocompleted::default(), None).unwrap_err();
        assert!(error.contains("@nobody"));
        let option = &command.options[0];
        assert_eq!(
            validate(option, Some("@nobody"), false, &Nobody, &Autocompleted::default()),
            Err("Not a valid user.".into())
        );
        assert_eq!(validate(option, Some("@ann"), false, &Known, &Autocompleted::default()), Ok(()));
        assert_eq!(
            validate(option, Some("600000000000000001"), false, &Nobody, &Autocompleted::default()),
            Ok(()),
            "an id is always fine"
        );
        assert_eq!(
            validate(option, Some(""), false, &Known, &Autocompleted::default()),
            Err("This option is required. Specify a value.".into())
        );
    }

    #[test]
    fn roles_channels_and_everyone() {
        use option_type::*;
        assert_eq!(resolve(ROLE, "@mods", &Known), Some(Snowflake(600_000_000_000_000_002)));
        assert_eq!(resolve(ROLE, "@everyone", &Known), Some(GUILD_ID));
        assert_eq!(resolve(MENTIONABLE, "@ann", &Known), Some(Snowflake(600_000_000_000_000_001)));
        assert_eq!(resolve(MENTIONABLE, "@mods", &Known), Some(Snowflake(600_000_000_000_000_002)));
        assert_eq!(resolve(CHANNEL, "#general", &Known), Some(Snowflake(600_000_000_000_000_003)));
        assert_eq!(resolve(CHANNEL, "general", &Known), None, "a channel needs its #");
        assert_eq!(resolve(USER, "@mods", &Known), None, "a role isn't a user");
    }

    fn option(kind: u8) -> CommandOption {
        CommandOption {
            name: "x".into(),
            display_name: "x".into(),
            server_name: None,
            description: String::new(),
            kind,
            required: false,
            choices: None,
            autocomplete: false,
            min_value: None,
            max_value: None,
            min_length: None,
            max_length: None,
            channel_types: None,
            file_types: Vec::new(),
        }
    }

    #[test]
    fn values_are_checked_as_the_client_checks_them() {
        use option_type::*;
        let auto = Autocompleted::default();
        let check = |o: &CommandOption, t: &str| validate(o, Some(t), false, &Nobody, &auto);
        assert_eq!(check(&option(BOOLEAN), "TRUE"), Ok(()));
        assert_eq!(check(&option(BOOLEAN), "yes"), Err("Pick either true or false.".into()));
        let mut count = option(INTEGER);
        (count.min_value, count.max_value) = (Some(1.0), Some(10.0));
        assert_eq!(check(&count, "5"), Ok(()));
        assert_eq!(check(&count, "5.5"), Err("Input a valid integer.".into()));
        assert_eq!(check(&count, "11"), Err("Enter a number between 1 and 10.".into()));
        assert_eq!(check(&count, "0"), Err("Enter a number between 1 and 10.".into()));
        let mut ratio = option(NUMBER);
        ratio.min_value = Some(0.5);
        assert_eq!(check(&ratio, "0.25"), Err("Enter a number greater than or equal to 0.5.".into()));
        assert_eq!(check(&ratio, "abc"), Err("Input a valid number.".into()));
        (ratio.min_value, ratio.max_value) = (None, Some(10.0));
        assert_eq!(check(&ratio, "11"), Err("Enter a number less than or equal to 10.".into()));
        let mut text = option(STRING);
        (text.min_length, text.max_length) = (Some(2), Some(4));
        assert_eq!(check(&text, "a"), Err("Enter between 2 and 4 characters.".into()));
        assert_eq!(check(&text, "abcd"), Ok(()));
        text.min_length = Some(4);
        assert_eq!(check(&text, "abc"), Err("Enter 4 characters.".into()));
        text.min_length = None;
        assert_eq!(check(&text, "abcde"), Err("Enter 4 or fewer characters.".into()));
        (text.min_length, text.max_length) = (Some(2), None);
        assert_eq!(check(&text, "a"), Err("Enter 2 or more characters.".into()));
        let mut picked = option(STRING);
        picked.choices = Some(vec![Choice { name: "One".into(), value: json!("1") }]);
        assert_eq!(check(&picked, "One"), Ok(()));
        assert_eq!(check(&picked, "1"), Err("Not a valid choice.".into()), "the name is what is typed");
    }

    #[test]
    fn a_missing_option_an_empty_one_and_one_not_yet_left() {
        let auto = Autocompleted::default();
        let check = |o: &CommandOption, content, allow_empty| validate(o, content, allow_empty, &Nobody, &auto);
        let mut count = option(option_type::INTEGER);
        assert_eq!(check(&count, None, false), Ok(()), "an optional option without a pill");
        assert_eq!(check(&count, Some(" "), false), Err("Input a valid integer.".into()), "empty, as its type");
        assert_eq!(check(&count, Some(""), true), Ok(()), "as a command starts");
        count.required = true;
        assert_eq!(check(&count, None, false), Err("This option is required. Specify a value.".into()));
        assert_eq!(check(&count, Some(""), false), Err("This option is required. Specify a value.".into()));
        assert_eq!(check(&count, Some(""), true), Ok(()));
        let mut size = option(option_type::STRING);
        size.choices = Some(vec![Choice { name: "D6".into(), value: json!("6") }]);
        assert_eq!(check(&size, Some(""), false), Err("Not a valid choice.".into()), "choices before the type");
        for (kind, words) in [
            (option_type::BOOLEAN, "Pick either true or false."),
            (option_type::CHANNEL, "Not a valid channel."),
            (option_type::NUMBER, "Input a valid number."),
            (option_type::ROLE, "Not a valid role."),
            (option_type::USER, "Not a valid user."),
            (option_type::MENTIONABLE, "Not a valid user or role."),
            (option_type::STRING, "Not a valid value."),
        ] {
            assert_eq!(type_error(&option(kind)), words);
        }
    }

    #[test]
    fn a_picked_mention_names_its_id() {
        use option_type::*;
        let id = Some(Snowflake(600_000_000_000_000_009));
        assert_eq!(resolve(USER, "<@600000000000000009>", &Nobody), id);
        assert_eq!(resolve(USER, "<@!600000000000000009>", &Nobody), id);
        assert_eq!(resolve(ROLE, "<@&600000000000000009>", &Nobody), id);
        assert_eq!(resolve(MENTIONABLE, "<@600000000000000009>", &Nobody), id);
        assert_eq!(resolve(MENTIONABLE, "<@&600000000000000009>", &Nobody), id);
        assert_eq!(resolve(CHANNEL, "<#600000000000000009>", &Nobody), id);
        assert_eq!(resolve(USER, "<@&600000000000000009>", &Nobody), None, "a role isn't a user");
        assert_eq!(resolve(ROLE, "<@600000000000000009>", &Nobody), None);
        assert_eq!(resolve(CHANNEL, "<@600000000000000009>", &Nobody), None);
        assert_eq!(resolve(USER, "<@nope>", &Nobody), None);
        let auto = Autocompleted::default();
        assert_eq!(validate(&option(USER), Some("<@600000000000000009>"), false, &Nobody, &auto), Ok(()));
    }

    #[test]
    fn choices_match_as_the_client_scores_them() {
        let choices: Vec<Choice> = ["apple pie", "Pineapple", "apple", "grape", "pie apple", "papaya"]
            .into_iter()
            .map(|n| Choice { name: n.into(), value: json!(n) })
            .collect();
        let names = |query| match_choices(&choices, query).into_iter().map(|c| c.name.as_str()).collect::<Vec<_>>();
        // Exact 10, starts 7, contains 5, each word 3, letters in order 1; equals in order.
        assert_eq!(names("apple"), ["apple", "apple pie", "Pineapple", "pie apple"]);
        assert_eq!(names("APPLE PIE"), ["apple pie", "pie apple"]);
        assert_eq!(names("pp"), ["apple pie", "Pineapple", "apple", "pie apple", "papaya"], "papaya by its letters");
        assert_eq!(names("ppy"), ["papaya"]);
        assert_eq!(names(""), ["apple pie", "Pineapple", "apple", "grape", "pie apple", "papaya"], "all, in order");
        assert_eq!(boolean_choices().map(|c| c.name), ["True", "False"]);
    }

    #[test]
    fn the_first_row_is_selected_as_a_list_shows_and_again_for_a_new_query() {
        let mut selection = Selection::default();
        selection.listed("choices", Focus::Auto, "", true, 0);
        assert_eq!(selection.selected(), None, "none while loading");
        selection.listed("choices", Focus::Auto, "", false, 3);
        assert_eq!(selection.selected(), Some(0));
        selection.moved(1, 3);
        selection.listed("choices", Focus::Auto, "", false, 3);
        assert_eq!(selection.selected(), Some(1), "kept while only the rows change");
        selection.listed("choices", Focus::Auto, "", false, 1);
        assert_eq!(selection.selected(), Some(0), "clamped");
        selection.listed("choices", Focus::Auto, "a", false, 1);
        assert_eq!(selection.selected(), Some(0), "a new query");
        // The options list waits for typing.
        selection.listed("options", Focus::WhenFiltered, "", false, 2);
        assert_eq!(selection.selected(), None);
        assert_eq!(selection.key(true, 2), None, "Enter is the box's");
        assert_eq!(selection.key(false, 2), Some(Pick::Selected), "Tab selects the first");
        assert_eq!(selection.selected(), Some(0));
        selection.listed("options", Focus::WhenFiltered, "c", false, 2);
        assert_eq!(selection.selected(), Some(0));
    }

    #[test]
    fn moving_wraps_and_enter_sends_while_tab_inserts() {
        let mut selection = Selection::default();
        selection.listed("gifs", Focus::Auto, "cat", false, 3);
        selection.moved(-1, 3);
        assert_eq!(selection.selected(), Some(2), "up from the first is the last");
        selection.moved(1, 3);
        assert_eq!(selection.selected(), Some(0), "down from the last is the first");
        assert_eq!(selection.key(true, 3), Some(Pick::Send(0)));
        assert_eq!(selection.key(false, 3), Some(Pick::Insert(0)));
        assert_eq!(selection.key(true, 0), None, "a row that isn't there");
        selection.hovered();
        assert_eq!(selection.key(true, 3), None);
        assert_eq!(selection.key(false, 3), None, "Tab takes nothing either");
        selection.moved(1, 3);
        assert_eq!(selection.selected(), Some(0));
    }

    #[test]
    fn a_command_typed_whole_becomes_active() {
        let commands = loaded(&index());
        let typed = |text: &str| {
            let (query, terminated) = query_of(text);
            let found = commands.find(&place(), command_type::CHAT, &query);
            typed_command(found, &query, terminated).map(|c| c.display_name.clone())
        };
        assert_eq!(typed("roll ").as_deref(), Some("roll"));
        assert_eq!(typed("roll"), None, "not ended");
        assert_eq!(typed("stats "), None, "two sub-commands start with it");
        assert_eq!(typed("stats show ").as_deref(), Some("stats show"));
        assert_eq!(typed("roll count:3").as_deref(), Some("roll"));
        assert_eq!(typed("roll 3 size:D6"), None, "roll 3 names nothing");
    }

    #[test]
    fn lengths_count_as_javascript_counts() {
        let auto = Autocompleted::default();
        let mut text = option(option_type::STRING);
        text.max_length = Some(4);
        assert_eq!(
            validate(&text, Some("😀😀😀"), false, &Nobody, &auto),
            Err("Enter 4 or fewer characters.".into()),
            "three emoji are six UTF-16 units"
        );
        (text.min_length, text.max_length) = (Some(2), None);
        assert_eq!(validate(&text, Some("😀"), false, &Nobody, &auto), Ok(()));
    }

    #[test]
    fn limits_read_as_the_client_writes_them() {
        // `toLocaleString` without grouping: at most three decimals, from the shortest form.
        assert_eq!(plain(10.0), "10");
        assert_eq!(plain(2.5), "2.5");
        assert_eq!(plain(0.12345), "0.123");
        assert_eq!(plain(1.0005), "1.001");
        assert_eq!(plain(999.9999), "1000");
        assert_eq!(plain(0.1 + 0.2), "0.3");
        assert_eq!(plain(-0.0001), "-0");
        assert_eq!(plain(-2.5), "-2.5");
        let mut ratio = option(option_type::NUMBER);
        ratio.min_value = Some(0.12345);
        assert_eq!(
            validate(&ratio, Some("0"), false, &Nobody, &Autocompleted::default()),
            Err("Enter a number greater than or equal to 0.123.".into())
        );
    }

    #[test]
    fn the_focused_option_goes_out_as_typed() {
        let commands = with(json!({"options": [
            {"type": 3, "name": "fruit", "description": "Which", "required": true, "autocomplete": true},
            {"type": 4, "name": "count", "description": "How many", "autocomplete": true},
        ]}));
        let found = commands.find(&place(), command_type::CHAT, "");
        let command = found[0].command;
        let values: Values = [("fruit".to_owned(), "ap ".to_owned()), ("count".to_owned(), " 3 ".to_owned())].into();
        let asking = |focused| run_options(command, &values, &Nobody, &Autocompleted::default(), Some(focused));
        assert_eq!(
            asking("fruit").unwrap(),
            [
                json!({"type": 3, "name": "fruit", "value": "ap ", "focused": true}),
                json!({"type": 4, "name": "count", "value": 3}),
            ]
        );
        assert_eq!(
            asking("count").unwrap(),
            [
                json!({"type": 3, "name": "fruit", "value": "ap"}),
                json!({"type": 4, "name": "count", "value": " 3 ", "focused": true}),
            ]
        );
    }

    #[test]
    fn numbers_go_out_as_javascript_would_write_them() {
        assert_eq!(number_json(3.0), json!(3));
        assert_eq!(number_json(2.5), json!(2.5));
        assert_eq!(number("1,234.5"), Some(1234.5));
        assert_eq!(number("abc"), None);
        assert_eq!(number("inf"), None);
    }

    #[test]
    fn running_a_command_sends_what_the_client_sends() {
        let commands = loaded(&index());
        let found = commands.find(&place(), command_type::CHAT, "roll");
        let command = found[0].command;
        let data = command.data(vec![json!({"type": 4, "name": "count", "value": 2})], None);
        let run = Run {
            application: command.application,
            guild: Some(GUILD_ID),
            channel: CHANNEL,
            session: "session",
            data,
            nonce: Snowflake(9),
        };
        let body = run.body(&[]);
        assert_eq!(body["type"], 2);
        assert_eq!(body["application_id"], APP);
        assert_eq!(body["guild_id"], "100000000000000010");
        assert_eq!(body["channel_id"], "100000000000000020");
        assert_eq!(body["session_id"], "session");
        assert_eq!(body["nonce"], "9");
        assert_eq!(body["analytics_location"], "slash_ui");
        assert!(body.get("section_name").is_none() && body.get("source").is_none());
        let mut keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["analytics_location", "application_id", "channel_id", "data", "guild_id", "nonce", "session_id", "type"]
        );
        let mut keys: Vec<&str> = body["data"].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["application_command", "attachments", "guild_id", "id", "name", "options", "type", "version"]
                .iter()
                .filter(|k| **k != "guild_id")
                .copied()
                .collect::<Vec<_>>()
        );
        let dm = Run { guild: None, ..run };
        assert!(dm.body(&[]).get("guild_id").is_none(), "a DM has none, not null");
        let asking = dm.autocomplete_body();
        assert_eq!(asking["type"], 4);
        assert!(asking.get("analytics_location").is_none());
    }

    #[test]
    fn a_run_names_its_files_even_without_any() {
        let commands = loaded(&index());
        let place = place();
        let run = |command: &Command, target| Run {
            application: command.application,
            guild: Some(GUILD_ID),
            channel: CHANNEL,
            session: "session",
            data: command.data(Vec::new(), target),
            nonce: Snowflake(9),
        };
        let (roll, _) = commands.get(&place, command_type::CHAT, "400000000000000001").unwrap();
        assert_eq!(run(roll, None).body(&[])["data"]["attachments"], json!([]));
        let (reroll, _) = commands.get(&place, command_type::MESSAGE, "400000000000000004").unwrap();
        assert_eq!(run(reroll, Some(Snowflake(7))).body(&[])["data"]["attachments"], json!([]));
        assert!(run(roll, None).autocomplete_body()["data"].get("attachments").is_none());
        let file = Sent {
            filename: "a.png",
            uploaded_filename: "u/a.png",
            description: None,
            spoiler: false,
            content_type: "image/png",
            voice: None,
        };
        assert_eq!(
            run(roll, None).body(&[file])["data"]["attachments"],
            json!([{"id": "0", "filename": "a.png", "uploaded_filename": "u/a.png", "original_content_type": "image/png"}])
        );
    }

    #[test]
    fn autocomplete_asks_once_per_query_and_remembers_the_answers() {
        let mut auto = Autocomplete::default();
        assert!(auto.request(Snowflake(1), "city", "pa", ""));
        assert!(!auto.request(Snowflake(2), "city", "pa", ""), "the same query again");
        assert!(auto.request(Snowflake(3), "city", "par", ""));
        // The first request was replaced by the second, so its answer is nobody's.
        assert_eq!(auto.answer(Snowflake(1), &json!([]), |_| false), None);
        let answer = json!([{"name": "Paris", "value": "paris"}]);
        assert_eq!(auto.answer(Snowflake(3), &answer, |_| false), Some("city".into()));
        assert_eq!(auto.answers().value_of("city", "Paris"), Some(&json!("paris")));
        assert_eq!(auto.answers().choices("city", "par").map(<[Choice]>::len), Some(1));
        // Back to a query asked before, whose answer never came: asked again.
        assert!(auto.request(Snowflake(4), "city", "pa", ""));
    }

    #[test]
    fn a_change_in_the_other_options_forgets_the_answers() {
        let mut auto = Autocomplete::default();
        assert!(auto.request(Snowflake(1), "city", "p", "country=fr\0"));
        auto.answer(Snowflake(1), &json!([{"name": "Paris", "value": "paris"}]), |_| false);
        assert!(auto.request(Snowflake(2), "city", "p", "country=de\0"), "same query, different country");
        assert!(auto.answers().choices("city", "p").is_none());
    }

    #[test]
    fn a_failed_autocomplete_is_ours_only_if_we_asked() {
        let mut auto = Autocomplete::default();
        assert_eq!(auto.failed(Snowflake(1)), None);
        auto.request(Snowflake(1), "city", "p", "");
        assert_eq!(auto.failed(Snowflake(1)).as_deref(), Some("city"));
        assert!(auto.errored);
        assert!(auto.request(Snowflake(2), "city", "pa", ""));
        assert!(!auto.errored, "asking again clears it");
    }

    #[test]
    fn integer_choices_from_an_app_are_numbers() {
        let mut auto = Autocomplete::default();
        auto.request(Snowflake(1), "n", "1", "");
        auto.answer(Snowflake(1), &json!([{"name": "One", "value": "1"}]), |_| true);
        assert_eq!(auto.answers().value_of("n", "One"), Some(&json!(1)));
    }

    #[test]
    fn names_sort_the_way_people_count() {
        let mut names = ["item 10", "Item 2", "item 1"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["item 1", "Item 2", "item 10"]);
    }

    #[test]
    fn a_timeout_reads_as_the_client_reads_it() {
        assert_eq!(reason_text(Some(2), Some("Dicey")).as_deref(), Some("Dicey didn't respond in time"));
        assert_eq!(reason_text(Some(2), None).as_deref(), Some("The application didn't respond in time"));
        assert_eq!(reason_text(Some(1), Some("Dicey")), None);
        assert_eq!(reason_text(None, None), None);
    }
}
