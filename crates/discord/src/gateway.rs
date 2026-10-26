//! The gateway websocket: identify, heartbeat, and turning dispatches into
//! [`Event`]s. Runs entirely on the GLib main loop via libsoup.
//!
//! Not handled yet: RESUME (a dropped connection is reported as
//! [`Event::Closed`] and the caller reconnects with a fresh IDENTIFY) and
//! zlib transport compression (payloads come as plain JSON text frames).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use soup::prelude::*;

use crate::notifications::{GuildSettings, NotificationSettings};
use crate::read_state::ReadStates;
use crate::{Channel, Error, Guild, Message, Snowflake, User, identity};

/// Gateway capability bits. 0 asks for the classic payload shapes, which the
/// model parses directly; the guild parser also accepts the newer shape.
const CAPABILITIES: u64 = 0;

/// Close code used when we drop a connection ourselves (zombied or told to
/// reconnect). 4000 lets the session be resumed once RESUME exists.
const CLOSE_RECONNECT: u16 = 4000;

/// Everything READY gives us to start from.
#[derive(Debug)]
pub struct Ready {
    pub user: User,
    pub guilds: Vec<Guild>,
    /// DMs and group DMs, in no particular order.
    pub private_channels: Vec<Channel>,
    pub read_states: ReadStates,
    pub notification_settings: NotificationSettings,
}

#[derive(Debug)]
pub enum Event {
    Ready(Box<Ready>),
    GuildCreate(Guild),
    /// A DM or group DM was opened (or reopened). Guild channels are not reported yet.
    PrivateChannelCreate(Channel),
    /// A DM or group DM was closed, or we left the group.
    PrivateChannelDelete(Snowflake),
    MessageCreate(Message),
    /// An edit. `content` is absent for embed-only updates, which we ignore.
    MessageUpdate { id: Snowflake, channel_id: Snowflake, content: Option<String>, edited: bool },
    MessageDelete { id: Snowflake, channel_id: Snowflake },
    /// One of our clients (maybe this one) marked `channel_id` read up to `message_id`.
    MessageAck { channel_id: Snowflake, message_id: Snowflake },
    /// Newest message ids for some of a server's channels, sent after READY
    /// because READY's can be stale.
    ChannelUnreadUpdate { guild_id: Snowflake, channels: Vec<(Snowflake, Snowflake)> },
    /// Our notification settings for one server (or DMs) changed.
    NotificationSettingsUpdate(GuildSettings),
    /// The connection is gone. `code` is the websocket close code.
    Closed { code: u16, reason: String },
}

impl Event {
    /// Close codes after which reconnecting with the same token is pointless.
    pub fn is_fatal_close(code: u16) -> bool {
        // 4004 authentication failed, 4010-4014 invalid shard/version/intents.
        matches!(code, 4004 | 4010..=4014)
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
    /// Our user id, known from READY on; used to find our roles in GUILD_CREATE.
    me: Cell<Option<Snowflake>>,
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
    pub async fn connect(token: &str) -> Result<(Gateway, async_channel::Receiver<Event>), Error> {
        let session = soup::Session::new();
        session.set_user_agent(&identity::user_agent());
        let msg = soup::Message::new("GET", identity::GATEWAY_URL).expect("gateway URL is valid");
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
            seq: Cell::new(None),
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
                let _ = state.events.try_send(Event::Closed {
                    code: conn.close_code(),
                    reason: conn.close_data().map(String::from).unwrap_or_default(),
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
            identify(conn, state);
        }
        0 => dispatch(state, payload.t.as_deref().unwrap_or(""), payload.d),
        // Server asks for an immediate heartbeat.
        1 => heartbeat(conn, state),
        11 => state.acked.set(true),
        // RECONNECT or INVALID SESSION: drop and let the caller reconnect.
        7 | 9 => conn.close(CLOSE_RECONNECT, Some("reconnect requested")),
        _ => {}
    }
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

/// A guild with `my_roles` taken from our own entry in its `members`.
fn guild_with_roles(value: &Value, me: Snowflake) -> Option<Guild> {
    let mut guild = Guild::deserialize(value).ok()?;
    let mine = value["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| parse_id(&m["user"]["id"]) == Some(me) || parse_id(&m["user_id"]) == Some(me));
    if let Some(member) = mine {
        guild.my_roles = member["roles"].as_array().into_iter().flatten().filter_map(parse_id).collect();
    }
    Some(guild)
}

/// Message requests and spam DMs, which Discord keeps out of the DM list.
fn is_hidden_dm(channel: &Value) -> bool {
    channel["is_message_request"].as_bool() == Some(true) || channel["is_spam"].as_bool() == Some(true)
}

/// READY's DMs, minus hidden ones. Recipients come either inline as `recipients` (the classic
/// shape) or as `recipient_ids` pointing into READY's top-level `users`.
fn private_channels(ready: &Value) -> Vec<Channel> {
    let users: std::collections::HashMap<&str, &Value> = ready["users"]
        .as_array()
        .map(|users| users.iter().filter_map(|u| Some((u["id"].as_str()?, u))).collect())
        .unwrap_or_default();
    let Some(raw) = ready["private_channels"].as_array() else { return Vec::new() };
    raw.iter()
        .filter(|value| !is_hidden_dm(value))
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
            state.me.set(Some(user.id));
            let guilds = data["guilds"]
                .as_array()
                .map(|raw| raw.iter().filter_map(|g| guild_with_roles(g, user.id)).collect())
                .unwrap_or_default();
            Event::Ready(Box::new(Ready {
                private_channels: private_channels(&data),
                read_states: ReadStates::from_ready(&data["read_state"]),
                notification_settings: NotificationSettings::from_ready(&data["user_guild_settings"]),
                user,
                guilds,
            }))
        }
        "MESSAGE_ACK" => {
            let (Some(channel_id), Some(message_id)) =
                (parse_id(&data["channel_id"]), parse_id(&data["message_id"]))
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
        "USER_GUILD_SETTINGS_UPDATE" => match GuildSettings::deserialize(&data) {
            Ok(settings) => Event::NotificationSettingsUpdate(settings),
            Err(e) => return eprintln!("gateway: bad USER_GUILD_SETTINGS_UPDATE: {e}"),
        },
        "CHANNEL_CREATE" => match Channel::deserialize(&data) {
            Ok(channel) if channel.kind.is_private() && !is_hidden_dm(&data) => {
                Event::PrivateChannelCreate(channel)
            }
            Ok(_) => return,
            Err(e) => return eprintln!("gateway: bad CHANNEL_CREATE: {e}"),
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
            match guild_with_roles(&data, me) {
                Some(guild) => Event::GuildCreate(guild),
                None => return,
            }
        }
        "MESSAGE_CREATE" => match serde_json::from_value(data) {
            Ok(message) => Event::MessageCreate(message),
            Err(e) => return eprintln!("gateway: bad MESSAGE_CREATE: {e}"),
        },
        // Partial: only id and channel_id are guaranteed; content is present on
        // a text edit, absent on embed-only updates.
        "MESSAGE_UPDATE" => {
            let (Some(id), Some(channel_id)) =
                (parse_id(&data["id"]), parse_id(&data["channel_id"]))
            else {
                return;
            };
            Event::MessageUpdate {
                id,
                channel_id,
                content: data["content"].as_str().map(String::from),
                edited: !data["edited_timestamp"].is_null(),
            }
        }
        "MESSAGE_DELETE" => {
            let (Some(id), Some(channel_id)) =
                (parse_id(&data["id"]), parse_id(&data["channel_id"]))
            else {
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
        assert_eq!(dms.len(), 1);
        assert_eq!(dms[0].title(), "alice");
    }

    #[test]
    fn guild_roles_from_own_member() {
        let raw = json!({"id": "1", "name": "G", "default_message_notifications": 1, "members": [
            {"user": {"id": "8"}, "roles": ["70"]},
            {"user": {"id": "5"}, "roles": ["40", "41"]}
        ]});
        let guild = guild_with_roles(&raw, Snowflake(5)).unwrap();
        assert_eq!(guild.my_roles, vec![Snowflake(40), Snowflake(41)]);
        assert_eq!(guild.default_message_notifications, 1);
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
}
