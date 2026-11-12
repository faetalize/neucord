//! How Neucord identifies itself to Discord: as the web app, in a browser
//! named Neucord. REST and the gateway both read from here.
//!
//! The properties are the ones the web client sends from any browser, in its
//! order, with Neucord's own values where the client reads the browser's. The
//! client builds them once (`getSuperProperties`, 613412), adds the build
//! and its launch id, and brings the heartbeat session and the focus up to
//! date before every request (`prepareRequest`, 852799, over 618651). The
//! gateway sends the same object (`handleIdentify`, 225123), refreshed only
//! by the last request, plus what only it carries.

use std::cell::RefCell;

use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use serde_json::{Value, json};

use crate::Snowflake;

/// The API version is the one the official client uses (its `API_VERSION`):
/// a version no official client speaks would set every request apart (#73).
/// `tools/discord-watch` reads it from `API_BASE` and reports when the
/// client moves on.
pub const API_BASE: &str = "https://discord.com/api/v9";
/// The gateway query is the client's too, in its order. It compresses with
/// `zlib-stream` where it has no zstd, as in every browser; without
/// `compress` only its debug setting connects (#75).
pub const GATEWAY_QUERY: &str = "encoding=json&v=9&compress=zlib-stream";
pub const GATEWAY_URL: &str = "wss://gateway.discord.gg/?encoding=json&v=9&compress=zlib-stream";

/// The web build Neucord's behaviour is checked against, which it names as
/// `client_build_number`: Discord withholds about half of an account's Apex
/// experiments from a session that names none (#190). The weekly check
/// raises it once that week's changes are handled (`discord-watch`'s
/// `raise-build`), so it never claims a build Neucord hasn't been compared
/// with; one some weeks old is what a tab left open sends.
pub const CLIENT_BUILD_NUMBER: u32 = 668923;

/// The locale every request names: `X-Discord-Locale`, and the `locale` the
/// client's sticker packs request carries, which are the same setting there.
pub const LOCALE: &str = "en-US";

const NAME: &str = "Neucord";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn user_agent() -> String {
    format!("{NAME}/{VERSION} (Linux; libsoup)")
}

/// `navigator.languages[0]`, which the client sends as `system_locale`
/// (175259): the first language of the environment, as a BCP 47 tag. A
/// session without one (`C`, `POSIX`) is in the language Neucord is written
/// in.
fn locale_tag<S: AsRef<str>>(names: &[S]) -> String {
    names
        .iter()
        .map(|name| name.as_ref().split(['.', '@']).next().unwrap_or_default())
        .find(|name| !matches!(*name, "" | "C" | "POSIX"))
        .map_or_else(|| LOCALE.to_owned(), |name| name.replace('_', "-"))
}

/// The system's IANA time zone, which the client sends as `X-Discord-Timezone`
/// (`Intl.DateTimeFormat().resolvedOptions().timeZone`, 472229).
pub fn timezone() -> Option<String> {
    timezone_from(
        std::env::var("TZ").ok().as_deref(),
        std::fs::read_link("/etc/localtime").ok().as_deref().and_then(std::path::Path::to_str),
        std::fs::read_to_string("/etc/timezone").ok().as_deref(),
    )
}

/// What `TZ`, where `/etc/localtime` points and `/etc/timezone` say, in that
/// order: a name such as `Europe/Berlin`. A POSIX rule or a file path in `TZ`
/// names no zone.
fn timezone_from(tz: Option<&str>, localtime: Option<&str>, etc_timezone: Option<&str>) -> Option<String> {
    let named = |name: &str| {
        let name = name.trim().trim_start_matches(':');
        let valid = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
            && !name.starts_with('/');
        valid.then(|| name.to_owned())
    };
    tz.and_then(named)
        .or_else(|| localtime.and_then(|path| path.split_once("zoneinfo/")).and_then(|(_, name)| named(name)))
        .or_else(|| etc_timezone.and_then(named))
}

/// A JavaScript object's keys keep the order they were first assigned in;
/// assigning again changes the value, not the place. A property set to
/// `undefined` (a heartbeat session that lapsed) stays in its place and is
/// left out of the JSON, as `JSON.stringify` does; here it is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties(Vec<(&'static str, Option<Value>)>);

impl Properties {
    fn assign(&mut self, key: &'static str, value: Option<Value>) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    fn set(&mut self, key: &'static str, value: impl Into<Value>) {
        self.assign(key, Some(value.into()));
    }

    fn unset(&mut self, key: &'static str) {
        self.assign(key, None);
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.as_ref())
    }

    /// The names, in the order sent.
    pub fn names(&self) -> Vec<&'static str> {
        self.0.iter().filter(|(_, v)| v.is_some()).map(|(k, _)| *k).collect()
    }

    fn json(&self) -> String {
        serde_json::to_string(self).expect("properties are plain data")
    }
}

impl Serialize for Properties {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.0.iter().filter_map(|(k, v)| Some((k, v.as_ref()?))) {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// The session the client's activity heartbeat opens while the app is in
/// use (903063), which the properties name as `client_heartbeat_session_id`.
/// It lasts until half an hour passes without use, and the client keeps it
/// across launches, for whoever is signed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatSession {
    uuid: String,
    /// In milliseconds since 1970.
    created_at: i64,
    last_used: i64,
}

impl HeartbeatSession {
    const LIFETIME_MS: i64 = 30 * 60 * 1000;

    /// `aE` in 604594.
    fn expired(&self, now_ms: i64) -> bool {
        Self::LIFETIME_MS + self.last_used - now_ms <= 0
    }
}

/// How often the activity heartbeat looks after the session while the app
/// is in use (`b` in 903063).
pub const BEAT_INTERVAL_MS: i64 = 15 * 60 * 1000;

/// What the client's heartbeat (`er` in 903063) does to the stored session
/// when it runs: in use, a missing or lapsed session is replaced and the
/// session counts as used now; not in use, a lapsed one is dropped and
/// nothing is made.
fn renewed(
    stored: Option<HeartbeatSession>,
    active: bool,
    now_ms: i64,
    new_id: impl FnOnce() -> String,
) -> Option<HeartbeatSession> {
    let mut session = stored;
    if active {
        let mut current = match session.take() {
            Some(s) if !s.expired(now_ms) => s,
            _ => HeartbeatSession { uuid: new_id(), created_at: now_ms, last_used: now_ms },
        };
        current.last_used = now_ms;
        return Some(current);
    }
    session.filter(|s| !s.expired(now_ms))
}

/// How long until the heartbeat next runs once the app is in use: at once
/// the first time, then every [`BEAT_INTERVAL_MS`] from the last run
/// (`X` in 903063).
pub fn beat_delay_ms(since_last_beat_ms: Option<i64>) -> i64 {
    since_last_beat_ms.map_or(0, |since| (BEAT_INTERVAL_MS - since).max(0))
}

/// The Apex assignments of an account, which the client keeps between
/// launches in its store, by user id (50258). One for each account.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    /// The account's own, as READY sent them
    /// (`[hash, variant, flags, revision, tracked variant, config]`). Only
    /// the gateway's choice of capabilities reads them before READY, so
    /// only the account's own are kept (see [`crate::obfuscation`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<Value>,
}

impl Cache {
    /// Reads what [`Self::to_json`] wrote; anything else is an empty cache,
    /// which only costs the client's first-launch behaviour.
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a cache is plain data")
    }

    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }
}

/// The one heartbeat session the client stores (`LAST_CLIENT_HEARTBEAT_SESSION`
/// in 903063), with the account whose token it was opened under. The client
/// drops it whenever its token changes, so it is only ever brought back by
/// the same account on the next launch. A window of its own, as a browser
/// profile is, would keep a slot of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    account: Snowflake,
    session: HeartbeatSession,
}

impl Slot {
    /// Reads what [`Self::to_json`] wrote; anything else is no slot.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a slot is plain data")
    }
}

/// The state behind the properties. One for the account that is shown, as in
/// the client, where one set of properties serves every request.
struct State {
    props: Properties,
    focused: bool,
    /// Whom the tokens in use belong to, once known; none before a first
    /// login's READY.
    account: Option<Snowflake>,
    /// Whether a token is in use: the heartbeat runs only then.
    signed_in: bool,
    heartbeat: Option<HeartbeatSession>,
    /// Nobody has signed in yet in this run: the stored slot may be the one
    /// this run continues.
    launching: bool,
    /// The account's own Apex assignments, see [`Cache`].
    assignments: Vec<Value>,
    /// Whether they put the account in the experiment that hides channels,
    /// which IDENTIFY asks for in its capabilities.
    obfuscation: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new());
}

fn now_ms() -> i64 {
    glib::real_time() / 1000
}

impl State {
    fn new() -> Self {
        State {
            props: Self::base(&glib::language_names().iter().map(|n| n.as_str()).collect::<Vec<_>>()),
            focused: true,
            account: None,
            signed_in: false,
            heartbeat: None,
            launching: true,
            assignments: Vec::new(),
            obfuscation: false,
        }
    }

    /// `eA`, `eh`, `eI` and the rest of 613412, then `eg`'s additions, in
    /// the order the client assigns them. Values the browser would give are
    /// Neucord's: its user agent and version, the language of the system.
    fn base(languages: &[&str]) -> Properties {
        let mut props = Properties::default();
        props.set("os", "Linux");
        props.set("browser", NAME);
        props.set("device", "");
        props.set("device_model_name", "");
        props.set("system_locale", locale_tag(languages));
        // Neucord has no `window`, so none of the globals the check looks
        // for, the known client mods', are there (70298).
        props.set("has_client_mods", false);
        props.set("browser_user_agent", user_agent());
        props.set("browser_version", VERSION);
        // `platform.js` reads no OS version from Neucord's user agent.
        props.set("os_version", "");
        // Neucord was opened from nothing, as a tab with no referrer is.
        props.set("referrer", "");
        props.set("referring_domain", "");
        props.set("referrer_current", "");
        props.set("referring_domain_current", "");
        props.set("release_channel", "stable");
        props.set("client_build_number", CLIENT_BUILD_NUMBER);
        props.set("client_event_source", Value::Null);
        props.set("client_launch_id", glib::uuid_string_random().as_str());
        // 129230 always adds the key: a string from libdiscore's WASM where it
        // is loaded, else null. Neucord has none, and forges none.
        props.set("launch_signature", Value::Null);
        props
    }

    /// `D` in 618651: before a request, the properties name the heartbeat
    /// session and the focus as they are now.
    fn refresh(&mut self, now_ms: i64) {
        let session = self.heartbeat.as_ref().filter(|s| !s.expired(now_ms)).map(|s| s.uuid.as_str());
        if session != self.props.get("client_heartbeat_session_id").and_then(Value::as_str) {
            match session {
                Some(uuid) => self.props.set("client_heartbeat_session_id", uuid),
                None => self.props.unset("client_heartbeat_session_id"),
            }
        }
        let state = if self.focused { "focused" } else { "unfocused" };
        if self.props.get("client_app_state") != Some(&json!(state)) {
            self.props.set("client_app_state", state);
        }
    }

    fn active(&self) -> bool {
        self.signed_in && self.focused
    }
}

/// `X-Super-Properties`: the properties, base64 of their JSON, as of the
/// request they go with.
pub(crate) fn super_properties() -> String {
    let json = STATE.with_borrow_mut(|s| {
        s.refresh(now_ms());
        s.props.json()
    });
    openssl::base64::encode_block(json.as_bytes())
}

/// `gateway_connect_reasons` (469177): what asked for the connection. The
/// app's view (`AppSkeleton`) does, from the moment it exists; Neucord
/// connects only with its window up.
const CONNECT_REASONS: &str = "AppSkeleton";

/// IDENTIFY's `properties` (`handleIdentify` in 225123): the properties as
/// the last request left them, `client_app_state` as it is now, and what only
/// the gateway sends. The client has made requests by then, which have named
/// its heartbeat session; Neucord's first IDENTIFY may come before any, so it
/// brings the session up to date as a request would.
pub fn identify_properties() -> Properties {
    STATE.with_borrow_mut(|s| {
        s.refresh(now_ms());
        let mut props = s.props.clone();
        props.set("client_app_state", if s.focused { "focused" } else { "unfocused" });
        props.set("is_fast_connect", false);
        props.set("gateway_connect_reasons", CONNECT_REASONS);
        props
    })
}

/// Whether the window has focus (`isAppFocused`, 531685).
pub fn set_focused(focused: bool) {
    STATE.with_borrow_mut(|s| s.focused = focused);
}

/// Whether the app is in use, which the heartbeat's session needs: a token,
/// and the window in focus (`K` in 903063).
pub fn is_active() -> bool {
    STATE.with_borrow(State::active)
}

/// A token is in use for `account`, with what was stored for it: the heartbeat
/// `slot` and its `cache` of assignments. The same account again (a
/// reconnect) changes nothing. Any other token ends the session (`Q` in
/// 903063), so the stored slot is the one this run continues only for the
/// account it was opened for, on the first sign-in of a run.
pub fn sign_in(account: Option<Snowflake>, slot: Option<Slot>, cache: Cache) {
    STATE.with_borrow_mut(|s| {
        if !(s.signed_in && s.account == account) {
            let continued = slot.filter(|slot| s.launching && Some(slot.account) == account);
            s.heartbeat = continued.map(|slot| slot.session);
            s.obfuscation = crate::obfuscation::assigned(&cache.assignments);
            s.assignments = cache.assignments;
            s.account = account;
        }
        s.launching = false;
        s.signed_in = true;
    });
}

/// READY gave the account a new token (`UPDATE_TOKEN`): the session of the
/// old one ends.
pub fn token_replaced() {
    STATE.with_borrow_mut(|s| s.heartbeat = None);
}

/// READY named the account a first login belongs to.
pub fn signed_in_as(account: Snowflake) {
    STATE.with_borrow_mut(|s| s.account = Some(account));
}

pub fn sign_out() {
    STATE.with_borrow_mut(|s| {
        s.launching = false;
        s.signed_in = false;
        s.account = None;
        s.heartbeat = None;
        s.assignments.clear();
        s.obfuscation = false;
    });
}

pub fn account() -> Option<Snowflake> {
    STATE.with_borrow(|s| s.account)
}

/// The heartbeat runs (`er`): a session is opened if the app is in use and
/// has none, and counts as used.
pub fn heartbeat() {
    STATE.with_borrow_mut(|s| {
        let active = s.active();
        s.heartbeat = renewed(s.heartbeat.take(), active, now_ms(), || glib::uuid_string_random().to_string());
    });
}

/// What READY says the account is assigned: the next IDENTIFY goes by it.
pub fn learn_assignments(assignments: Vec<Value>) {
    STATE.with_borrow_mut(|s| {
        s.obfuscation = crate::obfuscation::assigned(&assignments);
        s.assignments = assignments;
    });
}

/// The account's assignments, to keep for the next launch.
pub fn cache() -> Cache {
    STATE.with_borrow(|s| Cache { assignments: s.assignments.clone() })
}

/// The session, to keep for the next launch: none until the account is known.
pub fn slot() -> Option<Slot> {
    STATE.with_borrow(|s| Some(Slot { account: s.account?, session: s.heartbeat.clone()? }))
}

/// Whether IDENTIFY asks for hidden channels.
pub(crate) fn obfuscation() -> bool {
    STATE.with_borrow(|s| s.obfuscation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_api_version_everywhere() {
        let version = API_BASE.rsplit_once("/v").map(|(_, v)| v).unwrap();
        assert!(GATEWAY_QUERY.split('&').any(|param| param == format!("v={version}")));
        assert!(GATEWAY_URL.ends_with(&format!("?{GATEWAY_QUERY}")));
    }

    #[test]
    fn the_locale_is_the_first_language_of_the_system() {
        assert_eq!(locale_tag(&["pt_BR.UTF-8", "pt_BR", "pt", "C"]), "pt-BR");
        assert_eq!(locale_tag(&["de_DE@euro", "de"]), "de-DE");
        assert_eq!(locale_tag(&["en"]), "en");
        assert_eq!(locale_tag(&["C"]), LOCALE);
        assert_eq!(locale_tag(&["POSIX", "C"]), LOCALE);
        assert_eq!(locale_tag::<&str>(&[]), LOCALE);
    }

    #[test]
    fn the_timezone_is_the_systems_iana_name() {
        let zone = timezone_from;
        assert_eq!(zone(Some("Asia/Tokyo"), None, None).as_deref(), Some("Asia/Tokyo"));
        assert_eq!(zone(Some(":America/Sao_Paulo"), None, None).as_deref(), Some("America/Sao_Paulo"));
        assert_eq!(zone(None, Some("/usr/share/zoneinfo/Europe/Berlin"), None).as_deref(), Some("Europe/Berlin"));
        assert_eq!(zone(None, Some("../usr/share/zoneinfo/Etc/GMT+3"), None).as_deref(), Some("Etc/GMT+3"));
        assert_eq!(zone(None, None, Some("Europe/Lisbon\n")).as_deref(), Some("Europe/Lisbon"));
        // TZ wins; what is no name is passed over.
        assert_eq!(zone(Some("UTC"), Some("/usr/share/zoneinfo/Asia/Tokyo"), None).as_deref(), Some("UTC"));
        assert_eq!(
            zone(Some("EST5EDT,M3.2.0,M11.1.0"), Some("/usr/share/zoneinfo/Asia/Tokyo"), None).as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(zone(Some("/etc/localtime"), None, None), None);
        assert_eq!(zone(Some(""), Some("/etc/other"), Some("")), None);
    }

    fn keys(json: &str) -> Vec<String> {
        // The order of the text, which `Value` would sort.
        let mut keys = Vec::new();
        let mut depth = 0;
        let mut chars = json.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                '"' if depth == 1 => {
                    let end = json[i + 1..].find('"').unwrap() + i + 1;
                    let after = json[end + 1..].chars().next();
                    if after == Some(':') {
                        keys.push(json[i + 1..end].to_owned());
                    }
                    chars.by_ref().take_while(|(j, _)| *j < end).for_each(drop);
                }
                _ => {}
            }
        }
        keys
    }

    fn state() -> State {
        State {
            props: State::base(&["en_US.UTF-8", "C"]),
            focused: true,
            account: None,
            signed_in: true,
            heartbeat: None,
            launching: true,
            assignments: Vec::new(),
            obfuscation: false,
        }
    }

    #[test]
    fn the_web_clients_properties_in_its_order_with_neucords_values() {
        let mut state = state();
        state.refresh(1_000);
        let json = state.props.json();
        assert_eq!(
            keys(&json),
            [
                "os",
                "browser",
                "device",
                "device_model_name",
                "system_locale",
                "has_client_mods",
                "browser_user_agent",
                "browser_version",
                "os_version",
                "referrer",
                "referring_domain",
                "referrer_current",
                "referring_domain_current",
                "release_channel",
                "client_build_number",
                "client_event_source",
                "client_launch_id",
                "launch_signature",
                "client_app_state",
            ]
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["os"], "Linux");
        assert_eq!(value["browser"], NAME);
        assert_eq!(value["device"], "");
        assert_eq!(value["system_locale"], "en-US");
        assert_eq!(value["browser_user_agent"], user_agent());
        assert_eq!(value["browser_version"], VERSION);
        assert_eq!(value["release_channel"], "stable");
        assert_eq!(value["client_build_number"], CLIENT_BUILD_NUMBER);
        assert_eq!(value["client_event_source"], Value::Null);
        assert_eq!(value["launch_signature"], Value::Null, "the key, never a value");
        assert_eq!(value["client_app_state"], "focused");
        // Nothing claims a browser or the desktop app.
        for claim in ["Chrome", "Discord Client", "native_build_number", "window_manager"] {
            assert!(!json.contains(claim), "{claim}");
        }
    }

    #[test]
    fn the_session_id_joins_when_there_is_one_and_leaves_when_it_lapses() {
        let mut state = state();
        state.refresh(0);
        assert_eq!(state.props.get("client_heartbeat_session_id"), None);
        state.heartbeat = renewed(None, true, 10, || "abc".into());
        state.refresh(10);
        let value: Value = serde_json::from_str(&state.props.json()).unwrap();
        assert_eq!(value["client_heartbeat_session_id"], "abc");
        // It lasts half an hour from its last use; then the property is gone.
        state.refresh(10 + HeartbeatSession::LIFETIME_MS - 1);
        assert_eq!(state.props.get("client_heartbeat_session_id"), Some(&json!("abc")));
        state.refresh(10 + HeartbeatSession::LIFETIME_MS);
        let value: Value = serde_json::from_str(&state.props.json()).unwrap();
        assert!(value.get("client_heartbeat_session_id").is_none());
    }

    #[test]
    fn a_key_that_appears_late_goes_last_as_in_the_client() {
        // No session at the first request, so `client_app_state` is
        // assigned first and the session id follows it.
        let mut state = state();
        state.refresh(0);
        state.heartbeat = renewed(None, true, 5, || "abc".into());
        state.refresh(5);
        let json = state.props.json();
        let keys = keys(&json);
        let tail = &keys[keys.len() - 2..];
        assert_eq!(tail, ["client_app_state", "client_heartbeat_session_id"]);
    }

    #[test]
    fn focus_is_named_at_each_request_and_at_identify() {
        let mut state = state();
        state.refresh(0);
        state.focused = false;
        state.refresh(0);
        assert_eq!(state.props.get("client_app_state"), Some(&json!("unfocused")));
    }

    #[test]
    fn the_heartbeat_opens_renews_and_replaces_sessions() {
        let id = || "new".to_string();
        let s = |used: i64| HeartbeatSession { uuid: "old".into(), created_at: 0, last_used: used };
        let half_hour = HeartbeatSession::LIFETIME_MS;
        // In use: none → a new one, used now.
        let made = renewed(None, true, 100, id).unwrap();
        assert_eq!((made.uuid.as_str(), made.created_at, made.last_used), ("new", 100, 100));
        // In use: a live one is the same session, used now.
        let kept = renewed(Some(s(100)), true, 100 + half_hour - 1, id).unwrap();
        assert_eq!((kept.uuid.as_str(), kept.created_at, kept.last_used), ("old", 0, 100 + half_hour - 1));
        // In use, but it lapsed: replaced.
        assert_eq!(renewed(Some(s(100)), true, 100 + half_hour, id).unwrap().uuid, "new");
        // Not in use: kept as it is while live, dropped once lapsed, never made.
        assert_eq!(renewed(Some(s(100)), false, 200, id), Some(s(100)));
        assert_eq!(renewed(Some(s(100)), false, 100 + half_hour, id), None);
        assert_eq!(renewed(None, false, 100, id), None);
    }

    #[test]
    fn the_heartbeat_is_due_at_once_then_every_fifteen_minutes() {
        assert_eq!(beat_delay_ms(None), 0);
        assert_eq!(beat_delay_ms(Some(0)), BEAT_INTERVAL_MS);
        assert_eq!(beat_delay_ms(Some(BEAT_INTERVAL_MS - 1)), 1);
        // Overdue after a long time out of focus.
        assert_eq!(beat_delay_ms(Some(BEAT_INTERVAL_MS * 2)), 0);
    }

    #[test]
    fn the_cache_and_the_slot_round_trip_and_forget_what_they_cannot_read() {
        let cache = Cache { assignments: vec![json!([1, 1, 0, 1])] };
        assert_eq!(Cache::from_json(&cache.to_json()), cache);
        assert!(Cache::from_json("not json").is_empty());
        assert!(Cache::from_json("{}").is_empty());
        let slot = slot_of(1, "u");
        assert_eq!(Slot::from_json(&slot.to_json()), Some(slot));
        assert_eq!(Slot::from_json("not json"), None);
        assert_eq!(Slot::from_json("{}"), None);
    }

    fn slot_of(account: u64, uuid: &str) -> Slot {
        Slot {
            account: Snowflake(account),
            session: HeartbeatSession { uuid: uuid.into(), created_at: 1, last_used: 2 },
        }
    }

    fn assigned() -> Cache {
        Cache { assignments: vec![json!([crate::experiments::murmur3("2026-02-private-channel-hiding"), 1, 0, 1])] }
    }

    fn session() -> Option<String> {
        slot().map(|slot| slot.session.uuid)
    }

    #[test]
    fn a_reconnect_keeps_what_the_account_had_and_another_account_has_its_own() {
        let (alice, bob) = (Snowflake(1), Snowflake(2));
        sign_in(Some(alice), Some(slot_of(1, "alice")), assigned());
        assert!(obfuscation(), "its stored assignments are what IDENTIFY goes by");
        assert_eq!(session().as_deref(), Some("alice"), "a relaunch continues the session");
        // The same account connecting again reads nothing over what it holds.
        sign_in(Some(alice), None, Cache::default());
        assert_eq!(session().as_deref(), Some("alice"));
        assert!(obfuscation());
        // Another account's token starts from that account's own assignments.
        sign_in(Some(bob), None, Cache::default());
        assert_eq!(slot(), None);
        assert!(!obfuscation());
        sign_out();
        assert_eq!(account(), None);
    }

    #[test]
    fn a_first_login_learns_its_account_from_ready_and_what_it_is_assigned() {
        sign_in(None, None, Cache::default());
        assert!(!obfuscation(), "nothing stored on a first launch");
        signed_in_as(Snowflake(7));
        learn_assignments(assigned().assignments);
        assert_eq!((account(), obfuscation()), (Some(Snowflake(7)), true));
        // A reconnect of the same login finds the account known.
        sign_in(Some(Snowflake(7)), None, Cache::default());
        assert!(obfuscation());
        learn_assignments(vec![]);
        assert!(!obfuscation(), "the next IDENTIFY goes by the new assignment");
    }

    #[test]
    fn the_session_is_made_only_while_the_app_is_in_use() {
        sign_in(Some(Snowflake(1)), None, Cache::default());
        set_focused(false);
        heartbeat();
        assert_eq!(slot(), None, "unfocused: nothing is opened");
        set_focused(true);
        heartbeat();
        let uuid = session().expect("opened");
        heartbeat();
        assert_eq!(session(), Some(uuid), "the same session, used again");
    }

    #[test]
    fn identify_adds_what_only_the_gateway_sends() {
        let now = now_ms();
        let live = HeartbeatSession { uuid: "live".into(), created_at: now, last_used: now };
        sign_in(Some(Snowflake(1)), Some(Slot { account: Snowflake(1), session: live }), Cache::default());
        set_focused(false);
        let json = serde_json::to_string(&identify_properties()).unwrap();
        let keys = keys(&json);
        assert_eq!(
            keys[keys.len() - 4..],
            ["client_heartbeat_session_id", "client_app_state", "is_fast_connect", "gateway_connect_reasons"]
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["client_heartbeat_session_id"], "live");
        assert_eq!(value["client_app_state"], "unfocused");
        assert_eq!(
            (&value["is_fast_connect"], &value["gateway_connect_reasons"]),
            (&json!(false), &json!("AppSkeleton"))
        );
        assert_eq!(value["client_build_number"], CLIENT_BUILD_NUMBER);
        // Focus is read again at the next IDENTIFY.
        set_focused(true);
        let value: Value = serde_json::from_str(&serde_json::to_string(&identify_properties()).unwrap()).unwrap();
        assert_eq!(value["client_app_state"], "focused");
    }

    #[test]
    fn going_back_to_an_account_does_not_bring_its_session_back() {
        // The client keeps one session and drops it when the token changes.
        let (alice, bob) = (Snowflake(1), Snowflake(2));
        sign_in(Some(alice), Some(slot_of(1, "alice")), Cache::default());
        sign_in(Some(bob), None, Cache::default());
        sign_in(Some(alice), Some(slot_of(1, "alice")), Cache::default());
        assert_eq!(slot(), None);
    }

    #[test]
    fn only_the_account_a_slot_was_opened_for_continues_it() {
        sign_in(Some(Snowflake(2)), Some(slot_of(1, "alice")), Cache::default());
        assert_eq!(slot(), None, "another account's token opened it");
        sign_out();
        // Nor does a later sign-in of the same run read the file again.
        sign_in(Some(Snowflake(1)), Some(slot_of(1, "alice")), Cache::default());
        assert_eq!(slot(), None);
    }

    #[test]
    fn a_new_token_ends_the_session() {
        sign_in(Some(Snowflake(1)), Some(slot_of(1, "alice")), Cache::default());
        assert!(slot().is_some());
        token_replaced();
        assert_eq!(slot(), None);
        heartbeat();
        assert_ne!(session().as_deref(), Some("alice"), "the next heartbeat opens another");
    }
}
