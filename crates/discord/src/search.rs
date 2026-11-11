//! Searching messages: the grammar the search box understands, the request
//! it makes, and what the client does with the answer.
//!
//! The grammar is `getAutocompletions` and friends in 224467 (the rules), the
//! generic tokenizer in 988665, and what becomes the query in `Zf` (822382).
//! The request is `fetchMessages` in 256796 with the fetcher in 177068. A
//! server is searched at `/guilds/{id}/messages/search`, a DM at
//! `/channels/{id}/messages/search`; searching every DM at once is a POST to
//! `/users/@me/messages/search/tabs`, which the client does only where the
//! setting `isCrossDmSearchEnabled` is on, and which Neucord, reading no
//! settings protobuf, never sends.

use std::time::Duration;

use serde_json::Value;

use crate::{Message, Snowflake, User};

/// Results to a page, and the most the client pages through (`o` in 477654).
pub const PAGE: u32 = 25;
pub const MAX_RESULTS: u64 = 10_000;
pub const MAX_PAGES: u64 = 400;
/// How often a search that Discord is still indexing for is asked again.
pub const MAX_ATTEMPTS: u32 = 5;
/// The search box takes this much (`e.insertText` in 586944).
pub const MAX_QUERY: usize = 512;
/// The client waits this long after Enter before it sends (`te` in 586944).
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Where a search looks: a whole server, or one DM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    Guild(Snowflake),
    Channel(Snowflake),
}

impl Context {
    pub fn is_guild(self) -> bool {
        matches!(self, Context::Guild(_))
    }
}

/// How results are ordered (`nm` in 822382).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    MostRelevant,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::Newest, Sort::Oldest, Sort::MostRelevant];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Newest => "Newest",
            Sort::Oldest => "Oldest",
            Sort::MostRelevant => "Most Relevant",
        }
    }

    fn fields(self) -> (&'static str, &'static str) {
        match self {
            Sort::Newest => ("timestamp", "desc"),
            Sort::Oldest => ("timestamp", "asc"),
            Sort::MostRelevant => ("relevance", "desc"),
        }
    }
}

// ---- The grammar ----

/// What a filter filters on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    From,
    Mentions,
    Has,
    LinkFrom,
    FileType,
    FileName,
    Before,
    During,
    After,
    In,
    Pinned,
    AuthorType,
}

impl Filter {
    /// The rules in the order the client tries them (`ea` in 224467).
    const RULES: [Filter; 12] = [
        Filter::From,
        Filter::Mentions,
        Filter::Has,
        Filter::LinkFrom,
        Filter::FileType,
        Filter::FileName,
        Filter::Before,
        Filter::During,
        Filter::After,
        Filter::In,
        Filter::Pinned,
        Filter::AuthorType,
    ];

    /// What is typed, case aside. `on:` also means `during:`.
    pub fn key(self) -> &'static str {
        match self {
            Filter::From => "from:",
            Filter::Mentions => "mentions:",
            Filter::Has => "has:",
            Filter::LinkFrom => "linkFrom:",
            Filter::FileType => "fileType:",
            Filter::FileName => "fileName:",
            Filter::Before => "before:",
            Filter::During => "during:",
            Filter::After => "after:",
            Filter::In => "in:",
            Filter::Pinned => "pinned:",
            Filter::AuthorType => "authorType:",
        }
    }

    fn query_key(self) -> &'static str {
        match self {
            Filter::From => "author_id",
            Filter::Mentions => "mentions",
            Filter::Has => "has",
            Filter::LinkFrom => "link_hostname",
            Filter::FileType => "attachment_extension",
            Filter::FileName => "attachment_filename",
            Filter::In => "channel_id",
            Filter::Pinned => "pinned",
            Filter::AuthorType => "author_type",
            Filter::Before | Filter::During | Filter::After => "",
        }
    }

    /// The filters the box offers (`u_` in 5990): `in:` only in a server.
    pub fn offered(context: Context) -> Vec<Filter> {
        let mut filters = vec![Filter::From];
        if context.is_guild() {
            filters.push(Filter::In);
        }
        filters.extend([
            Filter::Has,
            Filter::Mentions,
            Filter::During,
            Filter::Before,
            Filter::After,
            Filter::AuthorType,
            Filter::Pinned,
        ]);
        filters
    }
}

/// What `has:` can ask for, as typed and as sent (`Y` in 224467).
pub const HAS: [(&str, &str); 9] = [
    ("image", "image"),
    ("video", "video"),
    ("link", "link"),
    ("file", "file"),
    ("embed", "embed"),
    ("sound", "sound"),
    ("poll", "poll"),
    ("sticker", "sticker"),
    ("forward", "snapshot"),
];
/// What `authorType:` can ask for (`K`).
pub const AUTHOR_TYPES: [&str; 3] = ["user", "bot", "webhook"];

/// What a filter was given.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    User(Snowflake),
    Channels(Vec<Snowflake>),
    /// What is sent, with the `-` that asks for the opposite.
    Word(String),
    Pinned(bool),
    /// Milliseconds since 1970: messages from `start` on, and before `end`.
    Dates {
        start: Option<i64>,
        end: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// Words to search for, with the spaces round them.
    Text(String),
    Filter(Filter),
    Answer(Filter, Answer),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub part: Part,
    /// In characters.
    pub start: usize,
    pub end: usize,
}

/// What the grammar looks up: people and channels by what they are called.
pub struct Lookup<'a> {
    pub context: Context,
    pub me: Snowflake,
    /// A person by username, and the four digits of an old tag if typed.
    pub user: &'a dyn Fn(&str, Option<&str>) -> Option<Snowflake>,
    /// The channels, threads included, a server's name for them says.
    pub channels: &'a dyn Fn(&str) -> Vec<Snowflake>,
    pub zone: &'a glib::TimeZone,
    /// When "today" is.
    pub now: i64,
}

fn is_snowflake(text: &str) -> bool {
    (17..=19).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// Characters of `text` from `at` that are spaces.
fn skip_space(text: &[char], mut at: usize) -> usize {
    while text.get(at).is_some_and(|c| c.is_whitespace()) {
        at += 1;
    }
    at
}

fn starts_with_ci(text: &[char], at: usize, word: &str) -> bool {
    let mut i = at;
    for w in word.chars() {
        match text.get(i) {
            Some(c) if c.to_lowercase().eq(w.to_lowercase()) => i += 1,
            _ => return false,
        }
    }
    true
}

/// The filter whose key starts at `at`, and where its key ends.
fn filter_at(text: &[char], at: usize, context: Context) -> Option<(Filter, usize)> {
    for filter in Filter::RULES {
        // `on:` and `during:` are one filter.
        let keys: &[&str] = if filter == Filter::During { &["on:", "during:"] } else { &[filter.key()] };
        for key in keys {
            if starts_with_ci(text, at, key) {
                // `in:` is for servers (the rule's validator, 5990).
                if filter == Filter::In && !context.is_guild() {
                    continue;
                }
                return Some((filter, at + key.chars().count()));
            }
        }
    }
    None
}

/// The word run `[^\s]+` from `at`, after spaces.
fn word_at(text: &[char], at: usize) -> Option<(String, usize)> {
    let start = skip_space(text, at);
    let end = (start..text.len()).find(|&i| text[i].is_whitespace()).unwrap_or(text.len());
    (end > start).then(|| (text[start..end].iter().collect(), end))
}

/// A person, as `from:` and `mentions:` take them (the regex `ei`).
fn user_at(text: &[char], at: usize, lookup: &Lookup) -> Option<(Snowflake, usize)> {
    let start = skip_space(text, at);
    let digits = text[start..].iter().take_while(|c| c.is_ascii_digit()).count().min(20);
    if digits >= 17 {
        let id: String = text[start..start + digits].iter().collect();
        return is_snowflake(&id).then(|| id.parse().ok().map(|n| (Snowflake(n), start + digits))).flatten();
    }
    if starts_with_ci(text, start, "@me") {
        // Only the lower-case word is us.
        let exact: String = text[start..start + 3].iter().collect();
        return (exact == "@me").then_some((lookup.me, start + 3));
    }
    // `name#1234`, the name being anything but `@`, `#` and `:`.
    let name = text[start..].iter().take_while(|c| !matches!(c, '@' | '#' | ':')).count();
    if name > 0
        && text.get(start + name) == Some(&'#')
        && text.len() >= start + name + 5
        && text[start + name + 1..start + name + 5].iter().all(char::is_ascii_digit)
    {
        let (name, tag): (String, String) =
            (text[start..start + name].iter().collect(), text[start + name + 1..start + name + 5].iter().collect());
        return (lookup.user)(&name, Some(&tag))
            .map(|id| (id, start + text[start..].iter().position(|&c| c == '#').unwrap() + 5));
    }
    let username =
        text[start..].iter().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.')).count().min(32);
    if username >= 2 {
        let name: String = text[start..start + username].iter().collect();
        return (lookup.user)(&name, None).map(|id| (id, start + username));
    }
    None
}

/// One of `words`, with a `-` before it for the opposite.
fn word_from(text: &[char], at: usize, words: &[(&str, &str)]) -> Option<(String, usize)> {
    let start = skip_space(text, at);
    let negated = text.get(start) == Some(&'-');
    let from = start + usize::from(negated);
    // The longest first, as the client's pattern lists them.
    let mut by_length: Vec<&(&str, &str)> = words.iter().collect();
    by_length.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
    let (typed, sent) = by_length.into_iter().find(|(w, _)| starts_with_ci(text, from, w))?;
    // The validator knows only the lower-case word.
    let exact: String = text[from..from + typed.len()].iter().collect();
    (exact == *typed).then(|| (format!("{}{sent}", if negated { "-" } else { "" }), from + typed.len()))
}

/// A channel for `in:`: a snowflake, or a name that may be quoted (`O`).
fn channel_at(text: &[char], at: usize, lookup: &Lookup) -> Option<(Vec<Snowflake>, usize)> {
    let mut start = skip_space(text, at);
    if text.get(start) == Some(&'#') {
        start += 1;
    }
    let (raw, end) = if text.get(start) == Some(&'"') {
        // A quoted name, where `\\` and `\"` are escapes.
        let mut i = start + 1;
        loop {
            match text.get(i) {
                Some('\\') if matches!(text.get(i + 1), Some('\\' | '"')) => i += 2,
                Some('"') => break (text[start..=i].iter().collect::<String>(), i + 1),
                Some(c) if *c != '\\' => i += 1,
                _ => return word_unquoted(text, start, lookup),
            }
        }
    } else {
        let end = (start..text.len()).find(|&i| text[i].is_whitespace()).unwrap_or(text.len());
        if end == start {
            return None;
        }
        (text[start..end].iter().collect::<String>(), end)
    };
    channel_named(&raw, lookup).map(|ids| (ids, end))
}

/// A name that opens with a quote but never closes it is a word like another.
fn word_unquoted(text: &[char], start: usize, lookup: &Lookup) -> Option<(Vec<Snowflake>, usize)> {
    let end = (start..text.len()).find(|&i| text[i].is_whitespace()).unwrap_or(text.len());
    let raw: String = text[start..end].iter().collect();
    channel_named(&raw, lookup).map(|ids| (ids, end))
}

fn channel_named(raw: &str, lookup: &Lookup) -> Option<Vec<Snowflake>> {
    if is_snowflake(raw) {
        return Some(vec![Snowflake(raw.parse().ok()?)]);
    }
    let mut name = raw.to_owned();
    if name.starts_with('"') && name.ends_with('"') && name.len() >= 2 {
        name = unescape(&name[1..name.len() - 1]);
    }
    let ids = (lookup.channels)(&name);
    (!ids.is_empty()).then_some(ids)
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(next)) => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// A day or a stretch of days for `before:`, `during:` and `after:` (`j`).
fn date_at(text: &[char], at: usize, filter: Filter, lookup: &Lookup) -> Option<(Answer, usize)> {
    let start = skip_space(text, at);
    // `YYYY-M-D`, `YYYY-M`, `YYYY`, or a run of anything but digits and spaces.
    let rest: String = text[start..].iter().collect();
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut len = 0;
    let d0 = digits(&rest);
    if d0 == 4 {
        len = 4;
        if rest[len..].starts_with('-') {
            let m = digits(&rest[len + 1..]);
            if (1..=2).contains(&m) {
                len += 1 + m;
                if rest[len..].starts_with('-') {
                    let d = digits(&rest[len + 1..]);
                    if (1..=2).contains(&d) {
                        len += 1 + d;
                    }
                }
            }
        }
    } else if d0 == 0 {
        len = rest.chars().take_while(|c| !c.is_ascii_digit() && !c.is_whitespace()).map(char::len_utf8).sum();
    }
    if len == 0 {
        return None;
    }
    let word = &rest[..len];
    let (from, to) = resolve_date(word, lookup.zone, lookup.now)?;
    let answer = match filter {
        Filter::Before => Answer::Dates { start: None, end: Some(from) },
        Filter::After => Answer::Dates { start: Some(to), end: None },
        _ => Answer::Dates { start: Some(from), end: Some(to) },
    };
    Some((answer, start + word.chars().count()))
}

fn answer_at(filter: Filter, text: &[char], at: usize, lookup: &Lookup) -> Option<(Answer, usize)> {
    match filter {
        Filter::From | Filter::Mentions => user_at(text, at, lookup).map(|(id, end)| (Answer::User(id), end)),
        Filter::Has => word_from(text, at, &HAS).map(|(w, end)| (Answer::Word(w), end)),
        Filter::AuthorType => {
            let words: Vec<(&str, &str)> = AUTHOR_TYPES.iter().map(|w| (*w, *w)).collect();
            word_from(text, at, &words).map(|(w, end)| (Answer::Word(w), end))
        }
        Filter::Pinned => {
            let start = skip_space(text, at);
            ["true", "false"].into_iter().find_map(|w| {
                // The pattern is case-insensitive, the validator is not.
                starts_with_ci(text, start, w).then(|| {
                    let typed: String = text[start..start + w.len()].iter().collect();
                    (typed == w).then_some((Answer::Pinned(w == "true"), start + w.len()))
                })?
            })
        }
        Filter::LinkFrom | Filter::FileType | Filter::FileName => {
            word_from_any(text, at).map(|(w, end)| (Answer::Word(w), end))
        }
        Filter::Before | Filter::During | Filter::After => date_at(text, at, filter, lookup),
        Filter::In => channel_at(text, at, lookup).map(|(ids, end)| (Answer::Channels(ids), end)),
    }
}

fn word_from_any(text: &[char], at: usize) -> Option<(String, usize)> {
    word_at(text, at)
}

/// Split what is typed into tokens (`tokenize` in 988665): at each place
/// the answer to the filter just before is tried first, then the filters.
pub fn tokenize(text: &str, lookup: &Lookup) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut pending_from = 0;
    let mut at = 0;
    let flush = |tokens: &mut Vec<Token>, from: usize, to: usize| {
        if to > from {
            tokens.push(Token { part: Part::Text(chars[from..to].iter().collect()), start: from, end: to });
        }
    };
    while at < chars.len() {
        let previous = tokens.last().filter(|t| t.end == at).map(|t| t.part.clone());
        let found = match previous {
            Some(Part::Filter(filter)) => {
                answer_at(filter, &chars, at, lookup).map(|(a, end)| (Part::Answer(filter, a), end))
            }
            _ => None,
        }
        .or_else(|| filter_at(&chars, at, lookup.context).map(|(f, end)| (Part::Filter(f), end)));
        match found {
            Some((part, end)) => {
                flush(&mut tokens, pending_from, at);
                tokens.push(Token { part, start: at, end });
                at = end;
                pending_from = end;
            }
            None => at += 1,
        }
    }
    flush(&mut tokens, pending_from, chars.len());
    tokens
}

/// Whether a token stays in the query: a filter with no answer after it
/// doesn't (`Zh` in 822382).
fn stays(tokens: &[Token], i: usize) -> bool {
    match &tokens[i].part {
        Part::Filter(_) => matches!(tokens.get(i + 1).map(|t| &t.part), Some(Part::Answer(..))),
        _ => true,
    }
}

/// The text with the filters that were given nothing cut out.
pub fn cleaned(text: &str, tokens: &[Token]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut last = 0;
    for (i, token) in tokens.iter().enumerate() {
        if !stays(tokens, i) {
            out.extend(&chars[last..token.start]);
            last = token.end;
        }
    }
    out.extend(&chars[last..]);
    out
}

/// What goes into the query: its keys in the order they first came up, and
/// what each has (`Zf` in 822382).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields(pub Vec<(String, Vec<String>)>);

impl Fields {
    fn push(&mut self, key: &str, value: String) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, values)) => {
                // A set: a repeat adds nothing.
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            None => self.0.push((key.to_owned(), vec![value])),
        }
    }

    fn set(&mut self, key: &str, value: String) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, values)) => *values = vec![value],
            None => self.0.push((key.to_owned(), vec![value])),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The query's fields from tokens.
pub fn fields(tokens: &[Token]) -> Fields {
    let mut out = Fields::default();
    for token in tokens {
        match &token.part {
            Part::Filter(_) => {}
            Part::Answer(filter, answer) => match answer {
                Answer::Dates { start, end } => {
                    if let Some(start) = start {
                        out.set("min_id", Snowflake::from_unix_ms(*start).to_string());
                    }
                    if let Some(end) = end {
                        out.set("max_id", Snowflake::from_unix_ms(*end).to_string());
                    }
                }
                Answer::User(id) => out.push(filter.query_key(), id.to_string()),
                Answer::Channels(ids) => ids.iter().for_each(|id| out.push(filter.query_key(), id.to_string())),
                Answer::Word(word) => out.push(filter.query_key(), word.clone()),
                Answer::Pinned(pinned) => out.push(filter.query_key(), pinned.to_string()),
            },
            Part::Text(text) => out.push("content", text.trim().to_owned()),
        }
    }
    if let Some(at) = out.0.iter().position(|(k, _)| k == "content") {
        let content = out.0[at].1.join(" ").trim().to_owned();
        if content.is_empty() {
            out.0.remove(at);
        } else {
            out.0[at].1 = vec![content];
        }
    }
    out
}

// ---- Dates ----

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const WEEKDAYS: [&str; 7] = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
/// The words that name a stretch of time, besides months, days and years.
pub const PERIODS: [&str; 5] = ["today", "yesterday", "week", "month", "year"];

fn day_start(zone: &glib::TimeZone, y: i32, m: i32, d: i32) -> Option<glib::DateTime> {
    glib::DateTime::new(zone, y, m, d, 0, 0, 0.0).ok()
}

/// The stretch `word` names, as `[from, to)` in milliseconds since 1970
/// (`j` in 224467, with the English words, weeks starting on Sunday).
fn resolve_date(word: &str, zone: &glib::TimeZone, now_ms: i64) -> Option<(i64, i64)> {
    let lower = word.trim().to_lowercase();
    let now = glib::DateTime::from_unix_utc(now_ms / 1000).ok()?.to_timezone(zone).ok()?;
    let today = day_start(zone, now.year(), now.month(), now.day_of_month())?;
    let ms = |t: &glib::DateTime| t.to_unix() * 1000;
    let (start, end) = match lower.as_str() {
        "today" => (today.clone(), today.add_days(1).ok()?),
        "yesterday" => {
            let day = today.add_days(-1).ok()?;
            (day, today.clone())
        }
        "week" => {
            // `day_of_week` is 1 for Monday to 7 for Sunday.
            let back = now.day_of_week() % 7;
            let first = today.add_days(-back).ok()?;
            let end = first.add_days(7).ok()?;
            (first, end)
        }
        "month" => {
            let first = day_start(zone, now.year(), now.month(), 1)?;
            let end = first.add_months(1).ok()?;
            (first, end)
        }
        "year" => {
            let first = day_start(zone, now.year(), 1, 1)?;
            let end = first.add_years(1).ok()?;
            (first, end)
        }
        w if MONTHS.contains(&w) => {
            let m = MONTHS.iter().position(|&x| x == w)? as i32 + 1;
            let first = day_start(zone, now.year(), m, 1)?;
            let end = first.add_months(1).ok()?;
            (first, end)
        }
        w if WEEKDAYS.contains(&w) => {
            let wanted = WEEKDAYS.iter().position(|&x| x == w)? as i32;
            let sunday = today.add_days(-(now.day_of_week() % 7)).ok()?;
            let day = sunday.add_days(wanted).ok()?;
            let end = day.add_days(1).ok()?;
            (day, end)
        }
        w if w.len() == 4 && w.parse::<i32>().is_ok_and(|y| (2015..=now.year()).contains(&y)) => {
            let first = day_start(zone, w.parse().ok()?, 1, 1)?;
            let end = first.add_years(1).ok()?;
            (first, end)
        }
        w => {
            // `YYYY-MM-DD` read loosely: what is left out is the first.
            let mut parts = w.split('-');
            let y: i32 = parts.next()?.parse().ok()?;
            let m: i32 = parts.next().map_or(Some(1), |m| m.parse().ok())?;
            let d: i32 = parts.next().map_or(Some(1), |d| d.parse().ok())?;
            let first = day_start(zone, y, m, d)?;
            let end = first.add_days(1).ok()?;
            (first, end)
        }
    };
    Some((ms(&start), ms(&end)))
}

// ---- The request ----

/// A search, as `fetchMessages` sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub context: Context,
    pub fields: Fields,
    pub sort: Sort,
    /// Where results start: a page number times [`PAGE`].
    pub offset: u32,
    /// `search_session_id` and `search_query_id`, which the client adds only
    /// to the servers that are in Server Discovery.
    pub discovery: Option<(String, String)>,
}

impl Request {
    /// The query string in the client's order: what was typed, the sort,
    /// Discovery's ids, the offset, and after a 202 the `attempts` so far
    /// (the fetcher counts them in the query itself, 177068).
    pub fn query(&self, attempts: u32) -> String {
        let (by, order) = self.sort.fields();
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for (key, values) in &self.fields.0 {
            pairs.extend(values.iter().map(|v| (key.as_str(), v.as_str())));
        }
        pairs.extend([("sort_by", by), ("sort_order", order)]);
        if let Some((session, query)) = &self.discovery {
            pairs.extend([("search_session_id", session.as_str()), ("search_query_id", query.as_str())]);
        }
        let offset = self.offset.to_string();
        pairs.push(("offset", &offset));
        let attempts = attempts.to_string();
        if attempts != "0" {
            pairs.push(("attempts", &attempts));
        }
        pairs.iter().map(|(k, v)| format!("{}={}", component(k), component(v))).collect::<Vec<_>>().join("&")
    }
}

/// `encodeURIComponent`.
fn component(text: &str) -> String {
    glib::uri_escape_string(text, Some("!*'()"), false).to_string()
}

/// Whether a request is worth sending (`K` in 586944): something is asked for.
pub fn askable(tokens: &[Token]) -> bool {
    !tokens.is_empty() && !fields(tokens).is_empty()
}

/// How long to wait before asking a search that was answered 202 again:
/// `Retry-After` in seconds, or five when it says none or 0.
pub fn retry_delay(retry_after: Option<&str>) -> Duration {
    // `parseInt` takes the digits at the start.
    let seconds = retry_after
        .map(str::trim)
        .map(|v| v.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|d| d.parse::<u64>().ok())
        .filter(|&s| s > 0);
    Duration::from_secs(seconds.unwrap_or(5))
}

/// What the client does with a 202, given the attempts made before it:
/// `None` stops asking (and says nothing), else the wait for the next try.
pub fn after_indexing(attempts_before: u32, retry_after: Option<&str>) -> Option<(u32, Duration)> {
    let attempts = attempts_before + 1;
    (attempts <= MAX_ATTEMPTS).then(|| (attempts, retry_delay(retry_after)))
}

// ---- The answer ----

/// A page of results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub total: u64,
    pub messages: Vec<Message>,
    /// Older messages are still being indexed.
    pub indexing: bool,
    pub documents_indexed: u64,
}

impl Page {
    /// From a 200: each result is a list of messages round the one found,
    /// whose first the client keeps (`SEARCH_MESSAGES_SUCCESS` in 517381).
    pub fn from_value(answer: &Value) -> Option<Page> {
        let groups = answer["messages"].as_array()?;
        let messages = groups
            .iter()
            .filter_map(|g| g.as_array()?.first())
            .filter_map(|m| serde_json::from_value::<Message>(m.clone()).ok())
            .collect();
        Some(Page {
            total: answer["total_results"].as_u64()?,
            messages,
            indexing: answer["doing_deep_historical_index"].as_bool().unwrap_or(false),
            documents_indexed: answer["documents_indexed"].as_u64().unwrap_or(0),
        })
    }
}

/// What a search shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Searching,
    /// Discord is indexing the server or DM before it can search.
    Indexing,
    Done(Page),
    Failed,
}

/// The pages the box offers for `total` results (`paginationTotalCount`).
pub fn pages(total: u64) -> u64 {
    total.min(MAX_RESULTS).div_ceil(u64::from(PAGE))
}

// ---- Suggestions ----

/// What the box offers under what is typed (`Gk` in 822382 and the filters'
/// `getAutocompletions`).
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// Start a filter: its key goes in.
    Filter(Filter),
    /// Complete the filter being typed with this text.
    Value { filter: Filter, label: String, insert: String },
}

/// What the box can fill in from: people and channels it knows.
pub struct Known<'a> {
    pub users: &'a [User],
    /// Channel names, for `in:`.
    pub channels: &'a [String],
}

/// `fuzzysearch` (91871): `needle`'s characters appear in `haystack` in order.
fn fuzzy(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

/// `fuzzysearch` over `names`, as `ee` takes the first `limit`.
fn fuzzy_names<'a>(query: &str, names: impl IntoIterator<Item = &'a str>, limit: usize) -> Vec<&'a str> {
    let q = query.to_lowercase();
    names.into_iter().filter(|n| fuzzy(&q, &n.to_lowercase())).take(limit).collect()
}

/// A name the way a query writes it: quoted where it holds a space or a quote.
pub fn quoted(name: &str) -> String {
    if name.contains(['\\', '"', ' ']) {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        name.to_owned()
    }
}

/// The filter being filled in at `cursor` (`Gk` in 822382), and what of its
/// answer is typed: the cursor is in the filter's own key, or in the words
/// that follow it.
fn filling(text: &str, cursor: usize, tokens: &[Token]) -> Option<(usize, Filter, String)> {
    let i = tokens.iter().position(|t| cursor >= t.start && cursor <= t.end)?;
    match &tokens[i].part {
        Part::Filter(f) => Some((i, *f, String::new())),
        Part::Text(_) => {
            let previous = i.checked_sub(1).map(|p| &tokens[p])?;
            let Part::Filter(f) = previous.part else { return None };
            let typed: String = text.chars().skip(tokens[i].start).take(cursor - tokens[i].start).collect();
            let typed = typed.trim_start();
            // Past the first word it is no longer the answer.
            (!typed.contains(char::is_whitespace)).then(|| (i - 1, f, typed.to_owned()))
        }
        Part::Answer(..) => None,
    }
}

/// The suggestions for `text` with the cursor at `cursor` (in characters).
pub fn suggest(text: &str, cursor: usize, tokens: &[Token], context: Context, known: &Known) -> Vec<Suggestion> {
    let starts = || {
        Filter::offered(context)
            .into_iter()
            .filter(|f| matches!(f, Filter::From | Filter::In | Filter::Has | Filter::Mentions))
            .map(Suggestion::Filter)
            .collect()
    };
    if text.is_empty() {
        return starts();
    }
    match filling(text, cursor, tokens) {
        Some((_, filter, typed)) => values(filter, &typed, known),
        None => starts(),
    }
}

fn values(filter: Filter, typed: &str, known: &Known) -> Vec<Suggestion> {
    let limit = 10;
    let value = |label: &str, insert: String| Suggestion::Value { filter, label: label.to_owned(), insert };
    match filter {
        Filter::From | Filter::Mentions => {
            let query = typed.to_lowercase();
            let query = query.trim_start_matches('@');
            let mut out: Vec<Suggestion> = Vec::new();
            if !query.is_empty() && ("me".starts_with(query) || "@me".starts_with(query)) {
                out.push(value("@me", "@me".to_owned()));
            }
            out.extend(
                known
                    .users
                    .iter()
                    .filter(|u| {
                        query.is_empty()
                            || fuzzy(query, &u.username.to_lowercase())
                            || u.global_name.as_deref().is_some_and(|g| fuzzy(query, &g.to_lowercase()))
                    })
                    .take(limit)
                    .map(|u| value(&u.username, u.username.clone())),
            );
            out
        }
        Filter::In => {
            let query = typed.trim_start_matches('"').trim_start_matches('#');
            fuzzy_names(query, known.channels.iter().map(String::as_str), limit)
                .into_iter()
                .map(|name| value(name, format!("#{}", quoted(name))))
                .collect()
        }
        Filter::Has => {
            fuzzy_names(typed, HAS.iter().map(|(w, _)| *w), limit).into_iter().map(|w| value(w, w.to_owned())).collect()
        }
        Filter::AuthorType => {
            fuzzy_names(typed, AUTHOR_TYPES, limit).into_iter().map(|w| value(w, w.to_owned())).collect()
        }
        Filter::Pinned => ["true", "false"].into_iter().map(|w| value(w, w.to_owned())).collect(),
        Filter::Before | Filter::During | Filter::After => {
            let current_year = glib::DateTime::now_local().map_or(2026, |d| d.year());
            let years: Vec<String> = (2015..=current_year).map(|y| y.to_string()).collect();
            let names = MONTHS.iter().chain(&WEEKDAYS).chain(&PERIODS).copied().chain(years.iter().map(String::as_str));
            fuzzy_names(typed, names, limit).into_iter().map(|w| value(w, w.to_owned())).collect()
        }
        Filter::LinkFrom | Filter::FileType | Filter::FileName => Vec::new(),
    }
}

/// `text` with the word at the cursor swapped for `suggestion`, and the cursor
/// after it: a filter's key replaces the word typed to start one, a value
/// the answer typed after its filter.
pub fn apply(text: &str, cursor: usize, tokens: &[Token], suggestion: &Suggestion) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let word_end = (cursor..chars.len()).find(|&i| chars[i].is_whitespace()).unwrap_or(chars.len());
    let (from, add) = match suggestion {
        Suggestion::Filter(f) => {
            let from = (0..cursor).rev().find(|&i| chars[i].is_whitespace()).map_or(0, |i| i + 1);
            (from, format!("{} ", f.key()))
        }
        Suggestion::Value { insert, .. } => {
            let from = filling(text, cursor, tokens).map_or(cursor, |(i, ..)| tokens[i].end);
            (from, format!("{insert} "))
        }
    };
    let mut out: String = chars[..from].iter().collect();
    out.push_str(&add);
    out.extend(&chars[word_end.max(from)..]);
    let cursor = from + add.chars().count();
    (out.chars().take(MAX_QUERY).collect(), cursor)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ALICE: Snowflake = Snowflake(111_111_111_111_111_111);
    const ME: Snowflake = Snowflake(999_999_999_999_999_999);
    /// 2026-11-11T12:00:00Z, a Wednesday.
    const NOW: i64 = 1_794_398_400_000;

    fn lookup<'a>(
        context: Context,
        zone: &'a glib::TimeZone,
        user: &'a dyn Fn(&str, Option<&str>) -> Option<Snowflake>,
        channels: &'a dyn Fn(&str) -> Vec<Snowflake>,
    ) -> Lookup<'a> {
        Lookup { context, me: ME, user, channels, zone, now: NOW }
    }

    fn parse(context: Context, text: &str) -> Vec<Token> {
        let zone = glib::TimeZone::utc();
        let user = |name: &str, tag: Option<&str>| match (name, tag) {
            ("alice", None) | ("Alice Smith", Some("0042")) => Some(ALICE),
            _ => None,
        };
        let channels = |name: &str| match name {
            "general" => vec![Snowflake(5)],
            "two words" => vec![Snowflake(6), Snowflake(7)],
            _ => Vec::new(),
        };
        tokenize(text, &lookup(context, &zone, &user, &channels))
    }

    const GUILD: Context = Context::Guild(Snowflake(1));
    const DM: Context = Context::Channel(Snowflake(2));

    fn query(context: Context, text: &str) -> String {
        let request =
            Request { context, fields: fields(&parse(context, text)), sort: Sort::Newest, offset: 0, discovery: None };
        request.query(0)
    }

    #[test]
    fn words_and_filters_make_the_query_in_the_order_they_came() {
        // The space between the person and `has:` is a piece of text of its
        // own, empty once trimmed, so the words are joined with two spaces,
        // as the client joins them.
        assert_eq!(
            query(GUILD, "hello from:alice has:image world"),
            "content=hello%20%20world&author_id=111111111111111111&has=image&sort_by=timestamp&sort_order=desc&offset=0"
        );
    }

    #[test]
    fn a_filter_with_nothing_after_it_is_dropped_and_the_words_stay() {
        // `alice` isn't a person we know of here, so `from:` has no answer.
        let tokens = parse(GUILD, "from:nobody hi");
        assert!(matches!(tokens[0].part, Part::Filter(Filter::From)));
        assert_eq!(cleaned("from:nobody hi", &tokens), "nobody hi");
        assert_eq!(query(GUILD, "from:nobody hi"), "content=nobody%20hi&sort_by=timestamp&sort_order=desc&offset=0");
        assert!(!askable(&parse(GUILD, "from:")));
        assert!(!askable(&parse(GUILD, "")));
    }

    #[test]
    fn answers_follow_their_filters_and_the_spelling_matters_where_the_client_checks_it() {
        assert_eq!(
            query(GUILD, "from: alice"),
            "author_id=111111111111111111&sort_by=timestamp&sort_order=desc&offset=0"
        );
        assert_eq!(query(GUILD, "from:@me"), format!("author_id={ME}&sort_by=timestamp&sort_order=desc&offset=0"));
        assert_eq!(
            query(GUILD, "from:Alice Smith#0042 x"),
            "author_id=111111111111111111&content=x&sort_by=timestamp&sort_order=desc&offset=0"
        );
        // The pattern is case-insensitive but the check is not.
        assert_eq!(query(GUILD, "pinned:TRUE"), "content=TRUE&sort_by=timestamp&sort_order=desc&offset=0");
        assert_eq!(query(GUILD, "pinned:true"), "pinned=true&sort_by=timestamp&sort_order=desc&offset=0");
        assert_eq!(query(GUILD, "HAS:-link"), "has=-link&sort_by=timestamp&sort_order=desc&offset=0");
        assert_eq!(query(GUILD, "has:forward"), "has=snapshot&sort_by=timestamp&sort_order=desc&offset=0");
        assert_eq!(query(GUILD, "authorType:bot"), "author_type=bot&sort_by=timestamp&sort_order=desc&offset=0");
    }

    #[test]
    fn in_names_a_channel_by_name_or_id_and_only_in_a_server() {
        assert_eq!(
            query(GUILD, "in:#general hi"),
            "channel_id=5&content=hi&sort_by=timestamp&sort_order=desc&offset=0"
        );
        assert_eq!(
            query(GUILD, "in:\"two words\""),
            "channel_id=6&channel_id=7&sort_by=timestamp&sort_order=desc&offset=0"
        );
        assert_eq!(
            query(GUILD, "in:123456789012345678"),
            "channel_id=123456789012345678&sort_by=timestamp&sort_order=desc&offset=0"
        );
        // In a DM `in:` is only words.
        assert_eq!(query(DM, "in:general"), "content=in%3Ageneral&sort_by=timestamp&sort_order=desc&offset=0");
    }

    #[test]
    fn repeated_words_are_one_and_others_pile_up() {
        assert_eq!(
            query(GUILD, "a from:alice a"),
            "content=a&author_id=111111111111111111&sort_by=timestamp&sort_order=desc&offset=0"
        );
        assert_eq!(query(GUILD, "has:link has:image"), "has=link&has=image&sort_by=timestamp&sort_order=desc&offset=0");
    }

    #[test]
    fn a_filter_is_found_wherever_its_letters_stand() {
        // The client matches `on:` at every position, as it does `from:`.
        let tokens = parse(GUILD, "reason: x");
        assert!(tokens.iter().any(|t| matches!(t.part, Part::Filter(Filter::During))));
    }

    #[test]
    fn dates_become_ids_bounding_the_messages() {
        let id = |ms: i64| Snowflake::from_unix_ms(ms).to_string();
        let tail = "&sort_by=timestamp&sort_order=desc&offset=0";
        const DAY: i64 = 86_400_000;
        // NOW is Wednesday 2026-11-11 12:00 UTC.
        let (today, sunday, nov5) = (1_794_355_200_000_i64, 1_794_096_000_000_i64, 1_793_836_800_000_i64);
        assert_eq!(query(GUILD, "after:today"), format!("min_id={}{tail}", id(today + DAY)));
        assert_eq!(query(GUILD, "before:yesterday"), format!("max_id={}{tail}", id(today - DAY)));
        assert_eq!(query(GUILD, "during:2026-11-05"), format!("min_id={}&max_id={}{tail}", id(nov5), id(nov5 + DAY)));
        assert_eq!(query(GUILD, "on:2026-11-5"), format!("min_id={}&max_id={}{tail}", id(nov5), id(nov5 + DAY)));
        // A week starts on Sunday.
        assert_eq!(query(GUILD, "on:week"), format!("min_id={}&max_id={}{tail}", id(sunday), id(sunday + 7 * DAY)));
        assert_eq!(
            query(GUILD, "on:monday"),
            format!("min_id={}&max_id={}{tail}", id(sunday + DAY), id(sunday + 2 * DAY))
        );
        // A month name is that month of this year, a year the whole of it.
        let november = 1_793_491_200_000_i64;
        assert_eq!(
            query(GUILD, "on:november"),
            format!("min_id={}&max_id={}{tail}", id(november), id(november + 30 * DAY))
        );
        assert!(query(GUILD, "on:2020").starts_with("min_id="));
        // What is no date is words.
        assert_eq!(query(GUILD, "before:nonsense"), "content=nonsense&sort_by=timestamp&sort_order=desc&offset=0");
        assert!(query(GUILD, "on:2026-13-40").starts_with("content="), "there is no such day");
    }

    #[test]
    fn sort_discovery_and_attempts_are_added_in_the_clients_order() {
        let request = Request {
            context: GUILD,
            fields: fields(&parse(GUILD, "hi")),
            sort: Sort::MostRelevant,
            offset: 50,
            discovery: Some(("s".into(), "q".into())),
        };
        assert_eq!(
            request.query(0),
            "content=hi&sort_by=relevance&sort_order=desc&search_session_id=s&search_query_id=q&offset=50"
        );
        assert!(request.query(2).ends_with("&offset=50&attempts=2"));
        let oldest = Request { sort: Sort::Oldest, discovery: None, ..request };
        assert!(oldest.query(0).contains("sort_by=timestamp&sort_order=asc"));
    }

    #[test]
    fn values_are_escaped_as_encode_uri_component_does() {
        assert_eq!(
            query(GUILD, "it's (a) test!*"),
            "content=it's%20(a)%20test!*&sort_by=timestamp&sort_order=desc&offset=0"
        );
        assert_eq!(query(GUILD, "100% ü"), "content=100%25%20%C3%BC&sort_by=timestamp&sort_order=desc&offset=0");
    }

    #[test]
    fn indexing_is_asked_again_up_to_five_times() {
        assert_eq!(retry_delay(None), Duration::from_secs(5));
        assert_eq!(retry_delay(Some("0")), Duration::from_secs(5));
        assert_eq!(retry_delay(Some("x")), Duration::from_secs(5));
        assert_eq!(retry_delay(Some("7")), Duration::from_secs(7));
        assert_eq!(retry_delay(Some("3.9")), Duration::from_secs(3), "parseInt stops at the dot");
        assert_eq!(after_indexing(0, Some("2")), Some((1, Duration::from_secs(2))));
        assert_eq!(after_indexing(4, None), Some((5, Duration::from_secs(5))));
        assert_eq!(after_indexing(5, Some("2")), None, "the sixth answer ends it");
    }

    #[test]
    fn an_answer_keeps_the_first_message_of_each_result() {
        let message = |id: u64| {
            json!({"id": id.to_string(), "channel_id": "9", "author": {"id": "1", "username": "a"},
                   "content": "hi", "timestamp": "2026-11-01T00:00:00+00:00"})
        };
        let page = Page::from_value(&json!({
            "total_results": 2, "doing_deep_historical_index": true, "documents_indexed": 7,
            "messages": [[message(10), message(9)], [message(5)]],
        }))
        .unwrap();
        assert_eq!((page.total, page.indexing, page.documents_indexed), (2, true, 7));
        assert_eq!(page.messages.iter().map(|m| m.id.0).collect::<Vec<_>>(), [10, 5]);
        assert!(Page::from_value(&json!({"messages": null})).is_none());
        assert_eq!((pages(0), pages(26), pages(50_000)), (0, 2, 400));
    }

    #[test]
    fn typing_offers_filters_then_what_they_take() {
        let users = vec![serde_json::from_value::<User>(json!({"id": "1", "username": "alice"})).unwrap()];
        let channels = vec!["general".to_owned(), "rust-help".to_owned()];
        let known = Known { users: &users, channels: &channels };
        let suggestions = |text: &str| {
            let tokens = parse(GUILD, text);
            suggest(text, text.chars().count(), &tokens, GUILD, &known)
        };
        assert_eq!(suggestions(""), [Filter::From, Filter::In, Filter::Has, Filter::Mentions].map(Suggestion::Filter));
        let after_in = suggestions("in:");
        assert_eq!(after_in.len(), 2);
        assert!(matches!(&after_in[0], Suggestion::Value { insert, .. } if insert == "#general"));
        assert!(suggestions("has:vi").iter().any(|s| matches!(s, Suggestion::Value { label, .. } if label == "video")));
        assert!(
            suggestions("from:al").iter().any(|s| matches!(s, Suggestion::Value { label, .. } if label == "alice"))
        );
        assert_eq!(suggestions("pinned:").len(), 2);
        // `in:` in a DM isn't a filter.
        let tokens = parse(DM, "in:");
        assert!(suggest("in:", 3, &tokens, DM, &known).iter().all(|s| matches!(s, Suggestion::Filter(_))));
    }

    #[test]
    fn choosing_a_suggestion_replaces_the_word_and_keeps_the_rest() {
        let tokens = parse(GUILD, "hello has:vi");
        let (text, cursor) = apply(
            "hello has:vi",
            12,
            &tokens,
            &Suggestion::Value { filter: Filter::Has, label: "video".into(), insert: "video".into() },
        );
        assert_eq!((text.as_str(), cursor), ("hello has:video ", 16));
        let tokens = parse(GUILD, "hi fr");
        let (text, _) = apply("hi fr", 5, &tokens, &Suggestion::Filter(Filter::From));
        assert_eq!(text, "hi from: ");
    }

    #[test]
    fn a_name_with_a_space_is_quoted() {
        assert_eq!(quoted("two words"), "\"two words\"");
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("plain"), "plain");
    }
}
