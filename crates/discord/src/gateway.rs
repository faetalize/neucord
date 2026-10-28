//! The gateway websocket: identify, heartbeat, and turning dispatches into
//! [`Event`]s. Runs entirely on the GLib main loop via libsoup.
//!
//! A dropped connection is reported as [`Event::Closed`], carrying a
//! [`Resume`] when the session can be resumed: the caller passes it back to
//! [`Gateway::connect`], which then sends RESUME instead of IDENTIFY and
//! Discord replays what was missed. Not handled yet: zlib transport
//! compression (payloads come as plain JSON text frames).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use soup::prelude::*;

use crate::embed::Embed;
use crate::notifications::{GuildSettings, NotificationSettings};
use crate::read_state::ReadStates;
use crate::relationships::{Presence, Relationship, Relationships};
use crate::settings::{self, GuildFolder, MediaPatch, MediaSettings};
use crate::verification::{Account, Rules};
use crate::{Channel, Error, Guild, Member, Message, MessageEdit, Snowflake, User, identity};

/// Gateway capability bits. 0 asks for the classic payload shapes, which the
/// model parses directly; the guild parser also accepts the newer shape.
const CAPABILITIES: u64 = 0;

/// Close code used when we drop a connection ourselves (zombied or told to
/// reconnect). 4000 lets the session be resumed once RESUME exists.
const CLOSE_RECONNECT: u16 = 4000;

/// What it takes to resume a session after the connection drops.
#[derive(Clone, Debug)]
pub struct Resume {
    pub session_id: String,
    /// READY's `resume_gateway_url`.
    pub url: String,
    /// The last sequence number we received.
    pub seq: u64,
}

/// Everything READY gives us to start from.
#[derive(Debug)]
pub struct Ready {
    pub user: User,
    /// What servers' verification asks of the same user.
    pub account: Account,
    pub guilds: Vec<Guild>,
    /// DMs and group DMs, message requests included, in no particular order.
    pub private_channels: Vec<Channel>,
    /// Friends, requests and blocks, with friends' presences.
    pub relationships: Relationships,
    pub read_states: ReadStates,
    pub notification_settings: NotificationSettings,
    /// Server folders and order; empty if the user never arranged servers.
    pub guild_folders: Vec<GuildFolder>,
    pub media_settings: MediaSettings,
    /// A step Discord requires before the account can be used, if any
    /// (see [`crate::account`]).
    pub required_action: Option<String>,
}

#[derive(Debug)]
pub enum Event {
    Ready(Box<Ready>),
    GuildCreate(Guild),
    /// A DM or group DM was opened (or reopened), or a message request
    /// arrived. Guild channels are not reported yet.
    PrivateChannelCreate(Channel),
    /// A DM changed, for instance a message request was accepted.
    PrivateChannelUpdate(Channel),
    /// A DM or group DM was closed, or we left the group.
    PrivateChannelDelete(Snowflake),
    /// A server channel changed: its name, topic or slowmode, say.
    GuildChannelUpdate {
        guild_id: Snowflake,
        channel: Channel,
    },
    /// Our own member in a server changed: roles, a timeout, screening passed.
    MemberUpdate {
        guild_id: Snowflake,
        member: Member,
    },
    /// A server's settings changed; only what verification reads is kept.
    GuildUpdate {
        guild_id: Snowflake,
        rules: Rules,
    },
    /// Our account changed, for instance its email was verified.
    AccountUpdate(Account),
    MessageCreate(Box<Message>),
    /// An edit, or link previews arriving for a message.
    MessageUpdate(MessageEdit),
    MessageDelete {
        id: Snowflake,
        channel_id: Snowflake,
    },
    /// One of our clients (maybe this one) marked `channel_id` read up to `message_id`.
    MessageAck {
        channel_id: Snowflake,
        message_id: Snowflake,
    },
    /// Newest message ids for some of a server's channels, sent after READY
    /// because READY's can be stale.
    ChannelUnreadUpdate {
        guild_id: Snowflake,
        channels: Vec<(Snowflake, Snowflake)>,
    },
    /// A new relationship, or one whose type changed (a request accepted).
    /// `notify` is Discord's hint that this deserves a notification.
    RelationshipAdd {
        relationship: Relationship,
        notify: bool,
    },
    /// We changed a friend's nickname.
    RelationshipUpdate {
        user_id: Snowflake,
        nickname: Option<String>,
    },
    /// Unfriended, unblocked, or a request withdrawn or ignored.
    RelationshipRemove(Snowflake),
    PresenceUpdate {
        user_id: Snowflake,
        presence: Presence,
    },
    /// Our notification settings for one server (or DMs) changed.
    NotificationSettingsUpdate(GuildSettings),
    /// The user rearranged servers or folders (on any client).
    GuildFoldersUpdate(Vec<GuildFolder>),
    /// The user changed how embeds and media show.
    MediaSettingsUpdate(MediaPatch),
    /// Discord started or stopped requiring a step (see [`crate::account`]).
    RequiredActionUpdate(Option<String>),
    /// A resume succeeded; everything missed has been replayed before this.
    Resumed,
    /// The connection is gone. `code` is the websocket close code. `resume`
    /// is set when the session survives and can be resumed.
    Closed {
        code: u16,
        reason: String,
        resume: Option<Resume>,
    },
}

impl Event {
    /// Close codes after which reconnecting with the same token is pointless.
    pub fn is_fatal_close(code: u16) -> bool {
        // 4004 authentication failed, 4010-4014 invalid shard/version/intents.
        matches!(code, 4004 | 4010..=4014)
    }

    /// Close codes after which the session is gone and only IDENTIFY works:
    /// 4007 invalid sequence, 4009 session timed out, plus the fatal ones.
    fn ends_session(code: u16) -> bool {
        matches!(code, 4007 | 4009) || Self::is_fatal_close(code)
    }
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    #[serde(default)]
    d: Value,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

struct State {
    token: String,
    /// Our user id, known from READY on; used to find our own member.
    me: Cell<Option<Snowflake>>,
    /// Session id and resume URL from READY (or the resume we were given).
    session: RefCell<Option<(String, String)>>,
    /// Send RESUME instead of IDENTIFY on HELLO.
    resuming: Cell<bool>,
    /// Cleared by INVALID SESSION (not resumable).
    resumable: Cell<bool>,
    seq: Cell<Option<u64>>,
    acked: Cell<bool>,
    events: async_channel::Sender<Event>,
}

/// A live gateway connection. Dropping it closes the connection.
pub struct Gateway {
    conn: soup::WebsocketConnection,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        if self.conn.state() == soup::WebsocketState::Open {
            self.conn.close(1000, None);
        }
    }
}

impl Gateway {
    /// Connect and IDENTIFY, or with `resume` RESUME a dropped session.
    pub async fn connect(
        token: &str,
        resume: Option<Resume>,
    ) -> Result<(Gateway, async_channel::Receiver<Event>), Error> {
        let session = soup::Session::new();
        session.set_user_agent(&identity::user_agent());
        let url = match &resume {
            Some(r) => format!("{}/?{}", r.url.trim_end_matches('/'), identity::GATEWAY_QUERY),
            None => identity::GATEWAY_URL.to_owned(),
        };
        let Ok(msg) = soup::Message::new("GET", &url) else {
            // A malformed resume URL from Discord: nothing to resume against.
            return Err(Error::Http { status: 0, body: format!("bad gateway URL {url}") });
        };
        let conn = session
            .websocket_connect_async_future(&msg, None, &[], glib::Priority::DEFAULT)
            .await
            .map_err(Error::Transport)?;
        // READY for a user account easily exceeds libsoup's 128 KiB default.
        conn.set_max_incoming_payload_size(0);

        let (tx, rx) = async_channel::unbounded();
        let state = Rc::new(State {
            token: token.to_owned(),
            me: Cell::new(None),
            resuming: Cell::new(resume.is_some()),
            resumable: Cell::new(true),
            seq: Cell::new(resume.as_ref().map(|r| r.seq)),
            session: RefCell::new(resume.map(|r| (r.session_id, r.url))),
            acked: Cell::new(true),
            events: tx,
        });

        conn.connect_message({
            let state = state.clone();
            move |conn, kind, bytes| {
                if kind != soup::ffi::SOUP_WEBSOCKET_DATA_TEXT {
                    return;
                }
                match serde_json::from_slice::<Payload>(bytes) {
                    Ok(payload) => handle(conn, &state, payload),
                    Err(e) => eprintln!("gateway: unparseable payload: {e}"),
                }
            }
        });
        conn.connect_closed({
            let state = state.clone();
            move |conn| {
                let code = conn.close_code();
                let resume = match (&*state.session.borrow(), state.seq.get()) {
                    (Some((session_id, url)), Some(seq)) if state.resumable.get() && !Event::ends_session(code) => {
                        Some(Resume { session_id: session_id.clone(), url: url.clone(), seq })
                    }
                    _ => None,
                };
                let _ = state.events.try_send(Event::Closed {
                    code,
                    reason: conn.close_data().map(String::from).unwrap_or_default(),
                    resume,
                });
                state.events.close();
            }
        });

        Ok((Gateway { conn }, rx))
    }
}

fn send(conn: &soup::WebsocketConnection, value: Value) {
    if conn.state() == soup::WebsocketState::Open {
        conn.send_text(&value.to_string());
    }
}

fn heartbeat(conn: &soup::WebsocketConnection, state: &State) {
    state.acked.set(false);
    send(conn, json!({ "op": 1, "d": state.seq.get() }));
}

fn resume(conn: &soup::WebsocketConnection, state: &State) {
    let session_id = state.session.borrow().as_ref().map(|(id, _)| id.clone());
    send(
        conn,
        json!({
            "op": 6,
            "d": { "token": state.token, "session_id": session_id, "seq": state.seq.get() }
        }),
    );
}

fn identify(conn: &soup::WebsocketConnection, state: &State) {
    send(
        conn,
        json!({
            "op": 2,
            "d": {
                "token": state.token,
                "capabilities": CAPABILITIES,
                "properties": identity::client_properties(),
                "presence": { "status": "unknown", "since": 0, "activities": [], "afk": false },
                "compress": false,
                "client_state": { "guild_versions": {} },
            }
        }),
    );
}

fn handle(conn: &soup::WebsocketConnection, state: &Rc<State>, payload: Payload) {
    if let Some(seq) = payload.s {
        state.seq.set(Some(seq));
    }
    match payload.op {
        // HELLO: start heartbeating, then identify.
        10 => {
            let interval = payload.d["heartbeat_interval"].as_u64().unwrap_or(41_250);
            spawn_heartbeat(conn, state, Duration::from_millis(interval));
            if state.resuming.get() {
                resume(conn, state);
            } else {
                identify(conn, state);
            }
        }
        0 => {
            let name = payload.t.as_deref().unwrap_or("");
            if matches!(name, "READY" | "RESUMED") {
                debug_drop(conn, name);
            }
            dispatch(state, name, payload.d)
        }
        // Server asks for an immediate heartbeat.
        1 => heartbeat(conn, state),
        11 => state.acked.set(true),
        // RECONNECT: drop and let the caller resume.
        7 => conn.close(CLOSE_RECONNECT, Some("reconnect requested")),
        // INVALID SESSION: `d` says whether resuming can still work. If not,
        // the caller has to IDENTIFY again.
        9 => {
            if payload.d.as_bool() != Some(true) {
                state.resumable.set(false);
            }
            conn.close(CLOSE_RECONNECT, Some("session invalidated"));
        }
        _ => {}
    }
}

/// Debug aid for testing resume: with `NEUCORD_DEBUG_DROP=<seconds>`, drop
/// each connection that long after it is ready, as a network failure would.
fn debug_drop(conn: &soup::WebsocketConnection, ready: &str) {
    let Some(secs) = std::env::var("NEUCORD_DEBUG_DROP").ok().and_then(|v| v.parse::<u64>().ok()) else { return };
    eprintln!("gateway: {ready}; dropping the connection in {secs}s (NEUCORD_DEBUG_DROP)");
    let conn = conn.downgrade();
    glib::timeout_add_local_once(Duration::from_secs(secs), move || {
        if let Some(conn) = conn.upgrade() {
            conn.close(CLOSE_RECONNECT, Some("debug drop"));
        }
    });
}

fn spawn_heartbeat(conn: &soup::WebsocketConnection, state: &Rc<State>, interval: Duration) {
    let conn = conn.downgrade();
    let state = state.clone();
    glib::spawn_future_local(async move {
        // Discord asks for a random first delay so clients don't beat in sync.
        let jitter = glib::random_double_range(0.0, 1.0);
        glib::timeout_future(interval.mul_f64(jitter)).await;
        loop {
            let Some(conn) = conn.upgrade() else { break };
            if conn.state() != soup::WebsocketState::Open {
                break;
            }
            if !state.acked.get() {
                // No ACK since the last beat: the connection is a zombie.
                conn.close(CLOSE_RECONNECT, Some("heartbeat not acknowledged"));
                break;
            }
            heartbeat(&conn, &state);
            drop(conn);
            glib::timeout_future(interval).await;
        }
    });
}

/// Discord ids are JSON strings; pull one out of a `Value`.
fn parse_id(value: &Value) -> Option<Snowflake> {
    value.as_str()?.parse().ok().map(Snowflake)
}

/// Whether a member object is ours; it names its user either inline or by id.
fn is_me(member: &Value, me: Snowflake) -> bool {
    parse_id(&member["user"]["id"]) == Some(me) || parse_id(&member["user_id"]) == Some(me)
}

/// A guild with our own member applied, found in its `members` or, in
/// READY's newer shape, in `merged`: its entry of `merged_members`.
fn guild_with_member(value: &Value, merged: &Value, me: Snowflake) -> Option<Guild> {
    let mut guild = Guild::deserialize(value).ok()?;
    let members = value["members"].as_array().into_iter().chain(merged.as_array()).flatten();
    if let Some(member) = members.into_iter().find(|m| is_me(m, me)) {
        guild.set_member(Member::from_value(member));
    }
    Some(guild)
}

/// READY's DMs, message requests included. Recipients come either inline as `recipients` (the classic
/// shape) or as `recipient_ids` pointing into READY's top-level `users`.
fn private_channels(ready: &Value) -> Vec<Channel> {
    let users: std::collections::HashMap<&str, &Value> = ready["users"]
        .as_array()
        .map(|users| users.iter().filter_map(|u| Some((u["id"].as_str()?, u))).collect())
        .unwrap_or_default();
    let Some(raw) = ready["private_channels"].as_array() else { return Vec::new() };
    raw.iter()
        .filter_map(|value| {
            let mut channel = Channel::deserialize(value).ok()?;
            if channel.recipients.is_empty()
                && let Some(ids) = value["recipient_ids"].as_array()
            {
                channel.recipients = ids
                    .iter()
                    .filter_map(|id| users.get(id.as_str()?))
                    .filter_map(|user| User::deserialize(*user).ok())
                    .collect();
            }
            channel.kind.is_private().then_some(channel)
        })
        .collect()
}

fn dispatch(state: &State, name: &str, data: Value) {
    if crate::shape::enabled() {
        eprintln!("gateway {name}: {}", crate::shape::describe(&data));
    }
    let event = match name {
        "READY" => {
            let user: User = match serde_json::from_value(data["user"].clone()) {
                Ok(user) => user,
                Err(e) => return eprintln!("gateway: READY without a usable user: {e}"),
            };
            let Some(account) = Account::from_user(&data["user"]) else { return };
            state.me.set(Some(user.id));
            state.resuming.set(false);
            if let (Some(id), Some(url)) = (data["session_id"].as_str(), data["resume_gateway_url"].as_str()) {
                *state.session.borrow_mut() = Some((id.to_owned(), url.to_owned()));
            }
            let guilds = data["guilds"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(i, g)| guild_with_member(g, &data["merged_members"][i], user.id))
                .collect();
            Event::Ready(Box::new(Ready {
                private_channels: private_channels(&data),
                relationships: Relationships::from_ready(&data),
                read_states: ReadStates::from_ready(&data["read_state"]),
                notification_settings: NotificationSettings::from_ready(&data["user_guild_settings"]),
                guild_folders: settings::guild_folders(&data["user_settings"]).unwrap_or_default(),
                media_settings: {
                    let mut media = MediaSettings::default();
                    media.apply(MediaPatch::from_settings(&data["user_settings"]));
                    media
                },
                required_action: data["required_action"].as_str().map(str::to_owned),
                account,
                user,
                guilds,
            }))
        }
        "RESUMED" => Event::Resumed,
        "USER_REQUIRED_ACTION_UPDATE" => {
            Event::RequiredActionUpdate(data["required_action"].as_str().map(str::to_owned))
        }
        "MESSAGE_ACK" => {
            let (Some(channel_id), Some(message_id)) = (parse_id(&data["channel_id"]), parse_id(&data["message_id"]))
            else {
                return;
            };
            Event::MessageAck { channel_id, message_id }
        }
        "CHANNEL_UNREAD_UPDATE" => {
            let Some(guild_id) = parse_id(&data["guild_id"]) else { return };
            let channels = data["channel_unread_updates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| Some((parse_id(&c["id"])?, parse_id(&c["last_message_id"])?)))
                .collect();
            Event::ChannelUnreadUpdate { guild_id, channels }
        }
        // Partial: only the settings that changed.
        "USER_SETTINGS_UPDATE" => {
            let media = MediaPatch::from_settings(&data);
            if !media.is_empty() {
                let _ = state.events.try_send(Event::MediaSettingsUpdate(media));
            }
            match settings::guild_folders(&data) {
                Some(folders) => Event::GuildFoldersUpdate(folders),
                None => return,
            }
        }
        "USER_GUILD_SETTINGS_UPDATE" => match GuildSettings::deserialize(&data) {
            Ok(settings) => Event::NotificationSettingsUpdate(settings),
            Err(e) => return eprintln!("gateway: bad USER_GUILD_SETTINGS_UPDATE: {e}"),
        },
        "CHANNEL_CREATE" => match Channel::deserialize(&data) {
            Ok(channel) if channel.kind.is_private() => Event::PrivateChannelCreate(channel),
            Ok(_) => return,
            Err(e) => return eprintln!("gateway: bad CHANNEL_CREATE: {e}"),
        },
        "CHANNEL_UPDATE" => match (Channel::deserialize(&data), parse_id(&data["guild_id"])) {
            (Ok(channel), _) if channel.kind.is_private() => Event::PrivateChannelUpdate(channel),
            (Ok(channel), Some(guild_id)) => Event::GuildChannelUpdate { guild_id, channel },
            (Ok(_), None) => return,
            (Err(e), _) => return eprintln!("gateway: bad CHANNEL_UPDATE: {e}"),
        },
        // Discord may send other members' too; only ours matters.
        "GUILD_MEMBER_UPDATE" => {
            let Some(guild_id) = parse_id(&data["guild_id"]) else { return };
            if !state.me.get().is_some_and(|me| is_me(&data, me)) {
                return;
            }
            Event::MemberUpdate { guild_id, member: Member::from_value(&data) }
        }
        "GUILD_UPDATE" => match parse_id(&data["id"]) {
            Some(guild_id) => Event::GuildUpdate { guild_id, rules: Rules::from_guild(&data) },
            None => return,
        },
        "USER_UPDATE" => match Account::from_user(&data) {
            Some(account) if Some(account.id) == state.me.get() => Event::AccountUpdate(account),
            _ => return,
        },
        "RELATIONSHIP_ADD" => match Relationship::from_value(&data, &Default::default()) {
            Some(relationship) => {
                Event::RelationshipAdd { relationship, notify: data["should_notify"].as_bool() == Some(true) }
            }
            None => return eprintln!("gateway: bad RELATIONSHIP_ADD"),
        },
        // Partial: no user, and never a type change (those come as RELATIONSHIP_ADD).
        "RELATIONSHIP_UPDATE" => {
            let Some(user_id) = parse_id(&data["id"]) else { return };
            let nickname = data["nickname"].as_str().filter(|n| !n.is_empty()).map(String::from);
            Event::RelationshipUpdate { user_id, nickname }
        }
        "RELATIONSHIP_REMOVE" => match parse_id(&data["id"]) {
            Some(id) => Event::RelationshipRemove(id),
            None => return,
        },
        // Friends' presences, and per-server ones; only `user.id` is certain.
        "PRESENCE_UPDATE" => match parse_id(&data["user"]["id"]) {
            Some(user_id) => Event::PresenceUpdate { user_id, presence: Presence::from_value(&data) },
            None => return,
        },
        "CHANNEL_DELETE" => {
            let is_private = data["guild_id"].is_null();
            match parse_id(&data["id"]) {
                Some(id) if is_private => Event::PrivateChannelDelete(id),
                _ => return,
            }
        }
        "GUILD_CREATE" => {
            let Some(me) = state.me.get() else { return };
            match guild_with_member(&data, &Value::Null, me) {
                Some(guild) => Event::GuildCreate(guild),
                None => return,
            }
        }
        "MESSAGE_CREATE" => match serde_json::from_value(data) {
            Ok(message) => Event::MessageCreate(Box::new(message)),
            Err(e) => return eprintln!("gateway: bad MESSAGE_CREATE: {e}"),
        },
        // Partial: only id and channel_id are guaranteed.
        "MESSAGE_UPDATE" => {
            let (Some(id), Some(channel_id)) = (parse_id(&data["id"]), parse_id(&data["channel_id"])) else {
                return;
            };
            Event::MessageUpdate(MessageEdit {
                id,
                channel_id,
                content: data["content"].as_str().map(String::from),
                edited: !data["edited_timestamp"].is_null(),
                embeds: data.get("embeds").and_then(|e| Vec::<Embed>::deserialize(e).ok()),
                flags: data["flags"].as_u64(),
            })
        }
        "MESSAGE_DELETE" => {
            let (Some(id), Some(channel_id)) = (parse_id(&data["id"]), parse_id(&data["channel_id"])) else {
                return;
            };
            Event::MessageDelete { id, channel_id }
        }
        _ => return,
    };
    let _ = state.events.try_send(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_dms_inline_recipients() {
        let ready = json!({"private_channels": [
            {"id": "1", "type": 1, "recipients": [{"id": "5", "username": "alice"}]},
            {"id": "2", "type": 0},
            {"id": "4", "type": 1, "is_message_request": true, "recipients": []},
            {"id": "5", "type": 1, "is_spam": true, "recipients": []}
        ]});
        let dms = private_channels(&ready);
        assert_eq!(dms.len(), 3, "message requests are kept, guild channels are not");
        assert_eq!(dms[0].title(), "alice");
        assert!(!dms[0].is_request());
        assert!(dms[1].message_request && dms[2].spam);
    }

    #[test]
    fn guild_roles_from_own_member() {
        let raw = json!({"id": "1", "name": "G", "default_message_notifications": 1, "members": [
            {"user": {"id": "8"}, "roles": ["70"], "communication_disabled_until": "2026-10-28T12:00:00+00:00"},
            {"user": {"id": "5"}, "roles": ["40", "41"], "communication_disabled_until": null}
        ]});
        let guild = guild_with_member(&raw, &Value::Null, Snowflake(5)).unwrap();
        assert_eq!(guild.my_roles, vec![Snowflake(40), Snowflake(41)]);
        assert_eq!(guild.timed_out_until, None, "someone else's timeout");
        assert_eq!(guild.default_message_notifications, 1);
    }

    #[test]
    fn own_member_and_channel_updates() {
        let ready = json!({
            "user": {"id": "5", "username": "me"},
            "guilds": [
                {"id": "1", "properties": {"name": "A"}},
                {"id": "2", "properties": {"name": "B"}}
            ],
            "merged_members": [
                [{"user_id": "5", "roles": ["40"], "communication_disabled_until": "2026-10-28T12:00:00.250000+00:00"}],
                [{"user_id": "5", "roles": []}]
            ]
        });
        let (tx, rx) = async_channel::unbounded();
        let state = State {
            token: String::new(),
            me: Cell::new(None),
            session: RefCell::new(None),
            resuming: Cell::new(false),
            resumable: Cell::new(true),
            seq: Cell::new(None),
            acked: Cell::new(true),
            events: tx,
        };
        dispatch(&state, "READY", ready);
        let Ok(Event::Ready(ready)) = rx.try_recv() else { panic!("no READY") };
        let noon = glib::DateTime::from_iso8601("2026-10-28T12:00:00Z", None).unwrap().to_unix();
        let a = &ready.guilds[0];
        assert_eq!((a.my_roles.as_slice(), a.timed_out_until), (&[Snowflake(40)][..], Some(noon + 1)));
        assert_eq!(a.timeout_left(noon), Some(1), "a part second rounds up, never down");
        assert_eq!(a.timeout_left(noon + 1), None);
        assert_eq!(ready.guilds[1].timed_out_until, None);

        let update = |user: &str, until: Value| {
            dispatch(
                &state,
                "GUILD_MEMBER_UPDATE",
                json!({"guild_id": "1", "user": {"id": user}, "roles": ["40"], "communication_disabled_until": until}),
            );
            rx.try_recv().ok()
        };
        assert!(update("8", json!("2026-10-28T12:00:00+00:00")).is_none(), "not ours");
        let Some(Event::MemberUpdate { guild_id, member }) = update("5", json!("2026-10-28T12:00:00+00:00")) else {
            panic!("our member")
        };
        assert_eq!((guild_id, member.timed_out_until), (Snowflake(1), Some(noon)));
        let Some(Event::MemberUpdate { member, .. }) = update("5", Value::Null) else { panic!("our member") };
        assert_eq!(member.timed_out_until, None, "the timeout lifted early");

        dispatch(&state, "GUILD_UPDATE", json!({"id": "1", "name": "A", "verification_level": 3}));
        let Ok(Event::GuildUpdate { guild_id, rules }) = rx.try_recv() else { panic!("no guild update") };
        assert_eq!((guild_id, rules.level), (Snowflake(1), crate::verification::VerificationLevel::High));
        dispatch(&state, "USER_UPDATE", json!({"id": "8", "verified": true}));
        assert!(rx.try_recv().is_err(), "someone else");
        dispatch(&state, "USER_UPDATE", json!({"id": "5", "verified": true, "email": "a@example.org"}));
        let Ok(Event::AccountUpdate(account)) = rx.try_recv() else { panic!("no account update") };
        assert!(account.verified && account.email && !account.phone);

        dispatch(&state, "CHANNEL_UPDATE", json!({"id": "9", "type": 0, "guild_id": "1", "rate_limit_per_user": 30}));
        let Ok(Event::GuildChannelUpdate { guild_id, channel }) = rx.try_recv() else { panic!("no channel update") };
        assert_eq!((guild_id, channel.rate_limit_per_user), (Snowflake(1), 30));
    }

    #[test]
    fn ready_dms_recipient_ids() {
        let ready = json!({
            "users": [{"id": "5", "username": "a"}, {"id": "6", "username": "b"}],
            "private_channels": [{"id": "3", "type": 3, "recipient_ids": ["5", "6", "7"]}]
        });
        let dms = private_channels(&ready);
        assert_eq!(dms[0].title(), "a, b");
    }

    #[test]
    fn ready_system_dm() {
        let ready = json!({
            "users": [{"id": "5", "username": "discord", "system": true}, {"id": "6", "username": "b", "system": null}],
            "private_channels": [
                {"id": "1", "type": 1, "recipient_ids": ["5"]},
                {"id": "2", "type": 1, "recipient_ids": ["6"]},
                {"id": "3", "type": 3, "recipient_ids": ["5", "6"]}
            ]
        });
        let system: Vec<bool> = private_channels(&ready).iter().map(Channel::is_system_dm).collect();
        assert_eq!(system, [true, false, false]);
    }
}
