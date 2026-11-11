//! The quick switcher: which servers, channels, DMs and people a query finds,
//! and what the box offers before anything is typed.
//!
//! It follows the official client's `QuickSwitcherStore` (174768), its
//! search (290676), the ranking in `AutocompleteUtils` (997625) and the
//! worker that ranks people (787578). Everything is decided from what we
//! already hold, so it sends nothing.
//!
//! The client also ranks by frecency (the channels and servers you open most),
//! which comes from the settings protobuf Neucord doesn't fetch, so every
//! frecency term is zero here. Applications, game profiles, links and the
//! client's own pages aren't things Neucord has, so those results are never
//! offered. Letters are folded for comparison by lower-casing and dropping
//! combining marks; the client also maps look-alike characters to one
//! skeleton, which is left out.

use std::cmp::Ordering;

use crate::relationships::{Kind as RelationshipKind, Relationships};
use crate::{Channel, ChannelKind, Guild, Snowflake, User, threads};

/// How many results of each kind a plain query shows.
const LIMIT: usize = 5;
/// How many results of each kind a query narrowed by a prefix shows, and a
/// plain one when a single kind has any.
const LIMIT_NARROWED: usize = 100;
/// Channels remembered for "Previous Channels".
pub const HISTORY: usize = 8;

/// What a prefix narrows a query to (`AT` in 926140, and `k` in 793322).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `@`: people, friends first.
    Users,
    /// `@@`: everyone we know of.
    AllUsers,
    /// `#`
    Channels,
    /// `!`
    Voice,
    /// `*`
    Servers,
    /// `$`: game profiles, which Neucord has none of.
    Games,
}

impl Mode {
    /// What the box says it is searching, above the results.
    pub fn header(self, guild: Option<&str>) -> String {
        match (self, guild) {
            (Mode::Users, Some(name)) => format!("Search friends and users in: {name}"),
            (Mode::Users | Mode::AllUsers, _) => "Searching All Users".to_owned(),
            (Mode::Channels, _) => "Searching Text Channels".to_owned(),
            (Mode::Voice, _) => "Searching Voice Channels".to_owned(),
            (Mode::Servers, _) => "Searching Servers".to_owned(),
            (Mode::Games, _) => "Searching Game Profiles".to_owned(),
        }
    }
}

/// What was typed: the query and the prefix that narrowed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub mode: Option<Mode>,
}

/// Split what was typed into a prefix and a query (`k` in 793322): the mode
/// follows the first character, and the first prefix character found
/// anywhere is cut out, so `foo#bar` searches `foobar` everywhere.
pub fn parse(input: &str) -> Query {
    let mut chars = input.chars();
    let first = chars.next();
    if first == Some('@') && chars.next() == Some('@') {
        return Query { text: input.chars().skip(2).collect(), mode: Some(Mode::AllUsers) };
    }
    let mode = match first {
        Some('@') => Some(Mode::Users),
        Some('#') => Some(Mode::Channels),
        Some('!') => Some(Mode::Voice),
        Some('*') => Some(Mode::Servers),
        Some('$') => Some(Mode::Games),
        _ => None,
    };
    // `^@|#|!|\*|\$`: an `@` only counts at the start.
    let cut = input.char_indices().find(|&(i, c)| matches!(c, '#' | '!' | '*' | '$') || (c == '@' && i == 0));
    let text = match cut {
        Some((i, c)) => format!("{}{}", &input[..i], &input[i + c.len_utf8()..]),
        None => input.to_owned(),
    };
    Query { text, mode }
}

/// What a result opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A person, whose DM opens.
    User,
    GroupDm,
    /// A text-like channel or thread.
    Channel,
    Voice,
    Server,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub kind: Kind,
    /// The user's, channel's or server's.
    pub id: Snowflake,
    pub score: f64,
    /// The name shown, and what ties sort on.
    pub comparator: String,
    pub sortable: Option<String>,
}

/// A line of the list: a heading, or something to open.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(String),
    Hit(Hit),
}

/// Everything the switcher searches, borrowed from the session.
pub struct Sources<'a> {
    pub me: Snowflake,
    /// The open server.
    pub guild: Option<Snowflake>,
    /// The open channel.
    pub channel: Option<Snowflake>,
    pub guilds: &'a [Guild],
    /// DMs and group DMs.
    pub private_channels: &'a [Channel],
    pub relationships: &'a Relationships,
    /// Whether a server's channel or thread shows to us (the client's
    /// `accessPermissions`: View Channel, and Connect for a call).
    pub can_access: &'a dyn Fn(&Guild, &Channel) -> bool,
    /// Whether we know `user` as a member of the server.
    pub is_member: &'a dyn Fn(Snowflake, Snowflake) -> bool,
}

/// What the box offers before anything is typed, besides the servers and
/// channels themselves. The session knows what is unread and muted, so it
/// lists the ids and this orders and trims them.
#[derive(Default)]
pub struct Home<'a> {
    /// Channels opened, newest first.
    pub history: &'a [Snowflake],
    /// Channels with mentions, oldest first.
    pub mentions: &'a [Snowflake],
    /// The open server's channels, then threads, we haven't read and that
    /// notify for every message, unless muted.
    pub unread: &'a [Snowflake],
    /// Who spoke in the open channel, newest first.
    pub speakers: &'a [User],
}

impl Sources<'_> {
    /// A person's name here: what we call a friend, else the display name.
    pub fn user_name(&self, user: &User) -> String {
        match self.relationships.get(user.id).and_then(|r| r.nickname.as_deref()) {
            Some(nickname) => nickname.to_owned(),
            None => user.display_name().to_owned(),
        }
    }

    /// A channel's name as the box lists it (`getChannelNameForDisplay`,
    /// 47167): a DM is the person, a group its name or its people.
    pub fn channel_name(&self, channel: &Channel) -> String {
        match channel.kind {
            ChannelKind::Dm => match channel.name.as_deref().filter(|n| !n.is_empty()) {
                Some(name) => name.to_owned(),
                None => channel.recipients.first().map_or_else(|| "???".to_owned(), |u| self.user_name(u)),
            },
            ChannelKind::GroupDm => match channel.name.as_deref().filter(|n| !n.is_empty()) {
                Some(name) => name.to_owned(),
                None => channel.recipients.iter().map(|u| self.user_name(u)).collect::<Vec<_>>().join(", "),
            },
            _ => channel.name.clone().unwrap_or_default(),
        }
    }

    fn guild(&self, id: Snowflake) -> Option<&Guild> {
        self.guilds.iter().find(|g| g.id == id)
    }

    /// A server's channels and threads we've joined, as the search walks them.
    fn guild_channels<'s>(&'s self, guild: &'s Guild) -> impl Iterator<Item = (&'s Guild, &'s Channel)> {
        guild.channels.iter().chain(threads::joined_active(guild)).map(move |c| (guild, c))
    }

    fn find(&self, id: Snowflake) -> Option<(Option<&Guild>, &Channel)> {
        if let Some(private) = self.private_channels.iter().find(|c| c.id == id) {
            return Some((None, private));
        }
        self.guilds.iter().find_map(|g| g.find(id).map(|c| (Some(g), c)))
    }

    /// A channel as a result (`$` in 174768): a DM is its person.
    fn as_hit(&self, id: Snowflake) -> Option<Hit> {
        let (_, channel) = self.find(id)?;
        let comparator = self.channel_name(channel);
        let (kind, id) = match channel.kind {
            ChannelKind::Dm => (Kind::User, channel.recipients.first()?.id),
            ChannelKind::GroupDm => (Kind::GroupDm, channel.id),
            kind if kind.is_vocal() => (Kind::Voice, channel.id),
            _ => (Kind::Channel, channel.id),
        };
        Some(Hit { kind, id, score: 0.0, comparator, sortable: None })
    }
}

/// Channels with a message history, or posts (`tr` in 95701).
fn text_like(kind: ChannelKind) -> bool {
    matches!(
        kind,
        ChannelKind::Text
            | ChannelKind::Announcement
            | ChannelKind::Forum
            | ChannelKind::Media
            | ChannelKind::AnnouncementThread
            | ChannelKind::PublicThread
            | ChannelKind::PrivateThread
    )
}

// ---- Matching ----

/// `fuzzysearch` (91871): `needle`'s characters appear in `haystack` in order.
fn fuzzy(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

/// Lower case, combining marks dropped (`sS` in 23339).
fn fold(text: &str) -> String {
    let decomposed = glib::normalize(text.to_lowercase(), glib::NormalizeMode::Default);
    let stripped: String = decomposed.chars().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect();
    glib::normalize(stripped, glib::NormalizeMode::DefaultCompose).to_string()
}

/// A word of the query, as the channel search matches it.
#[derive(Clone, Debug)]
struct Token {
    lower: Vec<char>,
    /// Where one space stands for a space or a hyphen: the whole query, when
    /// it has several words, matches a channel's `a-b` as `a b`.
    wildcard: Option<usize>,
    /// The whole query: matching it leaves nothing else to match.
    whole: bool,
}

impl Token {
    fn word(text: &str) -> Token {
        Token { lower: text.to_lowercase().chars().collect(), wildcard: None, whole: false }
    }

    fn whole(text: &str) -> Token {
        let lower: Vec<char> = text.to_lowercase().chars().collect();
        let wildcard = lower.iter().position(|&c| c == ' ');
        Token { lower, wildcard, whole: true }
    }

    fn matches_at(&self, name: &[char], at: usize) -> bool {
        if at + self.lower.len() > name.len() {
            return false;
        }
        self.lower.iter().enumerate().all(|(i, &q)| {
            let n = name[at + i];
            n == q || (self.wildcard == Some(i) && n == '-')
        })
    }

    fn starts(&self, name: &[char]) -> bool {
        self.matches_at(name, 0)
    }

    fn contained(&self, name: &[char]) -> bool {
        (0..=name.len()).any(|at| self.matches_at(name, at))
    }

    fn text(&self) -> String {
        self.lower.iter().collect()
    }
}

/// How well `name` (lower case) matches (`eT` in 997625): 10 for the same,
/// 7 for a start, 5 for a part, 3 where every word is in it, 1 where the
/// letters follow each other, else 0.
fn score(name: &str, token: &Token, fuzzy_too: bool) -> f64 {
    let chars: Vec<char> = name.chars().collect();
    if token.starts(&chars) {
        return if name == token.text() { 10.0 } else { 7.0 };
    }
    if token.contained(&chars) {
        return 5.0;
    }
    let query = token.text();
    if query.split([',', ' ']).filter(|w| !w.is_empty()).all(|w| name.contains(w)) {
        return 3.0;
    }
    if fuzzy_too && fuzzy(&query, name) { 1.0 } else { 0.0 }
}

/// The best of `tokens` against `name`, which it takes out of `tokens`, or
/// all of them if it was the whole query (`eC`).
fn take_best(name: &str, tokens: &mut Vec<Token>, fuzzy_too: bool) -> f64 {
    let mut best = (0.0, None);
    for (i, token) in tokens.iter().enumerate() {
        let s = score(name, token, fuzzy_too);
        if s > best.0 {
            best = (s, Some(i));
        }
    }
    if let Some(i) = best.1 {
        if tokens[i].whole {
            tokens.clear();
        } else {
            tokens.remove(i);
        }
    }
    best.0
}

/// The query's words, and first the whole query if it is more than one.
fn tokens(query: &str, allow_empty: bool) -> Vec<Token> {
    let mut out: Vec<Token> = query.split(' ').filter(|w| !w.is_empty() || allow_empty).map(Token::word).collect();
    if query.contains(' ') {
        out.insert(0, Token::whole(query));
    }
    out
}

/// Results of equal score sort by name, except that a result with a
/// `sortable` compares it with itself, so such ties stay as they were found
/// (`sort` in 989133).
fn compare(a: &Hit, b: &Hit) -> Ordering {
    if a.score == b.score {
        let n = a.sortable.clone().unwrap_or_else(|| a.comparator.to_lowercase());
        let i = a.sortable.clone().unwrap_or_else(|| b.comparator.to_lowercase());
        match n.cmp(&i) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    b.score.total_cmp(&a.score)
}

// ---- Queries ----

/// How a channel query is scoped.
#[derive(Clone, Copy)]
struct ChannelQuery<'q> {
    text: &'q str,
    /// Only this server's channels.
    guild: Option<Snowflake>,
    /// Calls only (`!`), else text-like channels and calls.
    calls_only: bool,
    allow_empty: bool,
    limit: usize,
}

/// Channels matching (`queryChannels`).
fn query_channels(sources: &Sources, q: ChannelQuery) -> Vec<Hit> {
    let scoped = q.guild.and_then(|id| sources.guild(id));
    let mut candidates: Vec<(&Guild, &Channel)> = Vec::new();
    match (q.guild, scoped) {
        (Some(_), Some(guild)) => {
            // The server's own list: its channels by position, then threads.
            let mut channels: Vec<&Channel> = guild
                .channels
                .iter()
                .filter(|c| if q.calls_only { c.kind.is_vocal() } else { text_like(c.kind) })
                .collect();
            channels.sort_by_key(|c| c.position.unwrap_or(0));
            candidates.extend(channels.into_iter().map(|c| (guild, c)));
            candidates.extend(threads::joined_active(guild).map(|t| (guild, t)));
        }
        (Some(_), None) => {}
        (None, _) => {
            for guild in sources.guilds {
                candidates.extend(sources.guild_channels(guild));
            }
        }
    }
    let mut hits = Vec::new();
    let lowered = q.text.to_lowercase();
    for (guild, channel) in candidates {
        let voice = channel.kind.is_vocal();
        let wanted = if q.calls_only { voice } else { text_like(channel.kind) || voice };
        if !wanted || !(sources.can_access)(guild, channel) {
            continue;
        }
        let name = sources.channel_name(channel);
        let lower = name.to_lowercase();
        let mut rest = tokens(q.text, q.allow_empty);
        let by_id = lowered == channel.id.to_string();
        let mut points = if by_id { 10.0 } else { take_best(&lower, &mut rest, true) };
        if points == 0.0 {
            continue;
        }
        if !rest.is_empty() {
            let parent = channel.parent_id.and_then(|p| guild.find(p)).and_then(|p| p.name.as_deref());
            for context in [Some(guild.name.as_str()), parent] {
                let Some(context) = context.filter(|c| !c.is_empty()) else { continue };
                let found = take_best(&context.to_lowercase(), &mut rest, false);
                if found != 0.0 {
                    points += 0.5 * found;
                }
            }
            points = points.min(6.0);
        }
        if points == 0.0 || rest.len() > 1 || (rest.len() == 1 && !rest[0].whole && !by_id) {
            continue;
        }
        if !q.calls_only && voice {
            points = (points - 1.0).max(0.5);
        }
        hits.push(Hit {
            kind: if voice { Kind::Voice } else { Kind::Channel },
            id: channel.id,
            score: 1000.0 * points,
            comparator: name,
            sortable: Some(lower),
        });
    }
    hits.sort_by(compare);
    hits.truncate(q.limit);
    hits
}

/// Servers matching (`queryGuilds`), without `skip`: a search leaves out the
/// open one, a bare `*` lists every server.
fn query_guilds(sources: &Sources, text: &str, limit: usize, skip: Option<Snowflake>) -> Vec<Hit> {
    let lower = text.to_lowercase();
    let token = Token::word(text);
    let mut hits = Vec::new();
    for guild in sources.guilds {
        if Some(guild.id) == skip {
            continue;
        }
        let name = guild.name.to_lowercase();
        let points = if lower == guild.id.to_string() { 10.0 } else { score(&name, &token, true) };
        if points > 0.0 {
            hits.push(Hit {
                kind: Kind::Server,
                id: guild.id,
                score: 1000.0 * points,
                comparator: guild.name.clone(),
                sortable: Some(name),
            });
        }
    }
    hits.sort_by(compare);
    hits.truncate(limit);
    hits
}

/// Group DMs matching (`queryGroupDMs`): by name, or by what any of their
/// people are called, which counts for at most 5.
fn query_group_dms(sources: &Sources, text: &str, limit: usize) -> Vec<Hit> {
    let query = fold(text);
    let token = Token::word(&query);
    let mut hits = Vec::new();
    for channel in sources.private_channels.iter().filter(|c| c.kind == ChannelKind::GroupDm) {
        let name = sources.channel_name(channel);
        let sortable = fold(&name);
        let mut points = score(&sortable, &token, true);
        for person in &channel.recipients {
            let nickname = sources.relationships.get(person.id).and_then(|r| r.nickname.as_deref());
            for called in
                [Some(person.username.as_str()), person.global_name.as_deref(), nickname].into_iter().flatten()
            {
                points = points.max(score(&fold(called), &token, true).min(5.0));
            }
        }
        if points > 0.0 {
            hits.push(Hit {
                kind: Kind::GroupDm,
                id: channel.id,
                score: 1000.0 * points,
                comparator: name,
                sortable: Some(sortable),
            });
        }
    }
    hits.sort_by(compare);
    hits.truncate(limit);
    hits
}

/// How much the client favours a person over equal matches: friends a
/// little, people we have a DM with a little less (`build` in 997625).
fn booster(sources: &Sources, user: Snowflake) -> f64 {
    let mut boost = 1.0;
    if sources.relationships.get(user).is_some_and(|r| r.kind == RelationshipKind::Friend) {
        boost += 0.2;
    }
    if sources
        .private_channels
        .iter()
        .any(|c| c.kind == ChannelKind::Dm && c.recipients.first().is_some_and(|u| u.id == user))
    {
        boost += 0.1;
    }
    boost
}

/// Everyone we know of: the people in our DMs and group DMs, and those we
/// have a relationship with other than blocking (the client leaves out the
/// blocked and ignored).
fn known_users<'a>(sources: &'a Sources) -> Vec<&'a User> {
    let mut seen = std::collections::HashSet::new();
    let mut users = Vec::new();
    let related = [crate::relationships::List::All, crate::relationships::List::Pending]
        .into_iter()
        .flat_map(|list| sources.relationships.list(list))
        .map(|r| &r.user);
    let in_dms = sources.private_channels.iter().flat_map(|c| &c.recipients);
    for user in related.chain(in_dms) {
        if user.id != sources.me && !sources.relationships.is_blocked(user.id) && seen.insert(user.id) {
            users.push(user);
        }
    }
    users
}

/// People matching (the worker's `S` in 787578): a name that starts with the
/// query is worth 10, one that holds it 5, letters in order 1, all times
/// the person's booster. `in_guild` keeps to friends and the server's
/// members, as `@` does inside a server.
fn query_users(sources: &Sources, text: &str, limit: usize, in_guild: Option<Snowflake>) -> Vec<Hit> {
    let lower = text.to_lowercase();
    let start = Token::word(text);
    let mut hits = Vec::new();
    for user in known_users(sources) {
        if let Some(guild) = in_guild {
            let friend = sources.relationships.get(user.id).is_some_and(|r| r.kind == RelationshipKind::Friend);
            if !friend && !(sources.is_member)(guild, user.id) {
                continue;
            }
        }
        let boost = booster(sources, user.id);
        let mut best: Option<(f64, String)> = None;
        let mut offer = |points: f64, name: &str| {
            if best.as_ref().is_none_or(|(b, _)| *b < points) {
                best = Some((points, name.to_owned()));
            }
        };
        if text == user.id.to_string() {
            offer(10.0 * boost, &user.id.to_string());
        } else {
            let nickname = sources.relationships.get(user.id).and_then(|r| r.nickname.as_deref());
            for name in [Some(user.username.as_str()), nickname, user.global_name.as_deref()].into_iter().flatten() {
                let chars: Vec<char> = name.to_lowercase().chars().collect();
                let plain = fold(name);
                if start.starts(&chars) {
                    offer(10.0 * boost, name);
                } else if start.contained(&chars) {
                    offer(5.0 * boost, name);
                } else if fuzzy(&lower, &plain) || fuzzy(&nfd(&lower), &nfd(&plain)) {
                    offer(boost, name);
                }
            }
        }
        if let Some((points, comparator)) = best {
            hits.push(Hit { kind: Kind::User, id: user.id, score: 1000.0 * points, comparator, sortable: None });
        }
    }
    hits.sort_by(compare);
    hits.truncate(limit);
    hits
}

fn nfd(text: &str) -> String {
    glib::normalize(text, glib::NormalizeMode::Default).to_string()
}

// ---- The box ----

/// What a query shows, with the heading its prefix puts first. `@` searches
/// inside the open server.
pub fn search(sources: &Sources, home: &Home, query: &Query) -> Vec<Row> {
    let text = query.text.trim();
    let hits = if text.is_empty() { empty(sources, home, query.mode) } else { search_text(sources, query.mode, text) };
    let mut rows = Vec::with_capacity(hits.len() + 1);
    if let Some(mode) = query.mode {
        let guild = sources.guild.and_then(|id| sources.guild(id)).map(|g| g.name.as_str());
        let guild = if mode == Mode::AllUsers { None } else { guild };
        rows.push(Row::Header(mode.header(guild)));
    }
    rows.extend(hits);
    rows
}

fn search_text(sources: &Sources, mode: Option<Mode>, text: &str) -> Vec<Row> {
    let narrowed = mode.is_some();
    let limit = if narrowed { LIMIT_NARROWED } else { LIMIT };
    let users = |limit| {
        let in_guild = if mode == Some(Mode::Users) { sources.guild } else { None };
        query_users(sources, text, limit, in_guild)
    };
    let channels = |limit| {
        query_channels(sources, ChannelQuery { text, guild: None, calls_only: false, allow_empty: false, limit })
    };
    let mut users_found = Vec::new();
    let mut group_dms = Vec::new();
    let mut channels_found = Vec::new();
    let mut calls = Vec::new();
    let mut servers = Vec::new();
    let all = mode.is_none();
    if all || matches!(mode, Some(Mode::Users | Mode::AllUsers)) {
        users_found = users(limit);
    }
    if all {
        group_dms = query_group_dms(sources, text, limit);
    }
    if all || mode == Some(Mode::Channels) {
        channels_found = channels(limit);
    }
    if mode == Some(Mode::Voice) {
        calls =
            query_channels(sources, ChannelQuery { text, guild: None, calls_only: true, allow_empty: false, limit });
    }
    if all || mode == Some(Mode::Servers) {
        servers = query_guilds(sources, text, limit, sources.guild);
    }
    // One kind with results gets all of them (`refetchIfSingleCategoryResults`).
    if all {
        let found = [!users_found.is_empty(), !group_dms.is_empty(), !channels_found.is_empty(), !servers.is_empty()];
        if found.iter().filter(|f| **f).count() == 1 {
            if found[0] {
                users_found = users(LIMIT_NARROWED);
            } else if found[1] {
                group_dms = query_group_dms(sources, text, LIMIT_NARROWED);
            } else if found[2] {
                channels_found = channels(LIMIT_NARROWED);
            } else {
                servers = query_guilds(sources, text, LIMIT_NARROWED, sources.guild);
            }
        }
    }
    let mut merged: Vec<Hit> = Vec::new();
    for hit in users_found.into_iter().chain(group_dms).chain(channels_found).chain(calls).chain(servers) {
        if !merged.iter().any(|m| m.kind == hit.kind && m.id == hit.id) {
            merged.push(hit);
        }
    }
    merged.sort_by(compare);
    merged.into_iter().map(Row::Hit).collect()
}

/// A bare mode, or nothing typed: the recent, the mentioned and the unread,
/// or everything of one kind.
fn empty(sources: &Sources, home: &Home, mode: Option<Mode>) -> Vec<Row> {
    match mode {
        Some(Mode::Users | Mode::AllUsers) => home
            .speakers
            .iter()
            .filter(|u| u.id != sources.me)
            .map(|u| {
                Row::Hit(Hit {
                    kind: Kind::User,
                    id: u.id,
                    score: 0.0,
                    comparator: sources.user_name(u),
                    sortable: None,
                })
            })
            .collect(),
        Some(Mode::Games) => Vec::new(),
        Some(Mode::Servers) => query_guilds(sources, "", LIMIT_NARROWED, None).into_iter().map(Row::Hit).collect(),
        Some(Mode::Channels) => query_channels(
            sources,
            ChannelQuery {
                text: "",
                guild: sources.guild,
                calls_only: false,
                allow_empty: true,
                limit: LIMIT_NARROWED,
            },
        )
        .into_iter()
        .map(Row::Hit)
        .collect(),
        Some(Mode::Voice) => query_channels(
            sources,
            ChannelQuery { text: "", guild: sources.guild, calls_only: true, allow_empty: true, limit: LIMIT_NARROWED },
        )
        .into_iter()
        .map(Row::Hit)
        .collect(),
        None => recent(sources, home),
    }
}

/// "Previous Channels" above "Mentions" above "Unread Channels"
/// (`QUICKSWITCHER_SHOW` in 174768; Neucord keeps no drafts to list).
fn recent(sources: &Sources, home: &Home) -> Vec<Row> {
    let shown_before = |id: Snowflake| Some(id) == sources.channel;
    let mut taken: Vec<Snowflake> = Vec::new();
    let mut previous = Vec::new();
    for &id in home.history {
        if shown_before(id) {
            continue;
        }
        let Some(hit) = sources.as_hit(id) else { continue };
        let visible = match sources.find(id) {
            Some((Some(guild), channel)) => (sources.can_access)(guild, channel),
            _ => true,
        };
        if visible {
            previous.push(hit);
            if taken.len() < 3 {
                taken.push(id);
            }
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    let mentions: Vec<(Snowflake, Hit)> = home
        .mentions
        .iter()
        .rev()
        .filter(|&&id| !shown_before(id) && !taken.contains(&id))
        .filter_map(|&id| sources.as_hit(id).map(|h| (id, h)))
        .collect();
    if !mentions.is_empty() {
        rows.push(Row::Header("Mentions".to_owned()));
        for (id, hit) in mentions {
            taken.push(id);
            rows.push(Row::Hit(hit));
        }
    }
    let unread: Vec<Hit> = home
        .unread
        .iter()
        .filter(|&&id| !shown_before(id) && !taken.contains(&id))
        .filter_map(|&id| sources.as_hit(id))
        .collect();
    if !unread.is_empty() {
        rows.push(Row::Header("Unread Channels".to_owned()));
        rows.extend(unread.into_iter().map(Row::Hit));
    }
    previous.truncate(if rows.is_empty() { 7 } else { 3 });
    if previous.is_empty() {
        return rows;
    }
    let mut out = vec![Row::Header("Previous Channels".to_owned())];
    out.extend(previous.into_iter().map(Row::Hit));
    out.extend(rows);
    out
}

/// Remember that `channel` was opened, newest first (`CHANNEL_SELECT`).
pub fn visited(history: &mut Vec<Snowflake>, channel: Snowflake) {
    history.retain(|&c| c != channel);
    history.insert(0, channel);
    history.truncate(HISTORY);
}

/// Where the selection starts: the first result, never a heading
/// (`Vv` in 662090 from -1 going down).
pub fn first_selectable(rows: &[Row]) -> Option<usize> {
    step(rows, None, true)
}

/// The next result up or down from `from`, wrapping round the ends and
/// skipping headings.
pub fn step(rows: &[Row], from: Option<usize>, down: bool) -> Option<usize> {
    let len = rows.len() as isize;
    if len == 0 {
        return None;
    }
    // `Vv` starts at -1 (before the first) going down, and wraps to the
    // other end past either.
    let mut at = from.map_or(-1, |i| i as isize);
    let delta = if down { 1 } else { -1 };
    for _ in 0..=len {
        at += delta;
        if at < 0 {
            at = len;
            continue;
        }
        if at >= len {
            at = -1;
            continue;
        }
        if matches!(rows[at as usize], Row::Hit(_)) {
            return Some(at as usize);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn user(id: u64, username: &str, global: Option<&str>) -> serde_json::Value {
        json!({"id": id.to_string(), "username": username, "global_name": global})
    }

    fn guild(id: u64, name: &str, channels: serde_json::Value) -> Guild {
        serde_json::from_value(json!({"id": id.to_string(), "name": name, "channels": channels})).unwrap()
    }

    fn private(id: u64, kind: u8, name: Option<&str>, people: &[(u64, &str)]) -> Channel {
        serde_json::from_value(json!({
            "id": id.to_string(), "type": kind, "name": name,
            "recipients": people.iter().map(|&(i, n)| user(i, n, None)).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn friends(list: &[(u64, &str, Option<&str>)]) -> Relationships {
        Relationships::from_ready(&json!({
            "relationships": list.iter().map(|&(id, name, nick)| json!({
                "id": id.to_string(), "type": 1, "nickname": nick, "user": user(id, name, None),
            })).collect::<Vec<_>>(),
        }))
    }

    struct World {
        guilds: Vec<Guild>,
        private: Vec<Channel>,
        relationships: Relationships,
    }

    impl World {
        fn run<T>(&self, open: Option<u64>, f: impl FnOnce(&Sources) -> T) -> T {
            let access = |_: &Guild, _: &Channel| true;
            let member = |_: Snowflake, _: Snowflake| false;
            f(&Sources {
                me: Snowflake(999),
                guild: open.map(Snowflake),
                channel: None,
                guilds: &self.guilds,
                private_channels: &self.private,
                relationships: &self.relationships,
                can_access: &access,
                is_member: &member,
            })
        }

        fn find(&self, open: Option<u64>, typed: &str) -> Vec<(Kind, u64)> {
            self.run(open, |s| {
                search(s, &Home::default(), &parse(typed))
                    .into_iter()
                    .filter_map(|r| match r {
                        Row::Hit(h) => Some((h.kind, h.id.0)),
                        Row::Header(_) => None,
                    })
                    .collect()
            })
        }
    }

    fn world() -> World {
        World {
            guilds: vec![
                guild(
                    1,
                    "Rustaceans",
                    json!([
                        {"id": "10", "type": 4, "name": "Text Channels"},
                        {"id": "11", "type": 0, "name": "general", "parent_id": "10", "position": 0},
                        {"id": "12", "type": 0, "name": "rust-help", "parent_id": "10", "position": 1},
                        {"id": "13", "type": 2, "name": "lounge", "position": 2},
                    ]),
                ),
                guild(2, "Gardening", json!([{"id": "20", "type": 0, "name": "general", "position": 0}])),
            ],
            private: vec![
                private(30, 1, None, &[(300, "alice")]),
                private(31, 3, Some("book club"), &[(301, "bob"), (302, "carol")]),
            ],
            relationships: friends(&[(300, "alice", None), (303, "dave", Some("Dad"))]),
        }
    }

    #[test]
    fn prefixes_narrow_and_the_first_one_found_is_cut_out() {
        let q = |text: &str, mode| Query { text: text.to_owned(), mode };
        assert_eq!(parse("foo"), q("foo", None));
        assert_eq!(parse("@foo"), q("foo", Some(Mode::Users)));
        assert_eq!(parse("@@foo"), q("foo", Some(Mode::AllUsers)));
        assert_eq!(parse("#gen"), q("gen", Some(Mode::Channels)));
        assert_eq!(parse("!lou"), q("lou", Some(Mode::Voice)));
        assert_eq!(parse("*gar"), q("gar", Some(Mode::Servers)));
        assert_eq!(parse("$game"), q("game", Some(Mode::Games)));
        // Only the first character picks the mode, yet a prefix anywhere is cut out, once.
        assert_eq!(parse("foo#bar#baz"), q("foobar#baz", None));
        assert_eq!(parse("foo@bar"), q("foo@bar", None));
        assert_eq!(parse("#a!b"), q("a!b", Some(Mode::Channels)));
    }

    #[test]
    fn a_name_scores_by_how_it_holds_the_query() {
        let word = Token::word("gen");
        assert_eq!(score("general", &word, true), 7.0);
        assert_eq!(score("gen", &word, true), 10.0);
        assert_eq!(score("my-gen-chat", &word, true), 5.0);
        assert_eq!(score("great-new", &word, true), 1.0);
        assert_eq!(score("great-new", &word, false), 0.0);
        let two = Token::whole("rust help");
        assert_eq!(score("rust-help", &two, true), 7.0, "a space stands for a hyphen");
        assert_eq!(score("help with rust", &two, true), 3.0, "every word is in it");
    }

    #[test]
    fn channels_rank_by_match_and_calls_come_after_text() {
        let w = world();
        // Both servers have #general: equal scores keep the order they were found in.
        assert_eq!(w.find(None, "general"), vec![(Kind::Channel, 11), (Kind::Channel, 20)]);
        // A call scores one lower than the same match in a text channel.
        let ranked = w.run(None, |s| {
            query_channels(
                s,
                ChannelQuery { text: "lounge", guild: None, calls_only: false, allow_empty: false, limit: 5 },
            )
        });
        assert_eq!(ranked[0].kind, Kind::Voice);
        assert_eq!(ranked[0].score, 9000.0);
    }

    #[test]
    fn the_server_and_category_name_help_a_second_word() {
        let w = world();
        // "rustaceans help": `help` is in the channel, the rest in the server's name.
        assert_eq!(w.find(None, "help rustaceans"), vec![(Kind::Channel, 12)]);
        // A second word nothing holds finds nothing.
        assert_eq!(w.find(None, "help gardening"), vec![]);
    }

    #[test]
    fn the_open_server_is_left_out_of_server_results() {
        let w = world();
        assert!(w.find(None, "rust").contains(&(Kind::Server, 1)));
        assert!(w.find(Some(1), "rustaceans").iter().all(|&(k, _)| k != Kind::Server));
    }

    #[test]
    fn a_dm_is_its_person_and_friends_outrank_strangers() {
        let w = world();
        // Prefix 10 × (1 + 0.2 friend + 0.1 DM) for alice; bob and carol are strangers.
        let alice = w.run(None, |s| query_users(s, "alice", 5, None));
        assert_eq!(alice.len(), 1);
        assert!((alice[0].score - 13_000.0).abs() < 1e-6, "{}", alice[0].score);
        let dave = w.run(None, |s| query_users(s, "dad", 5, None));
        assert_eq!(dave[0].comparator, "Dad", "a friend's nickname finds them");
        assert!((dave[0].score - 12_000.0).abs() < 1e-6);
    }

    #[test]
    fn a_group_dm_is_found_by_its_people_at_most_5() {
        let w = world();
        let found = w.run(None, |s| query_group_dms(s, "carol", 5));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].score, 5000.0, "a prefix of a person counts for 5, not 7");
        let named = w.run(None, |s| query_group_dms(s, "book", 5));
        assert_eq!(named[0].score, 7000.0);
    }

    #[test]
    fn inside_a_server_at_finds_friends_and_its_members() {
        let mut w = world();
        w.private.push(private(32, 1, None, &[(304, "alex")]));
        // Not a friend and not a member: left out inside a server, found elsewhere.
        assert_eq!(w.find(Some(1), "@al"), vec![(Kind::User, 300)]);
        assert_eq!(w.find(None, "@al").len(), 3, "carol too: a, then l, in order");
        assert_eq!(w.find(Some(1), "@@al").len(), 3, "two prefixes ask for everyone");
    }

    #[test]
    fn one_kind_with_results_shows_all_of_them() {
        let channels: Vec<_> = (0..8)
            .map(|i| json!({"id": (100 + i).to_string(), "type": 0, "name": format!("room-{i}"), "position": i}))
            .collect();
        let w = World {
            guilds: vec![guild(1, "G", serde_json::Value::Array(channels))],
            private: vec![],
            relationships: Relationships::default(),
        };
        assert_eq!(w.find(None, "room").len(), 8);
        let two_kinds = World {
            guilds: vec![
                guild(1, "room one", json!([{"id": "100", "type": 0, "name": "room-a"}])),
                guild(2, "G", json!([])),
            ],
            private: vec![],
            relationships: Relationships::default(),
        };
        assert_eq!(two_kinds.find(None, "room").len(), 2);
    }

    #[test]
    fn a_narrowed_query_has_a_heading_and_nothing_else_of_other_kinds() {
        let w = world();
        let rows = w.run(Some(1), |s| search(s, &Home::default(), &parse("*")));
        assert!(matches!(&rows[0], Row::Header(h) if h == "Searching Servers"));
        // A bare `*` lists every server, the open one too.
        assert_eq!(rows.len(), 3);
        let users = w.run(Some(1), |s| search(s, &Home::default(), &parse("@")));
        assert_eq!(users, vec![Row::Header("Search friends and users in: Rustaceans".to_owned())]);
    }

    #[test]
    fn nothing_typed_offers_previous_channels_mentions_and_unread() {
        let w = world();
        let rows = w.run(Some(1), |s| {
            let home = Home {
                history: &[Snowflake(12), Snowflake(30), Snowflake(20)],
                mentions: &[Snowflake(31)],
                unread: &[Snowflake(11)],
                speakers: &[],
            };
            recent(s, &home)
        });
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Header(h) => h.clone(),
                Row::Hit(h) => h.comparator.clone(),
            })
            .collect();
        assert_eq!(
            names,
            [
                "Previous Channels",
                "rust-help",
                "alice",
                "general",
                "Mentions",
                "book club",
                "Unread Channels",
                "general"
            ]
        );
    }

    #[test]
    fn without_other_sections_seven_previous_channels_show() {
        let w = world();
        let history: Vec<Snowflake> = [11, 12, 13, 20, 30, 31].into_iter().map(Snowflake).collect();
        let rows = w.run(None, |s| recent(s, &Home { history: &history, ..Home::default() }));
        assert_eq!(rows.len(), 7);
        let with_mention =
            w.run(None, |s| recent(s, &Home { history: &history, mentions: &[Snowflake(31)], ..Home::default() }));
        // The first three previous channels, a mention that isn't among them.
        assert_eq!(with_mention.len(), 1 + 3 + 2);
    }

    #[test]
    fn the_selection_skips_headings_and_wraps() {
        let hit = |id| {
            Row::Hit(Hit {
                kind: Kind::Server,
                id: Snowflake(id),
                score: 0.0,
                comparator: String::new(),
                sortable: None,
            })
        };
        let rows = vec![Row::Header("x".into()), hit(1), hit(2), Row::Header("y".into()), hit(3)];
        assert_eq!(first_selectable(&rows), Some(1));
        assert_eq!(step(&rows, Some(2), true), Some(4));
        assert_eq!(step(&rows, Some(4), true), Some(1));
        assert_eq!(step(&rows, Some(1), false), Some(4));
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn history_keeps_eight_newest_first() {
        let mut history = Vec::new();
        for id in 1..=10 {
            visited(&mut history, Snowflake(id));
        }
        visited(&mut history, Snowflake(5));
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history[0], Snowflake(5));
        assert_eq!(history.iter().filter(|&&c| c == Snowflake(5)).count(), 1);
    }

    #[test]
    fn marks_and_case_do_not_matter_to_a_group_dms_people() {
        assert_eq!(fold("Éloïse"), "eloise");
    }
}
