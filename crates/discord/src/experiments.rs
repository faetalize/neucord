//! Experiments: which variant of a feature Discord gives this account and
//! each of its servers. Rollouts and some limits come this way rather than
//! from the client's code, so the client's own definitions say what a
//! variant means, and only READY says which one applies.
//!
//! READY carries three kinds. Apex experiments (50258) are assigned by
//! Discord per user, install or server, keyed by murmur3 of the name. Older
//! user experiments (343446) are assigned buckets. Older server experiments
//! (343446) arrive as rollout rules, which the client evaluates itself for
//! each server; so does this. Apex assignments can also arrive after READY,
//! in a `STATE_UPDATE` ([`Experiments::apply_update`]).

use crate::model::{Guild, Snowflake};
use serde_json::Value;
use std::collections::HashMap;

/// What the client's experiments return for "not in it": the default config.
pub const NOT_ELIGIBLE: i64 = -1;
/// The control group: in the experiment, still on the default behaviour.
pub const CONTROL: i64 = 0;

/// murmur3 (x86, 32 bits, seed 0) of `key`, as the client's `v3` computes it.
pub fn murmur3(key: &str) -> u32 {
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;
    let data = key.as_bytes();
    let mut h: u32 = 0;
    let mix = |k: u32| k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
    let (chunks, tail) = data.as_chunks::<4>();
    for &chunk in chunks {
        h ^= mix(u32::from_le_bytes(chunk));
        h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xe654_6b64);
    }
    if !tail.is_empty() {
        let k = tail.iter().rev().fold(0u32, |k, &b| (k << 8) | u32::from(b));
        h ^= mix(k);
    }
    h ^= data.len() as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// An Apex assignment: `[hash, variant, flags, revision, tracked variant, config]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub variant: i64,
    pub revision: i64,
    flags: u64,
    /// A payload some variants read their config from.
    pub config: Option<Value>,
}

impl Assignment {
    const IS_OVERRIDE: u64 = 1;
    const USE_AS_ELIGIBILITY: u64 = 8;

    fn from_value(value: &Value) -> Option<(u32, Assignment)> {
        let hash = u32::try_from(value[0].as_u64()?).ok()?;
        let assignment = Assignment {
            variant: value[1].as_i64()?,
            flags: value[2].as_u64().unwrap_or(0),
            revision: value[3].as_i64().unwrap_or(0),
            config: value.get(5).filter(|c| !c.is_null()).cloned(),
        };
        Some((hash, assignment))
    }

    /// As READY sends it, for keeping.
    fn to_value(&self, hash: u32) -> Value {
        serde_json::json!([hash, self.variant, self.flags, self.revision, null, self.config])
    }

    fn is_override(&self) -> bool {
        self.flags & Self::IS_OVERRIDE != 0
    }

    /// Only says the user may be in a server-level experiment, without a variant.
    fn use_as_eligibility(&self) -> bool {
        self.flags & Self::USE_AS_ELIGIBILITY != 0
    }
}

/// What the filters of a server rollout look at.
#[derive(Clone, Debug, PartialEq)]
pub struct GuildFacts<'a> {
    pub id: Snowflake,
    pub features: &'a [String],
    pub member_count: Option<u64>,
    pub hub_type: Option<u8>,
    pub has_vanity_url: bool,
}

impl<'a> From<&'a Guild> for GuildFacts<'a> {
    fn from(guild: &'a Guild) -> Self {
        GuildFacts {
            id: guild.id,
            features: &guild.features,
            member_count: guild.member_count,
            hub_type: guild.hub_type,
            has_vanity_url: guild.vanity_url_code.is_some(),
        }
    }
}

/// Inclusive bounds. Discord sends them as numbers or as strings (ids), and
/// a bound that doesn't parse fails the filter, as in the client's `O`.
#[derive(Clone, Debug, PartialEq)]
struct Range {
    min: Option<Value>,
    max: Option<Value>,
}

impl Range {
    fn from_pairs(pairs: &Value) -> Range {
        let mut range = Range { min: None, max: None };
        for pair in pairs.as_array().into_iter().flatten() {
            let value = Some(pair[1].clone()).filter(|v| !v.is_null());
            match pair[0].as_u64().map(|k| k as u32) {
                Some(k) if k == murmur3("min_id") => range.min = value,
                Some(k) if k == murmur3("max_id") => range.max = value,
                _ => {}
            }
        }
        range
    }

    fn contains(&self, n: i128) -> bool {
        let bound = |v: &Option<Value>| -> Result<Option<i128>, ()> {
            match v {
                None => Ok(None),
                Some(Value::Number(n)) => n.as_i64().map(|n| Some(i128::from(n))).ok_or(()),
                Some(Value::String(s)) => s.parse().map(Some).map_err(|_| ()),
                Some(_) => Err(()),
            }
        };
        match (bound(&self.min), bound(&self.max)) {
            (Ok(min), Ok(max)) => min.is_none_or(|min| n >= min) && max.is_none_or(|max| n <= max),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Filter {
    GuildIds(Vec<Snowflake>),
    IdRange(Range),
    AgeDays(Range),
    MemberCount(Range),
    HasFeature(Vec<String>),
    HubTypes(Vec<i64>),
    HasVanityUrl(bool),
    /// `murmur3("{key}:{guild}")`, doubled, modulo 10000, under `target`.
    InRangeByHash {
        key: String,
        target: u64,
    },
    /// A filter this doesn't know, which the client lets through too.
    Unknown,
}

impl Filter {
    fn from_value(value: &Value) -> Filter {
        let Some(kind) = value[0].as_u64().map(|k| k as u32) else { return Filter::Unknown };
        let args = &value[1];
        // The first argument's value, for filters that take one.
        let first = &args[0][1];
        let strings =
            |v: &Value| v.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        match kind {
            k if k == murmur3("guild_ids") => {
                // The last `guild_ids` argument wins, as in the client.
                let ids = args
                    .as_array()
                    .into_iter()
                    .flatten()
                    .rfind(|pair| pair[0].as_u64() == Some(u64::from(murmur3("guild_ids"))))
                    .map(|pair| &pair[1]);
                let ids = ids.and_then(Value::as_array).into_iter().flatten();
                Filter::GuildIds(ids.filter_map(|id| id.as_str()?.parse().ok().map(Snowflake)).collect())
            }
            k if k == murmur3("guild_id_range") => Filter::IdRange(Range::from_pairs(args)),
            k if k == murmur3("guild_age_range_days") => Filter::AgeDays(Range::from_pairs(args)),
            k if k == murmur3("guild_member_count_range") => Filter::MemberCount(Range::from_pairs(args)),
            k if k == murmur3("guild_has_feature") => Filter::HasFeature(strings(first)),
            k if k == murmur3("guild_hub_types") => {
                Filter::HubTypes(first.as_array().into_iter().flatten().filter_map(Value::as_i64).collect())
            }
            k if k == murmur3("guild_has_vanity_url") => Filter::HasVanityUrl(first.as_bool().unwrap_or(false)),
            k if k == murmur3("guild_in_range_by_hash") => {
                let (mut key, mut target) = (String::new(), 0);
                for pair in args.as_array().into_iter().flatten() {
                    match pair[0].as_u64().map(|k| k as u32) {
                        Some(k) if k == murmur3("hash_key") => key = js_string(&pair[1]),
                        // `parseInt`: a number, or the leading digits of a string.
                        Some(k) if k == murmur3("target") => {
                            target = match &pair[1] {
                                Value::String(s) => {
                                    s.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
                                }
                                v => v.as_f64().map(|f| f as u64).unwrap_or(0),
                            }
                        }
                        _ => {}
                    }
                }
                Filter::InRangeByHash { key, target }
            }
            _ => Filter::Unknown,
        }
    }

    fn passes(&self, guild: &GuildFacts, now_ms: i64) -> bool {
        match self {
            Filter::GuildIds(ids) => ids.contains(&guild.id),
            Filter::IdRange(range) => range.contains(i128::from(guild.id.0)),
            Filter::AgeDays(range) => {
                let days = (now_ms - guild.id.unix_ms()).div_euclid(86_400_000);
                range.contains(i128::from(days))
            }
            Filter::MemberCount(range) => guild.member_count.is_some_and(|n| range.contains(i128::from(n))),
            Filter::HasFeature(features) => features.iter().any(|f| guild.features.contains(f)),
            Filter::HubTypes(types) => guild.hub_type.is_some_and(|t| types.contains(&i64::from(t))),
            Filter::HasVanityUrl(wanted) => guild.has_vanity_url == *wanted,
            Filter::InRangeByHash { key, target } => {
                // The client doubles a positive hash before the modulo.
                let hash = u64::from(murmur3(&format!("{key}:{}", guild.id.0)));
                let position = if hash > 0 { hash * 2 } else { 0 };
                position % 10_000 < *target
            }
            Filter::Unknown => true,
        }
    }
}

/// How the client turns a JSON value into text in a template string.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        v => v.to_string(),
    }
}

/// Buckets over positions 0..10000, for the servers the filters let in.
#[derive(Clone, Debug, PartialEq)]
struct Population {
    buckets: Vec<(i64, Vec<(u64, u64)>)>,
    filters: Vec<Filter>,
}

impl Population {
    fn from_parts(buckets: &Value, filters: &Value) -> Population {
        let buckets = buckets
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                let ranges = b[1].as_array()?.iter().filter_map(|r| Some((r["s"].as_u64()?, r["e"].as_u64()?)));
                Some((b[0].as_i64()?, ranges.collect()))
            })
            .collect();
        let filters = filters.as_array().into_iter().flatten().map(Filter::from_value).collect();
        Population { buckets, filters }
    }

    /// `[buckets, filters]`.
    fn from_value(value: &Value) -> Population {
        Population::from_parts(&value[0], &value[1])
    }
}

/// The client's `er`: the first population whose filters pass decides,
/// control if no bucket covers the position; "not eligible" stops the search.
fn evaluate(populations: &[Population], guild: &GuildFacts, position: u64, now_ms: i64) -> Option<i64> {
    for population in populations {
        if !population.filters.iter().all(|f| f.passes(guild, now_ms)) {
            continue;
        }
        let bucket = population
            .buckets
            .iter()
            .find(|(_, ranges)| ranges.iter().any(|&(s, e)| position >= s && position < e))
            .map_or(CONTROL, |&(bucket, _)| bucket);
        if bucket == NOT_ELIGIBLE {
            break;
        }
        return Some(bucket);
    }
    None
}

/// An older server experiment's rollout, as READY sends it.
#[derive(Clone, Debug, PartialEq)]
struct Rollout {
    hash_key: Option<String>,
    populations: Vec<Population>,
    overrides: HashMap<Snowflake, i64>,
    overrides_formatted: Vec<Vec<Population>>,
    holdout: Option<(String, i64)>,
}

impl Rollout {
    /// The client's layout is `[hash, hash_key, revision, populations,
    /// overrides, overrides_formatted, holdout_name, holdout_bucket, …]`.
    /// Ours has one population inline instead, as its buckets and filters,
    /// so one more field.
    fn from_value(value: &Value) -> Option<(u32, Rollout)> {
        let hash = u32::try_from(value[0].as_u64()?).ok()?;
        // Inline, `value[3]` is the buckets: `[[bucket, [{s, e}]]]`.
        let inline = value[3][0][0].is_i64();
        let (populations, rest) = if inline {
            (vec![Population::from_parts(&value[3], &value[4])], 5)
        } else {
            (value[3].as_array().into_iter().flatten().map(Population::from_value).collect(), 4)
        };
        let mut overrides = HashMap::new();
        for group in value[rest].as_array().into_iter().flatten() {
            let Some(bucket) = group["b"].as_i64() else { continue };
            for id in group["k"].as_array().into_iter().flatten() {
                if let Some(id) = id.as_str().and_then(|id| id.parse().ok()).map(Snowflake) {
                    overrides.insert(id, bucket);
                }
            }
        }
        let overrides_formatted = value[rest + 1]
            .as_array()
            .into_iter()
            .flatten()
            .map(|group| group.as_array().into_iter().flatten().map(Population::from_value).collect())
            .collect();
        let holdout = match (value[rest + 2].as_str(), value[rest + 3].as_i64()) {
            (Some(name), Some(bucket)) => Some((name.to_string(), bucket)),
            _ => None,
        };
        let hash_key = Some(&value[1]).filter(|k| !k.is_null()).map(js_string);
        Some((hash, Rollout { hash_key, populations, overrides, overrides_formatted, holdout }))
    }
}

/// Everything READY says about experiments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Experiments {
    /// Apex assignments by unit (1 user, 2 install, 3 server), unit id and hash.
    apex: HashMap<(u64, String), HashMap<u32, Assignment>>,
    user: HashMap<u32, i64>,
    guild: HashMap<u32, Rollout>,
}

impl Experiments {
    const USER: u64 = 1;
    const GUILD: u64 = 3;

    pub fn from_ready(ready: &Value) -> Experiments {
        let mut apex = HashMap::new();
        for (unit, ids) in ready["apex_experiments"]["assignments"].as_object().into_iter().flatten() {
            let Ok(unit) = unit.parse() else { continue };
            for (id, entry) in ids.as_object().into_iter().flatten() {
                let assignments = entry["assignments"].as_array().into_iter().flatten();
                apex.insert((unit, id.clone()), assignments.filter_map(Assignment::from_value).collect());
            }
        }
        // `[hash, revision, bucket, override, population, hash_result, …]`
        let user = ready["experiments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| Some((u32::try_from(e[0].as_u64()?).ok()?, e[2].as_i64()?)))
            .collect();
        let guild =
            ready["guild_experiments"].as_array().into_iter().flatten().filter_map(Rollout::from_value).collect();
        Experiments { apex, user, guild }
    }

    /// Our own Apex assignments, as the client keeps them between launches
    /// (the persisted `ApexExperimentStore`, 50258), in READY's layout.
    pub fn user_cache(&self, me: Snowflake) -> Vec<Value> {
        let Some(assignments) = self.apex.get(&(Self::USER, me.0.to_string())) else { return Vec::new() };
        let mut cached: Vec<_> = assignments.iter().map(|(hash, a)| (*hash, a.to_value(*hash))).collect();
        cached.sort_by_key(|(hash, _)| *hash);
        cached.into_iter().map(|(_, value)| value).collect()
    }

    /// [`Self::user_cache`] read back: what [`Self::apex_user`] says for `me`.
    pub fn from_user_cache(me: Snowflake, cached: &[Value]) -> Experiments {
        let assignments: HashMap<u32, Assignment> = cached.iter().filter_map(Assignment::from_value).collect();
        let mut apex = HashMap::new();
        apex.insert((Self::USER, me.0.to_string()), assignments);
        Experiments { apex, ..Default::default() }
    }

    /// Fold in the assignments of a `STATE_UPDATE`: each unit's entry is
    /// replaced whole and the rest stays, as the client's
    /// `setExperimentAssignments` does (50258).
    pub fn apply_update(&mut self, update: Experiments) {
        self.apex.extend(update.apex);
    }

    /// Our Apex assignment for a user experiment, or `None` for its default
    /// config, as the client's `getConfig` decides.
    pub fn apex_user(&self, me: Snowflake, name: &str) -> Option<&Assignment> {
        self.apex_assignment(Self::USER, &me.0.to_string(), name).filter(|a| !a.use_as_eligibility())
    }

    /// A server's Apex assignment for a server experiment: only for a user
    /// Discord made eligible, unless forced (50258's
    /// `getEvaluationAndAssignment`).
    pub fn apex_guild(&self, me: Snowflake, guild: Snowflake, name: &str) -> Option<&Assignment> {
        let user = self.apex_assignment(Self::USER, &me.0.to_string(), name)?;
        if user.is_override() {
            return Some(user);
        }
        if !user.use_as_eligibility() {
            return None;
        }
        self.apex_assignment(Self::GUILD, &guild.0.to_string(), name)
    }

    fn apex_assignment(&self, unit: u64, id: &str, name: &str) -> Option<&Assignment> {
        self.apex.get(&(unit, id.to_string()))?.get(&murmur3(name))
    }

    /// Our bucket in an older user experiment.
    pub fn user(&self, name: &str) -> i64 {
        self.user.get(&murmur3(name)).copied().unwrap_or(NOT_ELIGIBLE)
    }

    /// A server's bucket in an older server experiment: 343446's
    /// `getGuildExperimentDescriptor`.
    pub fn guild(&self, name: &str, guild: &GuildFacts) -> i64 {
        self.guild_at(name, guild, glib::real_time() / 1000).unwrap_or(NOT_ELIGIBLE)
    }

    fn guild_at(&self, name: &str, guild: &GuildFacts, now_ms: i64) -> Option<i64> {
        let rollout = self.guild.get(&murmur3(name))?;
        if let Some(&bucket) = rollout.overrides.get(&guild.id) {
            return Some(bucket).filter(|&b| b != NOT_ELIGIBLE);
        }
        let key = rollout.hash_key.as_deref().unwrap_or(name);
        let position = u64::from(murmur3(&format!("{key}:{}", guild.id.0))) % 10_000;
        for populations in &rollout.overrides_formatted {
            if let Some(bucket) = evaluate(populations, guild, position, now_ms) {
                return Some(bucket);
            }
        }
        let bucket = evaluate(&rollout.populations, guild, position, now_ms)?;
        // A server in the holdout's control group stays out.
        if let Some((holdout, control)) = &rollout.holdout
            && holdout != name
            && self.guild_at(holdout, guild, now_ms) == Some(*control)
        {
            return None;
        }
        Some(bucket)
    }
}

/// Debug aid for the weekly check: with `NEUCORD_DEBUG_EXPERIMENTS=<dir>`,
/// READY writes `<dir>/experiments.txt`, every assignment and rollout with the
/// names listed in `<dir>/names.txt` (one a line, from the client's
/// definitions) put to their hashes, and `<dir>/ready-shape.txt`, READY's
/// [`outline`](crate::shape::outline). Neither holds our user id; the first
/// names our servers by id, for a local diff week to week.
pub(crate) fn debug_dump(ready: &Value, experiments: &Experiments, me: Snowflake, guilds: &[Guild]) {
    let Some(dir) = std::env::var_os("NEUCORD_DEBUG_EXPERIMENTS").map(std::path::PathBuf::from) else { return };
    let names: Vec<String> = std::fs::read_to_string(dir.join("names.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let text = dump(ready, experiments, me, guilds, &names, glib::real_time() / 1000);
    let mut shape = String::new();
    for (key, value) in ready.as_object().into_iter().flatten() {
        shape.push_str(&format!("{key}: {}\n", crate::shape::outline(value)));
    }
    // The shape last: whoever waits on the dump can take it as done.
    let write = |file: &str, text: &str| {
        let partial = dir.join(format!("{file}.partial"));
        std::fs::write(&partial, text).and_then(|()| std::fs::rename(&partial, dir.join(file)))
    };
    if let Err(e) = write("experiments.txt", &text).and_then(|()| write("ready-shape.txt", &shape)) {
        eprintln!("experiments: cannot write the dump to {}: {e}", dir.display());
    }
}

fn dump(
    ready: &Value,
    experiments: &Experiments,
    me: Snowflake,
    guilds: &[Guild],
    names: &[String],
    now_ms: i64,
) -> String {
    let named: HashMap<u32, &str> = names.iter().map(|n| (murmur3(n), n.as_str())).collect();
    let name = |hash: u32| named.get(&hash).map_or_else(|| format!("#{hash}"), |n| n.to_string());
    let mut lines = Vec::new();
    for ((unit, id), assignments) in &experiments.apex {
        let owner = match *unit {
            Experiments::USER if *id == me.0.to_string() => "user".to_string(),
            Experiments::GUILD => format!("guild {id}"),
            // An install's id, or another user's, isn't worth keeping.
            other => format!("unit {other}"),
        };
        for (hash, a) in assignments {
            let config = a.config.as_ref().map(|c| format!(" config {c}")).unwrap_or_default();
            lines.push(format!(
                "apex {owner} {}: variant {} revision {} flags {}{config}",
                name(*hash),
                a.variant,
                a.revision,
                a.flags
            ));
        }
    }
    for (hash, bucket) in &experiments.user {
        lines.push(format!("user {}: bucket {bucket}", name(*hash)));
    }
    for raw in ready["guild_experiments"].as_array().into_iter().flatten() {
        let Some(hash) = raw[0].as_u64().and_then(|h| u32::try_from(h).ok()) else { continue };
        let mut rule = raw.clone();
        rule[0] = Value::Null;
        lines.push(format!("rollout {}: {rule}", name(hash)));
        let Some(exp) = named.get(&hash) else { continue };
        for guild in guilds {
            if let Some(bucket) = experiments.guild_at(exp, &GuildFacts::from(guild), now_ms) {
                lines.push(format!("guild {} {exp}: bucket {bucket}", guild.id));
            }
        }
    }
    // Rules sent per account; READY's `country_code` decides them, and is
    // where the user is, so it stays out.
    if !ready["regional_feature_config"].is_null() {
        lines.push(format!("regional_feature_config: {}", ready["regional_feature_config"]));
    }
    lines.sort();
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn murmur3_matches_the_clients_hashes() {
        assert_eq!(murmur3(""), 0);
        assert_eq!(murmur3("hello"), 0x248b_fa47);
        // Filter names as Discord sends them hashed.
        assert_eq!(murmur3("guild_has_feature"), 1_604_612_045);
        assert_eq!(murmur3("guild_member_count_range"), 2_918_402_255);
        assert_eq!(murmur3("min_id"), 3_399_957_344);
    }

    const ME: Snowflake = Snowflake(10);

    fn ready(apex: Value) -> Value {
        json!({ "apex_experiments": { "assignments": apex, "installation": "x" } })
    }

    #[test]
    fn apex_user_assignment_or_default() {
        let ready = ready(json!({ "1": { "10": { "assignments": [
            [murmur3("2099-01-on"), 1, 2, 3],
            [murmur3("2099-01-payload"), 2, 0, 1, null, "{\"limit\":5}"],
            [murmur3("2099-01-eligible-only"), 0, 8, 1],
        ], "evaluation_id": "e" } } }));
        let experiments = Experiments::from_ready(&ready);
        let on = experiments.apex_user(ME, "2099-01-on").unwrap();
        assert_eq!((on.variant, on.revision, on.config.as_ref()), (1, 3, None));
        assert_eq!(experiments.apex_user(ME, "2099-01-payload").unwrap().config, Some(json!("{\"limit\":5}")));
        // Not assigned, or only eligibility: the default config.
        assert_eq!(experiments.apex_user(ME, "2099-01-absent"), None);
        assert_eq!(experiments.apex_user(ME, "2099-01-eligible-only"), None);
        // Someone else's assignments aren't ours.
        assert_eq!(experiments.apex_user(Snowflake(11), "2099-01-on"), None);
    }

    #[test]
    fn apex_guild_needs_the_user_to_be_eligible() {
        let ready = ready(json!({
            "1": { "10": { "assignments": [
                [murmur3("2099-01-eligible"), 0, 8, 1],
                [murmur3("2099-01-forced"), 4, 1, 1],
                [murmur3("2099-01-not-eligible"), 0, 0, 1],
            ] } },
            "3": { "20": { "assignments": [
                [murmur3("2099-01-eligible"), 2, 0, 1],
                [murmur3("2099-01-not-eligible"), 2, 0, 1],
            ] } },
        }));
        let experiments = Experiments::from_ready(&ready);
        let variant = |name| experiments.apex_guild(ME, Snowflake(20), name).map(|a| a.variant);
        assert_eq!(variant("2099-01-eligible"), Some(2));
        assert_eq!(variant("2099-01-forced"), Some(4));
        assert_eq!(variant("2099-01-not-eligible"), None);
        assert_eq!(experiments.apex_guild(ME, Snowflake(21), "2099-01-eligible"), None);
    }

    #[test]
    fn older_user_experiments_are_assigned_buckets() {
        let ready = json!({ "experiments": [[murmur3("2099-01_user"), 3, 2, -1, 1, 517, 0, 0, null, null, null]] });
        let experiments = Experiments::from_ready(&ready);
        assert_eq!(experiments.user("2099-01_user"), 2);
        assert_eq!(experiments.user("2099-01_other"), NOT_ELIGIBLE);
    }

    const NOW: i64 = 1_800_000_000_000;

    fn facts(id: u64, features: &[String]) -> GuildFacts<'_> {
        GuildFacts { id: Snowflake(id), features, member_count: Some(500), hub_type: None, has_vanity_url: false }
    }

    fn rollout(name: &str, populations: Value, overrides: Value, formatted: Value) -> Experiments {
        // Our layout: the one population inline, then the rest.
        let buckets = &populations[0][0];
        let filters = &populations[0][1];
        let ready = json!({ "guild_experiments": [
            [murmur3(name), null, 0, buckets, filters, overrides, formatted, null, null, 0, 0],
        ] });
        Experiments::from_ready(&ready)
    }

    fn feature(name: &str) -> Value {
        json!([murmur3("guild_has_feature"), [[murmur3("guild_features"), [name]]]])
    }

    #[test]
    fn a_feature_filter_picks_the_servers() {
        let name = "2099-01_feature";
        let experiments =
            rollout(name, json!([[[[1, [{"s": 0, "e": 10000}]]], [feature("COMMUNITY")]]]), json!([]), json!([]));
        let community = ["COMMUNITY".to_string()];
        assert_eq!(experiments.guild_at(name, &facts(1, &community), NOW), Some(1));
        assert_eq!(experiments.guild_at(name, &facts(1, &[]), NOW), None);
    }

    #[test]
    fn overrides_win_and_not_eligible_stops() {
        let name = "2099-01_overrides";
        let experiments = rollout(
            name,
            json!([[[[-1, [{"s": 0, "e": 10000}]]], []]]),
            json!([{"b": 2, "k": ["5"]}, {"b": -1, "k": ["6"]}]),
            json!([]),
        );
        assert_eq!(experiments.guild_at(name, &facts(5, &[]), NOW), Some(2));
        assert_eq!(experiments.guild_at(name, &facts(6, &[]), NOW), None);
        // Everyone else lands on "not eligible".
        assert_eq!(experiments.guild_at(name, &facts(7, &[]), NOW), None);
    }

    #[test]
    fn positions_outside_every_bucket_are_control() {
        let name = "2099-01_positions";
        let id = 42u64;
        let position = u64::from(murmur3(&format!("{name}:{id}"))) % 10_000;
        let around = json!([[[[3, [{"s": position, "e": position + 1}]]], []]]);
        assert_eq!(rollout(name, around, json!([]), json!([])).guild_at(name, &facts(id, &[]), NOW), Some(3));
        let elsewhere = json!([[[[3, [{"s": (position + 1) % 10_000, "e": (position + 1) % 10_000 + 1}]]], []]]);
        assert_eq!(rollout(name, elsewhere, json!([]), json!([])).guild_at(name, &facts(id, &[]), NOW), Some(CONTROL));
    }

    #[test]
    fn formatted_overrides_come_before_the_populations() {
        let name = "2099-01_formatted";
        let ids = json!([murmur3("guild_ids"), [[murmur3("guild_ids"), ["9"]]]]);
        let experiments = rollout(
            name,
            json!([[[[-1, [{"s": 0, "e": 10000}]]], []]]),
            json!([]),
            json!([[[[[1, [{"s": 0, "e": 10000}]]], [ids]]]]),
        );
        assert_eq!(experiments.guild_at(name, &facts(9, &[]), NOW), Some(1));
        assert_eq!(experiments.guild_at(name, &facts(8, &[]), NOW), None);
    }

    #[test]
    fn ranges_take_numbers_and_ids() {
        let members =
            json!([murmur3("guild_member_count_range"), [[murmur3("min_id"), 100], [murmur3("max_id"), null]]]);
        let ids = json!([murmur3("guild_id_range"), [[murmur3("min_id"), null], [murmur3("max_id"), "50"]]]);
        let all = json!([[1, [{"s": 0, "e": 10000}]]]);
        let name = "2099-01_ranges";
        let experiments = rollout(name, json!([[all, [members, ids]]]), json!([]), json!([]));
        assert_eq!(experiments.guild_at(name, &facts(50, &[]), NOW), Some(1));
        assert_eq!(experiments.guild_at(name, &facts(51, &[]), NOW), None);
        let small = GuildFacts { member_count: Some(99), ..facts(50, &[]) };
        assert_eq!(experiments.guild_at(name, &small, NOW), None);
        let unknown = GuildFacts { member_count: None, ..facts(50, &[]) };
        assert_eq!(experiments.guild_at(name, &unknown, NOW), None);
    }

    #[test]
    fn hash_range_filter_doubles_the_hash() {
        let filter = |target: u64| {
            Filter::from_value(&json!([
                murmur3("guild_in_range_by_hash"),
                [[murmur3("hash_key"), 123], [murmur3("target"), target.to_string()]]
            ]))
        };
        let id = 77u64;
        let position = u64::from(murmur3(&format!("123:{id}"))) * 2 % 10_000;
        assert!(filter(position + 1).passes(&facts(id, &[]), NOW));
        assert!(!filter(position).passes(&facts(id, &[]), NOW));
    }

    #[test]
    fn unknown_filters_let_everyone_through() {
        let name = "2099-01_unknown";
        let all = json!([[1, [{"s": 0, "e": 10000}]]]);
        let experiments = rollout(name, json!([[all, [[123, [[1, 2]]]]]]), json!([]), json!([]));
        assert_eq!(experiments.guild_at(name, &facts(1, &[]), NOW), Some(1));
    }

    #[test]
    fn a_holdouts_control_group_stays_out() {
        let all = |bucket: i64| json!([[bucket, [{"s": 0, "e": 10000}]]]);
        let ready = json!({ "guild_experiments": [
            [murmur3("2099-01_feature"), null, 0, all(1), [], [], [], "2099-01_holdout", 0, 0, 0],
            [murmur3("2099-01_holdout"), null, 0, all(0), [], [{"b": 1, "k": ["3"]}], [], null, null, 0, 0],
        ] });
        let experiments = Experiments::from_ready(&ready);
        assert_eq!(experiments.guild_at("2099-01_feature", &facts(2, &[]), NOW), None);
        assert_eq!(experiments.guild_at("2099-01_feature", &facts(3, &[]), NOW), Some(1));
    }

    #[test]
    fn reads_the_clients_layout_too() {
        let name = "2099-01_layout";
        let population = json!([[[1, [{"s": 0, "e": 10000}]]], [feature("COMMUNITY")]]);
        let ready = json!({ "guild_experiments": [
            [murmur3(name), null, 0, [population], [{"b": 2, "k": ["4"]}], [], null, null, 0, 0],
        ] });
        let experiments = Experiments::from_ready(&ready);
        let community = ["COMMUNITY".to_string()];
        assert_eq!(experiments.guild_at(name, &facts(1, &community), NOW), Some(1));
        assert_eq!(experiments.guild_at(name, &facts(4, &[]), NOW), Some(2));
    }

    #[test]
    fn the_dump_names_what_it_can_and_leaves_out_our_id_and_country() {
        let ready = json!({
            "apex_experiments": { "assignments": { "1": { "10": { "assignments": [
                [murmur3("2099-01-known"), 1, 2, 3],
                [murmur3("2099-01-unknown"), 0, 0, 1],
            ] } } } },
            "guild_experiments": [
                [murmur3("2099-01_rollout"), null, 0, [[1, [{"s": 0, "e": 10000}]]], [], [], [], null, null, 0, 0],
            ],
            "guilds": [{ "id": "30", "name": "G" }],
            "regional_feature_config": { "age_gated_features": 0 },
            "country_code": "ZZ",
        });
        let guilds: Vec<Guild> = vec![serde_json::from_value(ready["guilds"][0].clone()).unwrap()];
        let names = ["2099-01-known".to_string(), "2099-01_rollout".to_string()];
        let text = dump(&ready, &Experiments::from_ready(&ready), ME, &guilds, &names, NOW);
        let unknown = murmur3("2099-01-unknown");
        assert_eq!(
            text,
            format!(
                "apex user #{unknown}: variant 0 revision 1 flags 0\n\
                 apex user 2099-01-known: variant 1 revision 3 flags 2\n\
                 guild 30 2099-01_rollout: bucket 1\n\
                 regional_feature_config: {{\"age_gated_features\":0}}\n\
                 rollout 2099-01_rollout: [null,null,0,[[1,[{{\"e\":10000,\"s\":0}}]]],[],[],[],null,null,0,0]\n"
            )
        );
    }

    #[test]
    fn age_counts_whole_days_since_the_servers_id() {
        // Discord's epoch is 2015-01-01.
        let created = Snowflake(((NOW - 10 * 86_400_000 - 1 - 1_420_070_400_000) as u64) << 22);
        let age = |min: i64, max: i64| {
            Filter::from_value(&json!([
                murmur3("guild_age_range_days"),
                [[murmur3("min_id"), min], [murmur3("max_id"), max]]
            ]))
        };
        let guild = GuildFacts { id: created, ..facts(0, &[]) };
        assert!(age(10, 10).passes(&guild, NOW));
        assert!(!age(11, 20).passes(&guild, NOW));
    }
}
