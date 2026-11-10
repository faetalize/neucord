//! Emoji: a server's custom ones and the Unicode ones, which of them the
//! picker and `:name:` autocomplete offer, and what a message says for
//! each. Which custom emoji we may send is `limits.rs`'s
//! ([`Limits::emoji`]); this is the rest of what the client does with them.
//!
//! Unicode emoji come from the `emojis` crate: names and groups from the
//! Unicode CLDR, shortcodes from GitHub's gemoji. The client ships its own
//! table, so a few shortcodes differ from Discord's (gemoji's
//! `slightly_smiling_face` is Discord's `slight_smile`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::limits::{Limits, Unavailable};
use crate::model::{Guild, Snowflake};

/// A custom emoji of a server.
#[derive(Clone, Debug, PartialEq)]
pub struct GuildEmoji {
    pub id: Snowflake,
    pub guild_id: Snowflake,
    pub name: String,
    pub animated: bool,
    /// The server lost the boost level it needed for it. The client reads
    /// the field as sent, so a payload without it counts as lost.
    pub available: bool,
    /// An integration's emoji (Twitch, say), which Nitro doesn't gate.
    pub managed: bool,
    /// Only members with one of these roles may use it; none, anyone.
    pub roles: Vec<Snowflake>,
}

impl GuildEmoji {
    fn from_value(guild_id: Snowflake, value: &Value) -> Option<GuildEmoji> {
        let id = |v: &Value| v.as_str()?.parse().ok().map(Snowflake);
        Some(GuildEmoji {
            id: id(&value["id"])?,
            guild_id,
            name: value["name"].as_str()?.to_owned(),
            animated: value["animated"] == true,
            available: value["available"] == true,
            managed: value["managed"] == true,
            roles: value["roles"].as_array().into_iter().flatten().filter_map(id).collect(),
        })
    }

    /// A server's `emojis`: a list, or the full sync the client's own store
    /// reads (`{"op": "full_sync", "items": [...]}`). An `update` op only
    /// edits a list we'd have kept from an earlier session, and we keep none.
    pub fn list(guild_id: Snowflake, value: &Value) -> Option<Vec<GuildEmoji>> {
        let items = match value {
            Value::Array(items) => items,
            Value::Object(_) if value["op"] == "full_sync" => value["items"].as_array()?,
            _ => return None,
        };
        Some(items.iter().filter_map(|e| GuildEmoji::from_value(guild_id, e)).collect())
    }

    /// What a message says for it: `<:name:id>`, `<a:name:id>` if animated.
    pub fn markup(&self) -> String {
        format!("<{}:{}:{}>", if self.animated { "a" } else { "" }, self.name, self.id)
    }

    /// Whether our roles let us use it (`isUsable` in the client's emoji
    /// store, which has no exception for owners or administrators).
    pub fn usable_with(&self, my_roles: &[Snowflake]) -> bool {
        self.roles.is_empty() || self.roles.iter().any(|r| my_roles.contains(r))
    }
}

/// Where emoji are being used: the open channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// The channel's server; none in a DM.
    pub guild: Option<Snowflake>,
    /// Another server's emoji are allowed in the channel: Use External
    /// Emojis there, or a DM (the client asks only in server channels).
    pub external: bool,
}

/// A Unicode emoji.
#[derive(Debug)]
pub struct UnicodeEmoji {
    /// The emoji itself, in its fully qualified form.
    pub glyph: &'static str,
    /// What `:name:` calls it, the first being its own: gemoji's shortcodes,
    /// or its Unicode name where gemoji has none.
    pub names: Vec<String>,
    pub category: Category,
    /// The Unicode name, without spaces, for searching.
    words: String,
}

/// The picker's groups of Unicode emoji.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    People,
    Nature,
    Food,
    Activities,
    Travel,
    Objects,
    Symbols,
    Flags,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::People,
        Category::Nature,
        Category::Food,
        Category::Activities,
        Category::Travel,
        Category::Objects,
        Category::Symbols,
        Category::Flags,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::People => "People",
            Category::Nature => "Nature",
            Category::Food => "Food & Drink",
            Category::Activities => "Activities",
            Category::Travel => "Travel & Places",
            Category::Objects => "Objects",
            Category::Symbols => "Symbols",
            Category::Flags => "Flags",
        }
    }

    /// The client's "people" holds both of Unicode's smileys and people.
    fn from_group(group: emojis::Group) -> Category {
        use emojis::Group::*;
        match group {
            SmileysAndEmotion | PeopleAndBody => Category::People,
            AnimalsAndNature => Category::Nature,
            FoodAndDrink => Category::Food,
            Activities => Category::Activities,
            TravelAndPlaces => Category::Travel,
            Objects => Category::Objects,
            Symbols => Category::Symbols,
            Flags => Category::Flags,
        }
    }
}

struct Catalog {
    emoji: Vec<UnicodeEmoji>,
    /// Every name, aliases too, to its emoji.
    by_name: HashMap<String, usize>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut emoji = Vec::new();
        for e in emojis::iter() {
            let mut names: Vec<String> = e.shortcodes().map(str::to_owned).collect();
            if names.is_empty() {
                names.push(snake_case(e.name()));
            }
            let words = e.name().chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            emoji.push(UnicodeEmoji { glyph: e.as_str(), names, category: Category::from_group(e.group()), words });
        }
        let mut by_name = HashMap::new();
        for (n, e) in emoji.iter().enumerate() {
            for name in &e.names {
                by_name.entry(name.clone()).or_insert(n);
            }
        }
        Catalog { emoji, by_name }
    })
}

fn snake_case(name: &str) -> String {
    let words: Vec<String> =
        name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    words.join("_")
}

/// The Unicode emoji `:name:` means, by its name or an alias.
pub fn unicode(name: &str) -> Option<&'static UnicodeEmoji> {
    let catalog = catalog();
    catalog.by_name.get(name).map(|&n| &catalog.emoji[n])
}

/// The Unicode emoji of a picker group, in Unicode's order, without skin
/// tone variants (the client's default tone is a setting only the protobuf
/// settings carry).
pub fn unicode_in(category: Category) -> impl Iterator<Item = &'static UnicodeEmoji> {
    catalog().emoji.iter().filter(move |e| e.category == category)
}

/// A custom emoji as the client names it: the first of a name keeps it
/// (after the Unicode emoji, then this channel's server, then the others in
/// the order given), the rest take `~1`, `~2`, … after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub emoji: GuildEmoji,
    pub name: String,
    /// Whether we may send it here; see [`Limits::emoji`].
    pub status: Result<(), Unavailable>,
}

/// One server's emoji we may see, by name.
#[derive(Clone, Debug)]
pub struct Server {
    pub id: Snowflake,
    pub name: String,
    pub emoji: Vec<Named>,
}

/// An emoji of the picker or a search.
#[derive(Clone, Debug)]
pub enum Item {
    Unicode(&'static UnicodeEmoji),
    Custom(Named),
}

impl Item {
    /// What goes in the message box: the emoji itself, or its markup.
    pub fn text(&self) -> String {
        match self {
            Item::Unicode(e) => e.glyph.to_owned(),
            Item::Custom(n) => n.emoji.markup(),
        }
    }

    /// The name `:name:` would write.
    pub fn name(&self) -> &str {
        match self {
            Item::Unicode(e) => &e.names[0],
            Item::Custom(n) => &n.name,
        }
    }

    /// Why it can't be sent here, if it can't.
    pub fn unavailable(&self) -> Option<Unavailable> {
        match self {
            Item::Unicode(_) => None,
            Item::Custom(n) => n.status.err(),
        }
    }
}

/// What a search found: what we may send, and what Nitro would let us.
#[derive(Debug, Default)]
pub struct Found {
    pub unlocked: Vec<Item>,
    pub locked: Vec<Item>,
}

/// A message after its `:name:`s are written out.
#[derive(Debug, PartialEq)]
pub struct Translated {
    pub text: String,
    /// The custom emoji in it that Nitro would be needed for. The client
    /// sends the message all the same, and tells the sender.
    pub locked: Vec<GuildEmoji>,
}

/// The emoji the open channel's account has: every server's custom ones we
/// may see, and the Unicode ones.
pub struct Library {
    pub servers: Vec<Server>,
    pub place: Place,
}

impl Library {
    /// `guilds` in the order the client lists servers: the channel's own
    /// first, then the rest as the rail has them.
    pub fn new<'a>(guilds: impl IntoIterator<Item = &'a Guild>, limits: &Limits, place: &Place) -> Library {
        // Unicode names are taken first, so a custom `:smile:` is `smile~1`.
        let mut taken: HashMap<String, usize> = catalog().emoji.iter().map(|e| (e.names[0].clone(), 1)).collect();
        let servers = guilds
            .into_iter()
            .map(|guild| {
                let mut usable: Vec<&GuildEmoji> =
                    guild.emojis.iter().filter(|e| e.usable_with(&guild.my_roles)).collect();
                usable.sort_by(|a, b| a.name.cmp(&b.name));
                let emoji = usable
                    .into_iter()
                    .map(|e| {
                        let seen = taken.entry(e.name.clone()).or_insert(0);
                        let name = if *seen == 0 { e.name.clone() } else { format!("{}~{}", e.name, seen) };
                        *seen += 1;
                        Named { status: limits.emoji(e, place), emoji: e.clone(), name }
                    })
                    .collect();
                Server { id: guild.id, name: guild.name.clone(), emoji }
            })
            .collect();
        Library { servers, place: *place }
    }

    fn custom(&self) -> impl Iterator<Item = &Named> {
        self.servers.iter().flat_map(|s| &s.emoji)
    }

    /// The custom sections of the picker: each server's emoji, those we may
    /// send first, then those locked. The client hides what the channel's
    /// permissions rule out, and shows the rest locked.
    pub fn picker(&self) -> Vec<Server> {
        self.servers
            .iter()
            .filter_map(|server| {
                let (open, locked): (Vec<&Named>, Vec<&Named>) = server
                    .emoji
                    .iter()
                    .filter(|n| n.status != Err(Unavailable::NotHere))
                    .partition(|n| n.status.is_ok());
                let emoji: Vec<Named> = open.into_iter().chain(locked).cloned().collect();
                (!emoji.is_empty()).then(|| Server { id: server.id, name: server.name.clone(), emoji })
            })
            .collect()
    }

    /// Emoji matching `query`, best first: what we may send (at most `max`),
    /// and what Nitro would unlock. Matching ignores case, `_` and spaces,
    /// and finds the query anywhere in a name. The client also ranks by how
    /// often an emoji was used, which a setting only the protobuf settings
    /// carry.
    pub fn search(&self, query: &str, max: usize) -> Found {
        let needle: String = query.to_lowercase().chars().filter(|c| !matches!(c, ' ' | '_')).collect();
        let plain = query.to_lowercase();
        let mut found: Vec<(u32, Item)> = Vec::new();
        for e in &catalog().emoji {
            let hit = e.names.iter().any(|n| n.replace('_', "").contains(&needle)) || e.words.contains(&needle);
            if hit {
                found.push((score(&e.names[0], &plain), Item::Unicode(e)));
            }
        }
        for n in self.custom() {
            if n.name.to_lowercase().replace('_', "").contains(&needle) {
                found.push((score(&n.name, &plain), Item::Custom(n.clone())));
            }
        }
        found.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.name().cmp(y.name())));
        let mut results = Found::default();
        for (_, item) in found {
            match item.unavailable() {
                None if results.unlocked.len() < max => results.unlocked.push(item),
                Some(Unavailable::Nitro) => results.locked.push(item),
                _ => {}
            }
        }
        results
    }

    /// Writes out the `:name:`s of `text` as the client does when sending:
    /// a custom emoji's name becomes its markup, a Unicode one's the emoji.
    /// Code, links and markup already written are left alone, and a
    /// `:name:` that is no emoji stays as typed.
    pub fn translate(&self, text: &str) -> Translated {
        let by_name: HashMap<&str, &Named> = self.custom().map(|n| (n.name.as_str(), n)).collect();
        let by_id: HashMap<Snowflake, &Named> = self.custom().map(|n| (n.emoji.id, n)).collect();
        let mut locked: Vec<GuildEmoji> = Vec::new();
        let mut lock = |n: &Named| {
            if n.status == Err(Unavailable::Nitro) {
                locked.push(n.emoji.clone());
            }
        };
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let rest = &chars[i..];
            let skip = match rest[0] {
                '`' => code_len(rest),
                '<' => markup_len(rest),
                'h' => link_len(rest),
                _ => 0,
            };
            if skip > 0 {
                let span = &rest[..skip];
                if let Some(id) = raw_emoji_id(span)
                    && let Some(n) = by_id.get(&id)
                {
                    lock(n);
                }
                out.extend(span);
                i += skip;
                continue;
            }
            if rest[0] == ':'
                && let Some(len) = shortcode_len(rest)
            {
                let name: String = rest[1..len - 1].iter().collect();
                match by_name.get(name.as_str()) {
                    Some(n) => {
                        lock(n);
                        out.push_str(&n.emoji.markup());
                    }
                    None => match unicode(&name) {
                        Some(e) => out.push_str(e.glyph),
                        None => out.extend(&rest[..len]),
                    },
                }
                i += len;
                continue;
            }
            out.push(rest[0]);
            i += 1;
        }
        Translated { text: out, locked }
    }
}

/// How well a name fits a query (`getSearchResultsOrder`): 1, plus 4 for the
/// name being the query, 2 for it as a whole word of the name (words split
/// at `_`; a plural `s` is let by), and 1 for starting with it.
fn score(name: &str, query: &str) -> u32 {
    let lower = name.to_lowercase();
    1 + 4 * u32::from(lower == query)
        + 2 * u32::from(is_word(&lower, query) || is_word(name, query))
        + u32::from(lower.starts_with(query))
}

fn is_word(name: &str, query: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let query: Vec<char> = query.chars().collect();
    let boundary = |c: Option<&char>| c.is_none_or(|&c| c == '_' || c.is_ascii_uppercase());
    (0..=chars.len().saturating_sub(query.len())).any(|at| {
        let end = at + query.len();
        chars[at..].starts_with(&query)
            && (at == 0 || boundary(chars.get(at - 1)))
            && (boundary(chars.get(end)) || (chars.get(end) == Some(&'s') && boundary(chars.get(end + 1))))
    })
}

/// The length of the code span or block at the start of `rest`, or 0: a
/// run of backticks up to the next run of as many.
fn code_len(rest: &[char]) -> usize {
    let ticks = rest.iter().take_while(|&&c| c == '`').count();
    let mut at = ticks;
    while at < rest.len() {
        let run = rest[at..].iter().take_while(|&&c| c == '`').count();
        if run == ticks {
            return at + run;
        }
        at += run.max(1);
    }
    0
}

/// The length of `<a:name:id>`, `<:name:id>` or any other markup in angle
/// brackets (a mention, a timestamp) at the start of `rest`, or 0.
fn markup_len(rest: &[char]) -> usize {
    let inner = rest[1..].iter().take_while(|&&c| c != '>' && c != '<' && !c.is_whitespace()).count();
    if rest.get(1 + inner) == Some(&'>') && inner > 0 { inner + 2 } else { 0 }
}

/// The id of the emoji in `<a:name:id>` or `<:name:id>`.
fn raw_emoji_id(markup: &[char]) -> Option<Snowflake> {
    let inner: String = markup.get(1..markup.len() - 1)?.iter().collect();
    let inner = inner.strip_prefix('a').unwrap_or(&inner);
    let mut parts = inner.strip_prefix(':')?.splitn(2, ':');
    let (name, id) = (parts.next()?, parts.next()?);
    let word = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    word.then(|| id.parse().ok().map(Snowflake))?
}

/// The length of a link at the start of `rest`, or 0.
fn link_len(rest: &[char]) -> usize {
    let text: String = rest.iter().take(8).collect();
    if text.starts_with("http://") || text.starts_with("https://") {
        rest.iter().take_while(|c| !c.is_whitespace()).count()
    } else {
        0
    }
}

/// The length of the `:name:` at the start of `rest`, or `None`: the name is
/// anything but whitespace and colons.
fn shortcode_len(rest: &[char]) -> Option<usize> {
    let name = rest[1..].iter().take_while(|&&c| c != ':' && !c.is_whitespace()).count();
    (name > 0 && rest.get(1 + name) == Some(&':')).then_some(name + 2)
}

/// Why a message that carries locked emoji went out all the same: the words
/// the client says. `external` is Use External Emojis in the channel, where
/// Nitro is all that's missing.
pub fn locked_notice(locked: &[GuildEmoji], external: bool) -> &'static str {
    if locked.iter().any(|e| e.animated) {
        "This emoji doesn't work because it's animated. Discord Nitro can solve all of that, check User Settings > Nitro for details"
    } else if external {
        "This emoji doesn't work here because it's from a different server. Discord Nitro can solve all of that, check User Settings > Nitro for details"
    } else {
        "This emoji doesn't work here because it's from a different server. Upgrade to Discord Nitro to use emoji from other servers"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::experiments::Experiments;
    use crate::limits::Nitro;

    const HOME: Snowflake = Snowflake(1);
    const OTHER: Snowflake = Snowflake(2);

    fn custom(id: &str, name: &str, extra: Value) -> Value {
        let mut e = json!({"id": id, "name": name, "available": true, "roles": []});
        e.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        e
    }

    fn guild(id: &str, roles: &[&str], emojis: Vec<Value>) -> Guild {
        let mut guild: Guild =
            serde_json::from_value(json!({"id": id, "name": format!("Server {id}"), "emojis": emojis})).unwrap();
        guild.my_roles = roles.iter().map(|r| Snowflake(r.parse().unwrap())).collect();
        guild
    }

    fn limits(nitro: &Nitro, experiments: &Experiments) -> Limits<'static> {
        // The limits only live for the call; leaking keeps the helper small.
        Limits {
            nitro: Box::leak(Box::new(nitro.clone())),
            experiments: Box::leak(Box::new(experiments.clone())),
            me: Snowflake(7),
        }
    }

    fn library(guilds: &[Guild], nitro: Value, place: Place) -> Library {
        let nitro = Nitro::from_user(&nitro);
        Library::new(guilds, &limits(&nitro, &Experiments::default()), &place)
    }

    fn here(external: bool) -> Place {
        Place { guild: Some(HOME), external }
    }

    fn names(item: &[Item]) -> Vec<&str> {
        item.iter().map(Item::name).collect()
    }

    #[test]
    fn emojis_read_as_a_list_or_a_full_sync() {
        let list = json!([custom("9", "blob", json!({"animated": true, "managed": true, "roles": ["4"]}))]);
        let emoji = GuildEmoji::list(HOME, &list).unwrap();
        assert_eq!(emoji.len(), 1);
        let e = &emoji[0];
        assert_eq!((e.id, e.guild_id, e.name.as_str()), (Snowflake(9), HOME, "blob"));
        assert!(e.animated && e.managed && e.available);
        assert_eq!(e.roles, [Snowflake(4)]);
        assert_eq!(e.markup(), "<a:blob:9>");
        assert_eq!(GuildEmoji::list(HOME, &json!({"op": "full_sync", "items": list})), Some(emoji));
        assert_eq!(GuildEmoji::list(HOME, &json!({"op": "update", "writes": list, "deletes": []})), None);
        // The client reads the field as sent: without it, the server lost it.
        let bare = GuildEmoji::list(HOME, &json!([{"id": "9", "name": "blob"}])).unwrap();
        assert!(!bare[0].available && !bare[0].animated);
    }

    #[test]
    fn a_role_restricted_emoji_shows_only_to_those_with_the_role() {
        let emoji = vec![custom("9", "zclub", json!({"roles": ["40"]})), custom("10", "zall", json!({}))];
        let nitro = json!({"premium_type": 2});
        let listed = |roles: &[&str]| -> Vec<String> {
            let library = library(&[guild("1", roles, emoji.clone())], nitro.clone(), here(true));
            library.servers[0].emoji.iter().map(|n| n.name.clone()).collect()
        };
        assert_eq!(listed(&[]), ["zall"]);
        assert_eq!(listed(&["41"]), ["zall"]);
        assert_eq!(listed(&["40"]), ["zall", "zclub"]);
    }

    #[test]
    fn what_we_may_send_follows_the_client() {
        let own = GuildEmoji::list(HOME, &json!([custom("9", "own", json!({}))])).unwrap().remove(0);
        let foreign = GuildEmoji::list(OTHER, &json!([custom("10", "away", json!({}))])).unwrap().remove(0);
        let nitro = |v: Value| Nitro::from_user(&v);
        let (none, basic) = (nitro(json!({})), nitro(json!({"premium_type": 3})));
        let experiments = Experiments::default();
        let at = |n: &Nitro, e: &GuildEmoji, place| limits(n, &experiments).emoji(e, &place);

        assert_eq!(at(&none, &own, here(false)), Ok(()), "a server's own emoji, to anyone");
        assert_eq!(at(&none, &foreign, here(true)), Err(Unavailable::Nitro));
        assert_eq!(at(&basic, &foreign, here(true)), Ok(()));
        assert_eq!(at(&basic, &foreign, here(false)), Err(Unavailable::NotHere), "no Use External Emojis");
        assert_eq!(at(&none, &foreign, here(false)), Err(Unavailable::NotHere), "the permission is asked first");
        let dm = Place { guild: None, external: true };
        assert_eq!(at(&none, &own, dm), Err(Unavailable::Nitro), "in a DM every server's emoji is another's");
        assert_eq!(at(&basic, &own, dm), Ok(()));

        let managed = GuildEmoji { managed: true, ..foreign.clone() };
        assert_eq!(at(&none, &managed, here(true)), Ok(()), "an integration's emoji skips Nitro");
        let animated = GuildEmoji { animated: true, ..own.clone() };
        assert_eq!(at(&none, &animated, here(true)), Err(Unavailable::Nitro));
        assert_eq!(
            at(&nitro(json!({"perks": {"active_perks_bitmask": [4]}})), &animated, here(true)),
            Err(Unavailable::Nitro)
        );
        assert_eq!(at(&nitro(json!({"perks": {"active_perks_bitmask": [2]}})), &animated, here(true)), Ok(()));
        let lost = GuildEmoji { available: false, ..own };
        assert_eq!(at(&nitro(json!({"premium_type": 2})), &lost, here(true)), Err(Unavailable::ServerBoost));
    }

    #[test]
    fn the_picker_hides_what_the_channel_rules_out_and_locks_the_rest() {
        let home = guild("1", &[], vec![custom("9", "zb", json!({"animated": true})), custom("8", "za", json!({}))]);
        let away = guild("2", &[], vec![custom("10", "zc", json!({}))]);
        let lost = guild("3", &[], vec![custom("11", "zd", json!({"available": false}))]);
        // Emojis Everywhere alone.
        let nitro = json!({"perks": {"active_perks_bitmask": [4]}});
        let guilds = [home, away, lost];

        let sections = library(&guilds, nitro.clone(), here(true)).picker();
        fn shown(s: &Server) -> Vec<(&str, bool)> {
            s.emoji.iter().map(|n| (n.name.as_str(), n.status.is_ok())).collect()
        }
        assert_eq!(shown(&sections[0]), [("za", true), ("zb", false)], "the usable first, the locked after");
        assert_eq!(shown(&sections[1]), [("zc", true)]);
        assert_eq!(shown(&sections[2]), [("zd", false)], "lost with the boosts: shown locked");

        let sections = library(&guilds, nitro, here(false)).picker();
        let ids: Vec<u64> = sections.iter().map(|s| s.id.0).collect();
        assert_eq!(
            ids,
            [1],
            "other servers' emoji are left out without Use External Emojis, a lost one's too, and so are their sections"
        );
    }

    #[test]
    fn same_names_take_a_number_after_unicode_and_the_first() {
        let home = guild("1", &[], vec![custom("9", "smile", json!({})), custom("8", "blob", json!({}))]);
        let away = guild("2", &[], vec![custom("10", "blob", json!({})), custom("11", "blob", json!({}))]);
        let library = library(&[home, away], json!({"premium_type": 2}), here(true));
        let names: Vec<&str> = library.servers.iter().flat_map(|s| s.emoji.iter().map(|n| n.name.as_str())).collect();
        assert_eq!(names, ["blob", "smile~1", "blob~1", "blob~2"]);
    }

    #[test]
    fn search_ranks_whole_names_first_and_splits_what_nitro_would_unlock() {
        let home = guild(
            "1",
            &[],
            vec![
                custom("9", "zonk_blob_party", json!({})),
                custom("8", "zonkblob", json!({})),
                custom("7", "zonk", json!({})),
            ],
        );
        let away = guild("2", &[], vec![custom("10", "zonk_far", json!({}))]);
        let free = library(&[home, away], json!({}), here(true));

        let found = free.search("zonk", 2);
        assert_eq!(
            names(&found.unlocked),
            ["zonk", "zonk_blob_party"],
            "the whole name, then a word of it, up to the cap"
        );
        assert_eq!(names(&found.locked), ["zonk_far"], "the other server's, for Nitro");
        let all = free.search("zonk", 10);
        assert_eq!(names(&all.unlocked), ["zonk", "zonk_blob_party", "zonkblob"]);

        assert_eq!(free.search("ZONK", 10).unlocked[0].name(), "zonk", "whatever the case");
        // Spaces and underscores don't count; the Unicode name finds too.
        assert_eq!(names(&free.search("zonk blob", 10).unlocked), ["zonk_blob_party", "zonkblob"]);
        let tears = free.search("face with tears", 50).unlocked;
        assert!(tears.iter().any(|i| matches!(i, Item::Unicode(e) if e.glyph == "😂")));
        assert!(free.search("zzzzqq", 10).unlocked.is_empty());
    }

    #[test]
    fn unicode_emoji_by_name_and_alias() {
        assert_eq!(unicode("thumbsup").map(|e| e.glyph), Some("👍"));
        assert_eq!(unicode("+1").map(|e| e.glyph), Some("👍"));
        assert!(unicode("no_such_emoji_at_all").is_none());
        assert!(unicode_in(Category::Flags).any(|e| e.glyph == "🏁"));
        assert!(Category::ALL.iter().all(|&c| unicode_in(c).next().is_some()));
        // Every emoji has a name to call it by.
        assert!(Category::ALL.iter().flat_map(|&c| unicode_in(c)).all(|e| !e.names[0].is_empty()));
    }

    #[test]
    fn sending_writes_names_out_as_the_client_does() {
        let home =
            guild("1", &[], vec![custom("9", "blob", json!({})), custom("8", "spin", json!({"animated": true}))]);
        let away = guild("2", &[], vec![custom("10", "far", json!({}))]);
        let free = library(&[home.clone(), away.clone()], json!({}), here(true));
        let say = |text: &str| free.translate(text).text;

        assert_eq!(say("hi :blob: and :smile:"), "hi <:blob:9> and 😄");
        assert_eq!(say(":unknown: :blob"), ":unknown: :blob", "no emoji, no change");
        assert_eq!(say("a:blob:b"), "a<:blob:9>b");
        assert_eq!(
            say("`:blob:` ``:smile:`` ```\n:blob:\n``` :blob:"),
            "`:blob:` ``:smile:`` ```\n:blob:\n``` <:blob:9>"
        );
        assert_eq!(say("<:blob:9> <@5> https://x.test/:blob:"), "<:blob:9> <@5> https://x.test/:blob:");
        assert_eq!(say("::blob: :: :"), ":<:blob:9> :: :", "a name is no colons or spaces");

        // The whole message goes out, and says what Nitro would have done.
        let sent = free.translate(":blob: :far: <:spin:8> :spin:");
        assert_eq!(sent.text, "<:blob:9> <:far:10> <:spin:8> <a:spin:8>");
        let locked: Vec<&str> = sent.locked.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(locked, ["far", "spin", "spin"]);
        assert!(free.translate("<:unknown:99>").locked.is_empty(), "an emoji we don't know isn't checked");
        let full = library(&[home, away], json!({"premium_type": 2}), here(true));
        assert!(full.translate(":far: :spin:").locked.is_empty());
    }

    /// The client's parser (`emoji` rule, 451909) looks a typed `:name:` up
    /// among the emoji we can see, whoever may send them, and only a role we
    /// lack takes an emoji out of that list. `invalidEmojis` only collects
    /// the Nitro-locked ones for the notice: the content keeps the markup.
    #[test]
    fn a_typed_name_of_an_emoji_we_may_not_send_is_written_as_the_client_does() {
        let home = guild(
            "1",
            &[],
            vec![
                custom("9", "zlost", json!({"available": false})),
                custom("8", "zclub", json!({"roles": ["40"]})),
                custom("7", "zspin", json!({"animated": true})),
            ],
        );
        let away = guild("2", &[], vec![custom("10", "zfar", json!({}))]);
        let guilds = [home, away];
        let typed = ":zlost: :zclub: :zspin: :zfar:";

        // No Nitro: the other server's and the animated one are locked.
        let free = library(&guilds, json!({}), here(true)).translate(typed);
        assert_eq!(free.text, "<:zlost:9> :zclub: <a:zspin:7> <:zfar:10>");
        let locked: Vec<&str> = free.locked.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(locked, ["zspin", "zfar"], "only Nitro makes an emoji invalid");

        // Use External Emojis missing: written out, and nothing to tell.
        let nitro = json!({"premium_type": 2});
        let no_permission = library(&guilds, nitro.clone(), here(false)).translate(typed);
        assert_eq!(no_permission.text, "<:zlost:9> :zclub: <a:zspin:7> <:zfar:10>");
        assert!(no_permission.locked.is_empty());

        // The server lost the emoji's boosts: written out all the same.
        let lost = library(&guilds, nitro, here(true)).translate(":zlost:");
        assert_eq!((lost.text.as_str(), lost.locked.len()), ("<:zlost:9>", 0));

        // A role we lack: not an emoji for us, so the text stays.
        assert!(free.text.contains(":zclub:") && no_permission.text.contains(":zclub:"));
    }

    #[test]
    fn shortcodes_are_gemojis_not_discords() {
        assert_eq!(library(&[], json!({}), here(true)).translate(":slightly_smiling_face:").text, "🙂");
        // Discord's own name for it isn't gemoji's, so it stays as typed.
        assert!(unicode("slight_smile").is_none());
        assert_eq!(library(&[], json!({}), here(true)).translate(":slight_smile:").text, ":slight_smile:");
    }

    #[test]
    fn the_notice_says_what_nitro_would_do() {
        let animated = GuildEmoji {
            animated: true,
            ..GuildEmoji::list(HOME, &json!([custom("9", "a", json!({}))])).unwrap().remove(0)
        };
        let plain = GuildEmoji { animated: false, ..animated.clone() };
        assert!(locked_notice(&[plain.clone(), animated], true).contains("animated"));
        assert!(locked_notice(std::slice::from_ref(&plain), true).contains("can solve all of that"));
        assert!(locked_notice(std::slice::from_ref(&plain), false).contains("Upgrade to Discord Nitro"));
    }
}
