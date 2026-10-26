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

use crate::{Error, Guild, Message, Snowflake, User, identity};

/// Gateway capability bits. 0 asks for the classic payload shapes, which the
/// model parses directly; the guild parser also accepts the newer shape.
const CAPABILITIES: u64 = 0;

/// Close code used when we drop a connection ourselves (zombied or told to
/// reconnect). 4000 lets the session be resumed once RESUME exists.
const CLOSE_RECONNECT: u16 = 4000;

#[derive(Debug)]
pub enum Event {
    Ready { user: User, guilds: Vec<Guild> },
    GuildCreate(Guild),
    MessageCreate(Message),
    /// An edit. `content` is absent for embed-only updates, which we ignore.
    MessageUpdate { id: Snowflake, channel_id: Snowflake, content: Option<String>, edited: bool },
    MessageDelete { id: Snowflake, channel_id: Snowflake },
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

fn dispatch(state: &State, name: &str, data: Value) {
    if crate::shape::enabled() {
        eprintln!("gateway {name}: {}", crate::shape::describe(&data));
    }
    let event = match name {
        "READY" => {
            let user = match serde_json::from_value(data["user"].clone()) {
                Ok(user) => user,
                Err(e) => return eprintln!("gateway: READY without a usable user: {e}"),
            };
            let guilds = data["guilds"]
                .as_array()
                .map(|raw| raw.iter().filter_map(|g| Guild::deserialize(g).ok()).collect())
                .unwrap_or_default();
            Event::Ready { user, guilds }
        }
        "GUILD_CREATE" => match Guild::deserialize(&data) {
            Ok(guild) => Event::GuildCreate(guild),
            Err(_) => return,
        },
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
