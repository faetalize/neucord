//! The gateway websocket: identify, heartbeat, and turning dispatches into
//! [`Event`]s. Runs entirely on the GLib main loop via libsoup.
//!
//! A dropped connection is reported as [`Event::Closed`], carrying a
//! [`Resume`] when the session can be resumed: the caller passes it back to
//! [`Gateway::connect`], which then sends RESUME instead of IDENTIFY and
//! Discord replays what was missed.
//!
//! Payloads come compressed, as for the official client: see [`Inflater`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use soup::prelude::*;

use crate::component::Component;
use crate::embed::Embed;
use crate::emoji::GuildEmoji;
use crate::experiments::Experiments;
use crate::gates::Gates;
use crate::inflate::Inflater;
use crate::limits::Nitro;
use crate::member_list::Update as MemberListUpdate;
use crate::members::Members;
use crate::model::Emoji;
use crate::notifications::{GuildSettings, NotificationSettings};
use crate::reactions::{self, Kind};
use crate::read_state::ReadStates;
use crate::relationships::{Presence, Relationship, Relationships};
use crate::settings::{Settings, SettingsPatch, UserStatus};
use crate::settings_proto;
use crate::stickers::Listed;
use crate::verification::Account;
use crate::{Channel, Error, Guild, Member, Message, MessageEdit, Role, Snowflake, ThreadMember, User, identity};

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
    /// What servers ask of the same user.
    pub account: Account,
    pub nitro: Nitro,
    pub guilds: Vec<Guild>,
    /// The other members READY carries.
    pub members: Members,
    /// DMs and group DMs, message requests included, in no particular order.
    pub private_channels: Vec<Channel>,
    /// Friends, requests and blocks, with friends' presences.
    pub relationships: Relationships,
    pub read_states: ReadStates,
    pub notification_settings: NotificationSettings,
    pub settings: Settings,
    /// The `PreloadedUserSettings` they were read from, whole: what changing
    /// a setting starts from (see [`crate::settings_sync`]).
    pub settings_proto: Option<Vec<u8>>,
    /// See [`crate::regional::media_takedown_regulated`].
    pub media_takedown_regulated: bool,
    /// A step Discord requires before the account can be used, if any
    /// (see [`crate::account`]).
    pub required_action: Option<String>,
    /// What Discord rolls out to the account and its servers.
    pub experiments: Experiments,
    /// This session's id, which interactions name.
    pub session_id: String,
    /// What holds privacy settings back, see [`crate::gates`].
    pub gates: Gates,
    /// How many of our servers READY could not give (an outage): the
    /// restrictions of servers we are no longer in are kept while any is.
    pub unavailable_guilds: usize,
}

/// How an interaction of ours ended, as the events `INTERACTION_SUCCESS`,
/// `INTERACTION_FAILURE`, and `INTERACTION_MODAL_CREATE` and
/// `INTERACTION_IFRAME_MODAL_CREATE` (a form to fill in) say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Form,
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
    /// A thread was created or changed: renamed, archived, locked, reopened.
    /// It may leave out our membership (see [`Guild::set_thread`]).
    ThreadUpdate {
        guild_id: Snowflake,
        thread: Channel,
    },
    /// A thread was created, which Discord says is new just now or one we
    /// hadn't been told of.
    ThreadCreate {
        guild_id: Snowflake,
        thread: Channel,
        newly_created: bool,
    },
    ThreadDelete {
        guild_id: Snowflake,
        id: Snowflake,
    },
    /// A server's emoji changed: all of them, as they are now.
    GuildEmojisUpdate {
        guild_id: Snowflake,
        emojis: Vec<GuildEmoji>,
    },
    /// A server's stickers changed: all of them, as they are now.
    GuildStickersUpdate {
        guild_id: Snowflake,
        stickers: Vec<Listed>,
    },
    /// Active threads in a server, and our membership of those we've joined.
    ThreadListSync {
        guild_id: Snowflake,
        threads: Vec<Channel>,
        members: Vec<(Snowflake, ThreadMember)>,
        /// Every active thread of the server, rather than those of some
        /// channels (`channel_ids`).
        complete: bool,
    },
    /// How many messages of a forum's posts we haven't read, which asking
    /// for them (op 28, see [`Gateway::request_forum_unreads`]) brings.
    ForumUnreads {
        channel_id: Snowflake,
        threads: Vec<(Snowflake, Option<u32>)>,
    },
    /// We joined a thread (`Some`), changed how it notifies, or left it.
    ThreadMember {
        guild_id: Snowflake,
        thread: Snowflake,
        member: Option<ThreadMember>,
    },
    /// Our own member in a server changed: roles, a timeout, screening passed.
    MemberUpdate {
        guild_id: Snowflake,
        member: Member,
    },
    /// Other people's members, with their roles: those a server arrives
    /// with, one that changed, or those we asked for. `not_found` are users
    /// we asked for who aren't members.
    Members {
        guild_id: Snowflake,
        members: Vec<(Snowflake, Vec<Snowflake>)>,
        not_found: Vec<Snowflake>,
    },
    /// A server's settings changed, its roles among them. It has no channels
    /// and no member of ours: see [`Guild::update`].
    GuildUpdate(Guild),
    /// A role was created or changed.
    RoleUpdate {
        guild_id: Snowflake,
        role: Role,
    },
    RoleDelete {
        guild_id: Snowflake,
        role_id: Snowflake,
    },
    /// Our account changed, for instance its email was verified, 2FA
    /// turned on or Nitro ran out.
    AccountUpdate(Account, Nitro),
    MessageCreate(Box<Message>),
    /// An edit, or link previews arriving for a message.
    MessageUpdate(MessageEdit),
    MessageDelete {
        id: Snowflake,
        channel_id: Snowflake,
    },
    /// A purge or an automod sweep.
    MessageDeleteBulk {
        ids: Vec<Snowflake>,
        channel_id: Snowflake,
    },
    /// Discord answered the interaction we sent as `nonce`.
    Interaction {
        nonce: Snowflake,
        outcome: Outcome,
    },
    /// Votes on a poll's answers, as `(answer_id, user_id)`: added, or taken back.
    /// `many` is a `MESSAGE_POLL_VOTE_ADD_MANY`, which the client counts
    /// without its DM rule.
    PollVote {
        channel_id: Snowflake,
        message_id: Snowflake,
        votes: Vec<(u32, Snowflake)>,
        added: bool,
        many: bool,
    },
    /// Someone reacted, or took a reaction back.
    Reaction(reactions::Reacted),
    /// Reactions from several people at once, as `(emoji, who)`.
    ReactionsAdded {
        channel_id: Snowflake,
        message_id: Snowflake,
        reactions: Vec<(Emoji, Vec<Snowflake>)>,
    },
    /// Every reaction was removed from a message.
    ReactionsCleared {
        channel_id: Snowflake,
        message_id: Snowflake,
    },
    /// Every reaction with one emoji was removed from a message.
    EmojiReactionsCleared {
        channel_id: Snowflake,
        message_id: Snowflake,
        emoji: Emoji,
    },
    /// Someone started typing in a channel. In a server it carries their
    /// member, which names them.
    TypingStart {
        channel_id: Snowflake,
        user_id: Snowflake,
        user: Option<User>,
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
    /// A note of ours on someone changed, from any of our sessions (see
    /// [`crate::notes`]); none for one that was emptied.
    UserNoteUpdate {
        user_id: Snowflake,
        note: Option<String>,
    },
    /// A presence changed. With a server, it came with our subscription to
    /// it (see [`crate::subscriptions`]).
    PresenceUpdate {
        user_id: Snowflake,
        guild_id: Option<Snowflake>,
        presence: Presence,
    },
    /// Rows of a server's member list (see [`crate::member_list`]).
    MemberList(Box<MemberListUpdate>),
    /// Our notification settings for one server (or DMs) changed.
    NotificationSettingsUpdate(GuildSettings),
    /// The user changed settings (on any client). With `replace`, the patch
    /// is all of them and what it leaves out is back to its default. From
    /// the protobuf form, `proto` is what arrived.
    SettingsUpdate {
        patch: SettingsPatch,
        replace: bool,
        proto: Option<Vec<u8>>,
    },
    /// The favourites and counts of use changed (the settings proto of type
    /// 2, see [`crate::frecency`]): all of them, or with `partial` the
    /// messages that changed in them.
    FrecencyUpdate {
        proto: Vec<u8>,
        partial: bool,
    },
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
    /// READY or RESUMED arrived: Discord takes requests now.
    established: Cell<bool>,
    seq: Cell<Option<u64>>,
    acked: Cell<bool>,
    events: async_channel::Sender<Event>,
}

/// A live gateway connection. Dropping it closes the connection.
pub struct Gateway {
    conn: soup::WebsocketConnection,
    state: Rc<State>,
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
            established: Cell::new(false),
            seq: Cell::new(resume.as_ref().map(|r| r.seq)),
            session: RefCell::new(resume.map(|r| (r.session_id, r.url))),
            acked: Cell::new(true),
            events: tx,
        });

        conn.connect_message({
            let state = state.clone();
            let inflater = RefCell::new(Inflater::new());
            move |conn, kind, bytes| {
                if kind != soup::ffi::SOUP_WEBSOCKET_DATA_BINARY {
                    return;
                }
                let text = match inflater.borrow_mut().push(bytes) {
                    Ok(Some(text)) => text,
                    Ok(None) => return,
                    Err(e) => {
                        // Nothing after this can be read: start over on a
                        // new connection, resuming, as the client does.
                        eprintln!("gateway: cannot inflate: {e}");
                        conn.close(CLOSE_RECONNECT, Some("decompression error"));
                        return;
                    }
                };
                match serde_json::from_slice::<Payload>(&text) {
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

        Ok((Gateway { conn, state }, rx))
    }

    /// Ask for `users`' members in `guild` (at most
    /// [`crate::members::REQUEST_LIMIT`]), as the official client does
    /// (`requestGuildMembers`): they come back as [`Event::Members`]. Like
    /// the client, nothing is sent before the session is established.
    pub fn request_members(&self, guild: Snowflake, users: &[Snowflake]) {
        if self.state.established.get() {
            send_text(&self.conn, &request_text(8, &MemberRequest::new(guild, users)));
        }
    }

    /// Tell Discord what we show as, as the client does (`presenceUpdate`,
    /// 868): op 3, see [`crate::presence`]. Like the client, nothing is sent
    /// before the session is established.
    pub fn update_presence(&self, status: UserStatus) {
        if self.state.established.get() {
            send_text(&self.conn, &request_text(3, &PresenceUpdate::new(status)));
        }
    }

    /// Ask for how many messages of a forum's posts we haven't read, as the
    /// client does when it lists them (`requestForumUnreads`, 868): op 28,
    /// answered with [`Event::ForumUnreads`]. `request` is
    /// [`crate::forum::UnreadRequest`]. Like the client, nothing is sent
    /// before the session is established.
    pub fn request_forum_unreads(&self, request: &crate::forum::UnreadRequest) {
        if self.state.established.get() {
            send_text(&self.conn, &request_text(28, request));
        }
    }

    /// Send what [`crate::subscriptions::Subscriptions`] says to, the op 37
    /// messages the client sends. Like the client, nothing is sent before
    /// the session is established.
    pub fn send_subscriptions(&self, messages: &[String]) {
        if self.state.established.get() {
            for message in messages {
                send_text(&self.conn, message);
            }
        }
    }
}

/// Op 8's payload, in the client's field order.
#[derive(serde::Serialize)]
struct MemberRequest<'a> {
    guild_id: [Snowflake; 1],
    user_ids: &'a [Snowflake],
    presences: bool,
}

impl<'a> MemberRequest<'a> {
    fn new(guild: Snowflake, users: &'a [Snowflake]) -> Self {
        MemberRequest { guild_id: [guild], user_ids: users, presences: false }
    }
}

/// Op 3's payload, in the client's field order. Neucord reports no
/// activities, never goes idle on its own and is never away.
#[derive(serde::Serialize)]
struct PresenceUpdate {
    status: &'static str,
    since: u64,
    activities: [(); 0],
    afk: bool,
}

impl PresenceUpdate {
    fn new(status: UserStatus) -> Self {
        PresenceUpdate { status: status.wire(), since: 0, activities: [], afk: false }
    }
}

/// A payload as the client packs it: `{"op":…,"d":…}`.
pub(crate) fn request_text(op: u8, d: &impl serde::Serialize) -> String {
    #[derive(serde::Serialize)]
    struct Payload<'a, D> {
        op: u8,
        d: &'a D,
    }
    serde_json::to_string(&Payload { op, d }).unwrap_or_default()
}

fn send_text(conn: &soup::WebsocketConnection, text: &str) {
    if conn.state() == soup::WebsocketState::Open {
        conn.send_text(text);
    }
}

fn send(conn: &soup::WebsocketConnection, value: Value) {
    send_text(conn, &value.to_string());
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

/// Everyone's members a guild object carries, as in [`guild_with_member`],
/// with their roles.
fn members_of(value: &Value, merged: &Value) -> Vec<(Snowflake, Vec<Snowflake>)> {
    let members = value["members"].as_array().into_iter().chain(merged.as_array()).flatten();
    members.filter_map(member_roles).collect()
}

/// A member's user and roles; it names its user either inline or by id.
fn member_roles(member: &Value) -> Option<(Snowflake, Vec<Snowflake>)> {
    let user = parse_id(&member["user"]["id"]).or_else(|| parse_id(&member["user_id"]))?;
    Some((user, Member::from_value(member).roles))
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
            state.established.set(true);
            if let (Some(id), Some(url)) = (data["session_id"].as_str(), data["resume_gateway_url"].as_str()) {
                *state.session.borrow_mut() = Some((id.to_owned(), url.to_owned()));
            }
            let raw_guilds = data["guilds"].as_array().map(Vec::as_slice).unwrap_or_default();
            let guilds: Vec<Guild> = raw_guilds
                .iter()
                .enumerate()
                .filter_map(|(i, g)| guild_with_member(g, &data["merged_members"][i], user.id))
                .collect();
            let experiments = Experiments::from_ready(&data);
            crate::experiments::debug_dump(&data, &experiments, user.id, &guilds);
            let mut members = Members::default();
            for (i, g) in raw_guilds.iter().enumerate() {
                if let Some(id) = parse_id(&g["id"]) {
                    members.add(id, members_of(g, &data["merged_members"][i]), &[]);
                }
            }
            Event::Ready(Box::new(Ready {
                private_channels: private_channels(&data),
                relationships: Relationships::from_ready(&data),
                read_states: ReadStates::from_ready(&data["read_state"]),
                notification_settings: NotificationSettings::from_ready(&data["user_guild_settings"]),
                settings: {
                    // The protobuf is what the official client reads, so it
                    // wins where both say something.
                    let mut settings = Settings::default();
                    settings.apply(SettingsPatch::from_json(&data["user_settings"]));
                    if let Some(proto) = data["user_settings_proto"].as_str().and_then(SettingsPatch::from_proto) {
                        settings.apply(proto);
                    }
                    settings
                },
                settings_proto: data["user_settings_proto"].as_str().map(settings_proto::decode),
                media_takedown_regulated: crate::regional::media_takedown_regulated(&data["regional_feature_config"]),
                required_action: data["required_action"].as_str().map(str::to_owned),
                experiments,
                session_id: data["session_id"].as_str().unwrap_or_default().to_owned(),
                gates: Gates::from_ready(&data),
                unavailable_guilds: raw_guilds.iter().filter(|g| g["unavailable"] == true).count(),
                account,
                nitro: Nitro::from_user(&data["user"]),
                user,
                guilds,
                members,
            }))
        }
        "RESUMED" => {
            state.established.set(true);
            Event::Resumed
        }
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
            let patch = SettingsPatch::from_json(&data);
            if patch.is_empty() {
                return;
            }
            Event::SettingsUpdate { patch, replace: false, proto: None }
        }
        // `type` 1 is the account's settings, 2 its favourites and what it
        // uses most.
        "USER_SETTINGS_PROTO_UPDATE" => {
            let kind = data["settings"]["type"].as_u64();
            let Some(proto) = data["settings"]["proto"].as_str().map(settings_proto::decode) else {
                return eprintln!("gateway: bad USER_SETTINGS_PROTO_UPDATE");
            };
            if kind == Some(2) {
                return send_event(
                    state,
                    Event::FrecencyUpdate { proto, partial: data["partial"].as_bool() == Some(true) },
                );
            }
            if kind != Some(1) {
                return;
            }
            let Some(patch) = SettingsPatch::from_proto_bytes(&proto) else {
                return eprintln!("gateway: bad USER_SETTINGS_PROTO_UPDATE");
            };
            Event::SettingsUpdate { patch, replace: data["partial"].as_bool() != Some(true), proto: Some(proto) }
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
        // Discord may send other members' too, whose roles colour their names.
        "GUILD_MEMBER_UPDATE" => {
            let Some(guild_id) = parse_id(&data["guild_id"]) else { return };
            if !state.me.get().is_some_and(|me| is_me(&data, me)) {
                let Some(member) = member_roles(&data) else { return };
                return send_event(state, Event::Members { guild_id, members: vec![member], not_found: Vec::new() });
            }
            Event::MemberUpdate { guild_id, member: Member::from_value(&data) }
        }
        "GUILD_MEMBERS_CHUNK" => {
            let Some(guild_id) = parse_id(&data["guild_id"]) else { return };
            let members = data["members"].as_array().into_iter().flatten().filter_map(member_roles).collect();
            let not_found = data["not_found"].as_array().into_iter().flatten().filter_map(parse_id).collect();
            Event::Members { guild_id, members, not_found }
        }
        "GUILD_UPDATE" => match Guild::deserialize(&data) {
            Ok(guild) => Event::GuildUpdate(guild),
            Err(e) => return eprintln!("gateway: bad GUILD_UPDATE: {e}"),
        },
        "GUILD_EMOJIS_UPDATE" => match parse_id(&data["guild_id"]) {
            Some(guild_id) => match GuildEmoji::list(guild_id, &data["emojis"]) {
                Some(emojis) => Event::GuildEmojisUpdate { guild_id, emojis },
                None => return eprintln!("gateway: bad GUILD_EMOJIS_UPDATE"),
            },
            None => return,
        },
        "GUILD_STICKERS_UPDATE" => match parse_id(&data["guild_id"]) {
            Some(guild_id) => match Listed::guild_list(guild_id, &data["stickers"]) {
                Some(stickers) => Event::GuildStickersUpdate { guild_id, stickers },
                None => return eprintln!("gateway: bad GUILD_STICKERS_UPDATE"),
            },
            None => return,
        },
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
            match (parse_id(&data["guild_id"]), Role::deserialize(&data["role"])) {
                (Some(guild_id), Ok(role)) => Event::RoleUpdate { guild_id, role },
                _ => return eprintln!("gateway: bad {name}"),
            }
        }
        "GUILD_ROLE_DELETE" => match (parse_id(&data["guild_id"]), parse_id(&data["role_id"])) {
            (Some(guild_id), Some(role_id)) => Event::RoleDelete { guild_id, role_id },
            _ => return,
        },
        "USER_UPDATE" => match Account::from_user(&data) {
            Some(account) if Some(account.id) == state.me.get() => {
                Event::AccountUpdate(account, Nitro::from_user(&data))
            }
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
        "USER_NOTE_UPDATE" => match parse_id(&data["id"]) {
            Some(user_id) => Event::UserNoteUpdate { user_id, note: data["note"].as_str().map(String::from) },
            None => return,
        },
        // Friends' presences, and per-server ones; only `user.id` is certain.
        "PRESENCE_UPDATE" => match parse_id(&data["user"]["id"]) {
            Some(user_id) => Event::PresenceUpdate {
                user_id,
                guild_id: parse_id(&data["guild_id"]),
                presence: Presence::from_value(&data),
            },
            None => return,
        },
        // The members in it also colour names in the chat, as in the client.
        "GUILD_MEMBER_LIST_UPDATE" => {
            let Some(update) = MemberListUpdate::from_value(&data) else {
                return eprintln!("gateway: bad GUILD_MEMBER_LIST_UPDATE");
            };
            let members = update.members().iter().map(|m| (m.user.id, m.roles.clone())).collect();
            send_event(state, Event::Members { guild_id: update.guild, members, not_found: Vec::new() });
            Event::MemberList(Box::new(update))
        }
        "THREAD_CREATE" | "THREAD_UPDATE" => match (Channel::deserialize(&data), parse_id(&data["guild_id"])) {
            (Ok(thread), Some(guild_id)) if thread.kind.is_thread() && name == "THREAD_CREATE" => Event::ThreadCreate {
                guild_id,
                thread,
                newly_created: data["newly_created"].as_bool().unwrap_or(false),
            },
            (Ok(thread), Some(guild_id)) if thread.kind.is_thread() => Event::ThreadUpdate { guild_id, thread },
            (Ok(_), _) => return,
            (Err(e), _) => return eprintln!("gateway: bad {name}: {e}"),
        },
        "THREAD_DELETE" => match (parse_id(&data["guild_id"]), parse_id(&data["id"])) {
            (Some(guild_id), Some(id)) => Event::ThreadDelete { guild_id, id },
            _ => return,
        },
        "THREAD_LIST_SYNC" => {
            let Some(guild_id) = parse_id(&data["guild_id"]) else { return };
            let threads = data["threads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| Channel::deserialize(t).ok())
                .filter(|t| t.kind.is_thread())
                .collect();
            let members = data["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| Some((parse_id(&m["id"])?, ThreadMember::deserialize(m).ok()?)))
                .collect();
            Event::ThreadListSync { guild_id, threads, members, complete: data["channel_ids"].is_null() }
        }
        // The client drops an answer Discord says we may not have (225123).
        "FORUM_UNREADS" => {
            if data["permission_denied"].as_bool().unwrap_or(false) {
                return;
            }
            let Some(channel_id) = parse_id(&data["channel_id"]) else { return };
            let threads = data["threads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| Some((parse_id(&t["thread_id"])?, t["count"].as_u64().map(|n| n as u32))))
                .collect();
            Event::ForumUnreads { channel_id, threads }
        }
        // Only ours: the client keeps no one else's.
        "THREAD_MEMBER_UPDATE" => {
            let (Some(guild_id), Some(thread)) = (parse_id(&data["guild_id"]), parse_id(&data["id"])) else { return };
            if parse_id(&data["user_id"]) != state.me.get() {
                return;
            }
            match ThreadMember::deserialize(&data) {
                Ok(member) => Event::ThreadMember { guild_id, thread, member: Some(member) },
                Err(e) => return eprintln!("gateway: bad THREAD_MEMBER_UPDATE: {e}"),
            }
        }
        // Who joined and left; ours, if we're among them.
        "THREAD_MEMBERS_UPDATE" => {
            let (Some(guild_id), Some(thread), Some(me)) =
                (parse_id(&data["guild_id"]), parse_id(&data["id"]), state.me.get())
            else {
                return;
            };
            let removed =
                data["removed_member_ids"].as_array().into_iter().flatten().any(|id| parse_id(id) == Some(me));
            let added = data["added_members"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| parse_id(&m["user_id"]) == Some(me))
                .and_then(|m| ThreadMember::deserialize(m).ok());
            match added {
                Some(member) => Event::ThreadMember { guild_id, thread, member: Some(member) },
                None if removed => Event::ThreadMember { guild_id, thread, member: None },
                None => return,
            }
        }
        "CHANNEL_DELETE" => {
            let is_private = data["guild_id"].is_null();
            match parse_id(&data["id"]) {
                Some(id) if is_private => Event::PrivateChannelDelete(id),
                _ => return,
            }
        }
        "GUILD_CREATE" => {
            let Some(me) = state.me.get() else { return };
            let Some(guild) = guild_with_member(&data, &Value::Null, me) else { return };
            let members =
                Event::Members { guild_id: guild.id, members: members_of(&data, &Value::Null), not_found: Vec::new() };
            send_event(state, Event::GuildCreate(guild));
            members
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
                poll: data.get("poll").and_then(|p| crate::poll::Poll::deserialize(p).ok()),
                components: data.get("components").and_then(|c| Vec::<Component>::deserialize(c).ok()),
            })
        }
        "INTERACTION_SUCCESS"
        | "INTERACTION_FAILURE"
        | "INTERACTION_MODAL_CREATE"
        | "INTERACTION_IFRAME_MODAL_CREATE" => {
            // Without the nonce we sent, it isn't one of ours.
            let Some(nonce) = parse_id(&data["nonce"]) else { return };
            let outcome = match name {
                "INTERACTION_SUCCESS" => Outcome::Success,
                "INTERACTION_FAILURE" => Outcome::Failure,
                _ => Outcome::Form,
            };
            Event::Interaction { nonce, outcome }
        }
        "MESSAGE_DELETE" => {
            let (Some(id), Some(channel_id)) = (parse_id(&data["id"]), parse_id(&data["channel_id"])) else {
                return;
            };
            Event::MessageDelete { id, channel_id }
        }
        "MESSAGE_POLL_VOTE_ADD" | "MESSAGE_POLL_VOTE_REMOVE" => {
            let (Some(channel_id), Some(message_id), Some(user), Some(answer)) = (
                parse_id(&data["channel_id"]),
                parse_id(&data["message_id"]),
                parse_id(&data["user_id"]),
                data["answer_id"].as_u64().and_then(|a| u32::try_from(a).ok()),
            ) else {
                return;
            };
            Event::PollVote {
                channel_id,
                message_id,
                votes: vec![(answer, user)],
                added: name.ends_with("ADD"),
                many: false,
            }
        }
        "MESSAGE_POLL_VOTE_ADD_MANY" => {
            let (Some(channel_id), Some(message_id)) = (parse_id(&data["channel_id"]), parse_id(&data["message_id"]))
            else {
                return;
            };
            let votes = data["votes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((u32::try_from(v["answer_id"].as_u64()?).ok()?, v["users"].as_array()?)))
                .flat_map(|(answer, users)| users.iter().filter_map(parse_id).map(move |user| (answer, user)))
                .collect();
            Event::PollVote { channel_id, message_id, votes, added: true, many: true }
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let (Some(channel_id), Some(message_id), Some(user_id), Ok(emoji), Some(kind)) = (
                parse_id(&data["channel_id"]),
                parse_id(&data["message_id"]),
                parse_id(&data["user_id"]),
                Emoji::deserialize(&data["emoji"]),
                Kind::of_event(data["type"].as_u64()),
            ) else {
                return;
            };
            let colors = data["burst_colors"].as_array().into_iter().flatten().filter_map(Value::as_str);
            Event::Reaction(reactions::Reacted {
                channel_id,
                message_id,
                user_id,
                emoji,
                kind,
                colors: colors.map(String::from).collect(),
                added: name.ends_with("ADD"),
            })
        }
        "MESSAGE_REACTION_ADD_MANY" => {
            let (Some(channel_id), Some(message_id)) = (parse_id(&data["channel_id"]), parse_id(&data["message_id"]))
            else {
                return;
            };
            let reactions = data["reactions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let users = r["users"].as_array()?.iter().filter_map(parse_id).collect();
                    Some((Emoji::deserialize(&r["emoji"]).ok()?, users))
                })
                .collect();
            Event::ReactionsAdded { channel_id, message_id, reactions }
        }
        "MESSAGE_REACTION_REMOVE_ALL" => {
            let (Some(channel_id), Some(message_id)) = (parse_id(&data["channel_id"]), parse_id(&data["message_id"]))
            else {
                return;
            };
            Event::ReactionsCleared { channel_id, message_id }
        }
        "MESSAGE_REACTION_REMOVE_EMOJI" => {
            let (Some(channel_id), Some(message_id), Ok(emoji)) =
                (parse_id(&data["channel_id"]), parse_id(&data["message_id"]), Emoji::deserialize(&data["emoji"]))
            else {
                return;
            };
            Event::EmojiReactionsCleared { channel_id, message_id, emoji }
        }
        "MESSAGE_DELETE_BULK" => {
            let Some(channel_id) = parse_id(&data["channel_id"]) else { return };
            let ids = data["ids"].as_array().into_iter().flatten().filter_map(parse_id).collect();
            Event::MessageDeleteBulk { ids, channel_id }
        }
        "TYPING_START" => match typing_start(&data) {
            Some(event) => event,
            None => return,
        },
        _ => return,
    };
    send_event(state, event);
}

fn typing_start(data: &Value) -> Option<Event> {
    Some(Event::TypingStart {
        channel_id: parse_id(&data["channel_id"])?,
        user_id: parse_id(&data["user_id"])?,
        user: User::deserialize(&data["member"]["user"]).ok(),
    })
}

fn send_event(state: &State, event: Event) {
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
        let raw = json!({"id": "1", "name": "G", "default_message_notifications": 1,
                         "roles": [{"id": "1", "permissions": "1024"}, {"id": "40", "permissions": "2048"}], "members": [
            {"user": {"id": "8"}, "roles": ["70"], "communication_disabled_until": "2026-10-28T12:00:00+00:00"},
            {"user": {"id": "5"}, "roles": ["40", "41"], "communication_disabled_until": null}
        ]});
        let guild = guild_with_member(&raw, &Value::Null, Snowflake(5)).unwrap();
        assert_eq!(guild.my_roles, vec![Snowflake(40), Snowflake(41)]);
        assert_eq!(guild.role(Snowflake(40)).map(|r| r.permissions.0), Some(2048));
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
            established: Cell::new(false),
            seq: Cell::new(None),
            acked: Cell::new(true),
            events: tx,
        };
        dispatch(&state, "READY", ready);
        let Ok(Event::Ready(ready)) = rx.try_recv() else { panic!("no READY") };
        let noon = glib::DateTime::from_iso8601("2026-10-28T12:00:00Z", None).unwrap().to_unix();
        let a = &ready.guilds[0];
        assert_eq!((a.my_roles.as_slice(), a.timed_out_until), (&[Snowflake(40)][..], Some(noon + 1)));
        let me = Account { id: Snowflake(5), ..Default::default() };
        assert_eq!(a.timeout_left(&me, noon), Some(1), "a part second rounds up, never down");
        assert_eq!(a.timeout_left(&me, noon + 1), None);
        assert_eq!(ready.guilds[1].timed_out_until, None);

        let update = |user: &str, until: Value| {
            dispatch(
                &state,
                "GUILD_MEMBER_UPDATE",
                json!({"guild_id": "1", "user": {"id": user}, "roles": ["40"], "communication_disabled_until": until}),
            );
            rx.try_recv().ok()
        };
        assert!(
            matches!(update("8", json!("2026-10-28T12:00:00+00:00")), Some(Event::Members { .. })),
            "not ours, only someone's roles"
        );
        let Some(Event::MemberUpdate { guild_id, member }) = update("5", json!("2026-10-28T12:00:00+00:00")) else {
            panic!("our member")
        };
        assert_eq!((guild_id, member.timed_out_until), (Snowflake(1), Some(noon)));
        let Some(Event::MemberUpdate { member, .. }) = update("5", Value::Null) else { panic!("our member") };
        assert_eq!(member.timed_out_until, None, "the timeout lifted early");

        dispatch(
            &state,
            "GUILD_UPDATE",
            json!({"id": "1", "name": "A", "owner_id": "8", "verification_level": 3, "mfa_level": 1,
                   "premium_tier": 1, "roles": [{"id": "1", "permissions": "1024"}]}),
        );
        let Ok(Event::GuildUpdate(update)) = rx.try_recv() else { panic!("no guild update") };
        assert_eq!(update.premium_tier, 1);
        assert_eq!(
            (update.id, update.owner_id, update.verification.level, update.requires_mfa, update.roles.len()),
            (Snowflake(1), Some(Snowflake(8)), crate::verification::VerificationLevel::High, true, 1),
            "ownership can move, and with it every permission"
        );

        dispatch(
            &state,
            "GUILD_EMOJIS_UPDATE",
            json!({"guild_id": "1", "emojis": [{"id": "9", "name": "blob", "available": true, "roles": ["40"]}]}),
        );
        let Ok(Event::GuildEmojisUpdate { guild_id, emojis }) = rx.try_recv() else { panic!("no emoji update") };
        assert_eq!((guild_id, emojis.len(), emojis[0].roles.clone()), (Snowflake(1), 1, vec![Snowflake(40)]));

        dispatch(
            &state,
            "GUILD_STICKERS_UPDATE",
            json!({"guild_id": "1", "stickers": [{"id": "8", "name": "wave", "format_type": 1, "available": true}]}),
        );
        let Ok(Event::GuildStickersUpdate { guild_id, stickers }) = rx.try_recv() else { panic!("no sticker update") };
        assert_eq!((guild_id, stickers.len(), stickers[0].sticker.name.as_str()), (Snowflake(1), 1, "wave"));
        dispatch(&state, "GUILD_STICKERS_UPDATE", json!({"guild_id": "1", "stickers": []}));
        let Ok(Event::GuildStickersUpdate { stickers, .. }) = rx.try_recv() else { panic!("no sticker update") };
        assert!(stickers.is_empty(), "a server that lost its last sticker");

        dispatch(
            &state,
            "GUILD_ROLE_UPDATE",
            json!({"guild_id": "1", "role": {"id": "40", "name": "Mod", "permissions": "8192"}}),
        );
        let Ok(Event::RoleUpdate { guild_id, role }) = rx.try_recv() else { panic!("no role update") };
        assert_eq!((guild_id, role.id, role.permissions.0), (Snowflake(1), Snowflake(40), 8192));
        dispatch(&state, "GUILD_ROLE_DELETE", json!({"guild_id": "1", "role_id": "40"}));
        let Ok(Event::RoleDelete { role_id, .. }) = rx.try_recv() else { panic!("no role delete") };
        assert_eq!(role_id, Snowflake(40));
        dispatch(&state, "USER_UPDATE", json!({"id": "8", "verified": true}));
        assert!(rx.try_recv().is_err(), "someone else");
        dispatch(
            &state,
            "USER_UPDATE",
            json!({"id": "5", "verified": true, "email": "a@example.org", "mfa_enabled": true, "premium_type": 2}),
        );
        let Ok(Event::AccountUpdate(account, nitro)) = rx.try_recv() else { panic!("no account update") };
        assert!(account.verified && account.email && !account.phone && account.mfa);
        assert_eq!(nitro.premium_type, Some(crate::limits::PremiumType::Tier2));

        dispatch(&state, "CHANNEL_UPDATE", json!({"id": "9", "type": 0, "guild_id": "1", "rate_limit_per_user": 30}));
        let Ok(Event::GuildChannelUpdate { guild_id, channel }) = rx.try_recv() else { panic!("no channel update") };
        assert_eq!((guild_id, channel.rate_limit_per_user), (Snowflake(1), 30));
    }

    #[test]
    fn other_members() {
        let (tx, rx) = async_channel::unbounded();
        let state = State {
            token: String::new(),
            me: Cell::new(None),
            session: RefCell::new(None),
            resuming: Cell::new(false),
            resumable: Cell::new(true),
            established: Cell::new(false),
            seq: Cell::new(None),
            acked: Cell::new(true),
            events: tx,
        };
        let ready = json!({
            "user": {"id": "5", "username": "me", "premium_type": 3},
            "guilds": [{"id": "1", "properties": {"name": "A", "premium_tier": 2}}],
            "merged_members": [[{"user_id": "5", "roles": []}, {"user_id": "6", "roles": ["40"]}]]
        });
        dispatch(&state, "READY", ready);
        let Ok(Event::Ready(ready)) = rx.try_recv() else { panic!("no READY") };
        assert_eq!(ready.nitro.premium_type, Some(crate::limits::PremiumType::Tier0));
        assert_eq!(ready.guilds[0].premium_tier, 2);
        assert_eq!(ready.members.roles(Snowflake(1), Snowflake(6)), Some(&[Snowflake(40)][..]));
        assert!(state.established.get());

        dispatch(
            &state,
            "GUILD_CREATE",
            json!({"id": "2", "name": "B", "members": [{"user": {"id": "7"}, "roles": ["41"]}]}),
        );
        assert!(matches!(rx.try_recv(), Ok(Event::GuildCreate(_))));
        let Ok(Event::Members { guild_id, members, .. }) = rx.try_recv() else { panic!("no members") };
        assert_eq!((guild_id, members), (Snowflake(2), vec![(Snowflake(7), vec![Snowflake(41)])]));

        dispatch(&state, "GUILD_MEMBER_UPDATE", json!({"guild_id": "1", "user": {"id": "6"}, "roles": []}));
        let Ok(Event::Members { members, .. }) = rx.try_recv() else { panic!("someone else's update") };
        assert_eq!(members, vec![(Snowflake(6), vec![])]);

        dispatch(
            &state,
            "GUILD_MEMBERS_CHUNK",
            json!({"guild_id": "1", "members": [{"user": {"id": "8"}, "roles": ["40"]}], "not_found": ["9"],
                   "chunk_index": 0, "chunk_count": 1}),
        );
        let Ok(Event::Members { members, not_found, .. }) = rx.try_recv() else { panic!("no chunk") };
        assert_eq!((members, not_found), (vec![(Snowflake(8), vec![Snowflake(40)])], vec![Snowflake(9)]));
    }

    #[test]
    fn threads() {
        let (tx, rx) = async_channel::unbounded();
        let state = State {
            token: String::new(),
            me: Cell::new(Some(Snowflake(5))),
            session: RefCell::new(None),
            resuming: Cell::new(false),
            resumable: Cell::new(true),
            established: Cell::new(true),
            seq: Cell::new(None),
            acked: Cell::new(true),
            events: tx,
        };
        let member = json!({"flags": 2, "muted": false, "mute_config": null, "join_timestamp": "2026-10-28T00:00:00Z"});
        let thread = json!({"id": "20", "guild_id": "1", "parent_id": "9", "type": 11, "name": "t",
                            "thread_metadata": {"archived": false, "locked": true, "auto_archive_duration": 1440,
                                                "archive_timestamp": "2026-10-27T00:00:00Z"},
                            "member": member, "newly_created": true});
        dispatch(&state, "THREAD_CREATE", thread);
        let Ok(Event::ThreadCreate { guild_id, thread, newly_created }) = rx.try_recv() else { panic!("no thread") };
        assert_eq!((guild_id, thread.parent_id, newly_created), (Snowflake(1), Some(Snowflake(9)), true));
        assert!(thread.is_locked() && thread.is_active_thread());
        assert_eq!(thread.member.map(|m| (m.flags, m.join_timestamp)), Some((2, Some(1_793_145_600_000))));

        dispatch(&state, "THREAD_UPDATE", json!({"id": "9", "guild_id": "1", "type": 0}));
        assert!(rx.try_recv().is_err(), "not a thread");
        dispatch(&state, "THREAD_UPDATE", json!({"id": "20", "guild_id": "1", "parent_id": "9", "type": 11}));
        assert!(matches!(rx.try_recv(), Ok(Event::ThreadUpdate { .. })), "an update isn't a creation");
        dispatch(&state, "THREAD_DELETE", json!({"id": "20", "guild_id": "1", "parent_id": "9", "type": 11}));
        assert!(matches!(rx.try_recv(), Ok(Event::ThreadDelete { id: Snowflake(20), .. })));

        dispatch(
            &state,
            "THREAD_LIST_SYNC",
            json!({"guild_id": "1", "threads": [{"id": "21", "type": 12, "parent_id": "9"}],
                   "members": [{"id": "21", "user_id": "5", "flags": 0, "join_timestamp": "2026-10-28T00:00:00Z"}]}),
        );
        let Ok(Event::ThreadListSync { threads, members, complete, .. }) = rx.try_recv() else { panic!("no sync") };
        assert_eq!((threads.len(), members[0].0, complete), (1, Snowflake(21), true));
        dispatch(
            &state,
            "THREAD_LIST_SYNC",
            json!({"guild_id": "1", "channel_ids": ["9"], "threads": [], "members": []}),
        );
        let Ok(Event::ThreadListSync { complete, .. }) = rx.try_recv() else { panic!("no sync") };
        assert!(!complete, "some channels' threads");

        dispatch(
            &state,
            "FORUM_UNREADS",
            json!({"guild_id": "1", "channel_id": "15", "threads": [
                {"thread_id": "20", "count": 3}, {"thread_id": "21", "missing": true}]}),
        );
        let Ok(Event::ForumUnreads { channel_id, threads }) = rx.try_recv() else { panic!("no unreads") };
        assert_eq!((channel_id, threads), (Snowflake(15), vec![(Snowflake(20), Some(3)), (Snowflake(21), None)]));
        dispatch(&state, "FORUM_UNREADS", json!({"channel_id": "15", "permission_denied": true, "threads": []}));
        assert!(rx.try_recv().is_err(), "denied");

        let ours = json!({"id": "20", "guild_id": "1", "user_id": "5", "flags": 8, "muted": true,
                          "mute_config": {"end_time": null}, "join_timestamp": "2026-10-28T00:00:00Z"});
        dispatch(&state, "THREAD_MEMBER_UPDATE", ours);
        let Ok(Event::ThreadMember { member: Some(member), .. }) = rx.try_recv() else { panic!("our member") };
        assert!(member.flags == 8 && member.is_muted(0));
        dispatch(&state, "THREAD_MEMBER_UPDATE", json!({"id": "20", "guild_id": "1", "user_id": "6", "flags": 0}));
        assert!(rx.try_recv().is_err(), "someone else's");

        let members = |added: &str, removed: &str| {
            dispatch(
                &state,
                "THREAD_MEMBERS_UPDATE",
                json!({"id": "20", "guild_id": "1", "member_count": 2,
                       "added_members": [{"id": "20", "user_id": added, "flags": 0, "join_timestamp": "2026-10-28T00:00:00Z"}],
                       "removed_member_ids": [removed]}),
            );
            rx.try_recv().ok()
        };
        assert!(matches!(members("5", "7"), Some(Event::ThreadMember { member: Some(_), .. })), "we joined");
        assert!(matches!(members("6", "5"), Some(Event::ThreadMember { member: None, .. })), "we left");
        assert!(members("6", "7").is_none(), "neither");
    }

    #[test]
    fn member_lists_and_presences_of_a_server() {
        let (state, rx) = fresh_state();
        dispatch(
            &state,
            "GUILD_MEMBER_LIST_UPDATE",
            json!({
                "guild_id": "1", "id": "everyone", "member_count": 2, "online_count": 1,
                "groups": [{"id": "online", "count": 1}],
                "ops": [{"op": "SYNC", "range": [0, 1], "items": [
                    {"group": {"id": "online", "count": 1}},
                    {"member": {"user": {"id": "6", "username": "u"}, "roles": ["40"], "presence": {"status": "online"}}},
                ]}],
            }),
        );
        // Their roles colour names in the chat, as the client's member store takes them.
        let Ok(Event::Members { guild_id, members, .. }) = rx.try_recv() else { panic!("no members") };
        assert_eq!((guild_id, members), (Snowflake(1), vec![(Snowflake(6), vec![Snowflake(40)])]));
        let Ok(Event::MemberList(update)) = rx.try_recv() else { panic!("no list") };
        assert_eq!((update.guild, update.list.as_str(), update.ops.len()), (Snowflake(1), "everyone", 1));

        dispatch(&state, "PRESENCE_UPDATE", json!({"user": {"id": "6"}, "guild_id": "1", "status": "idle"}));
        let Ok(Event::PresenceUpdate { user_id, guild_id, presence }) = rx.try_recv() else { panic!("no presence") };
        assert_eq!(
            (user_id, guild_id, presence.status),
            (Snowflake(6), Some(Snowflake(1)), crate::relationships::Status::Idle)
        );
        dispatch(&state, "PRESENCE_UPDATE", json!({"user": {"id": "6"}, "status": "online"}));
        let Ok(Event::PresenceUpdate { guild_id, .. }) = rx.try_recv() else { panic!("no presence") };
        assert_eq!(guild_id, None);
    }

    #[test]
    fn a_note_changed_elsewhere() {
        let (state, rx) = fresh_state();
        dispatch(&state, "USER_NOTE_UPDATE", json!({"id": "6", "note": "met at a meetup"}));
        let Ok(Event::UserNoteUpdate { user_id, note }) = rx.try_recv() else { panic!("no note") };
        assert_eq!((user_id, note.as_deref()), (Snowflake(6), Some("met at a meetup")));
        // The client stores what is missing as no note.
        dispatch(&state, "USER_NOTE_UPDATE", json!({"id": "6"}));
        let Ok(Event::UserNoteUpdate { note, .. }) = rx.try_recv() else { panic!("no note") };
        assert_eq!(note, None);
    }

    #[test]
    fn member_request_as_the_client_sends_it() {
        let d = MemberRequest::new(Snowflake(1), &[Snowflake(6), Snowflake(7)]);
        assert_eq!(request_text(8, &d), r#"{"op":8,"d":{"guild_id":["1"],"user_ids":["6","7"],"presences":false}}"#);
    }

    #[test]
    fn an_update_of_the_favourites_is_told_apart_from_the_settings() {
        let (state, rx) = fresh_state();
        let update = |kind: u64, partial: bool| json!({"settings": {"type": kind, "proto": settings_proto::encode(&[0x2a, 0x00])}, "partial": partial});
        dispatch(&state, "USER_SETTINGS_PROTO_UPDATE", update(2, true));
        let Ok(Event::FrecencyUpdate { proto, partial }) = rx.try_recv() else { panic!("no update") };
        assert_eq!((proto, partial), (vec![0x2a, 0x00], true));
        dispatch(&state, "USER_SETTINGS_PROTO_UPDATE", update(2, false));
        assert!(matches!(rx.try_recv(), Ok(Event::FrecencyUpdate { partial: false, .. })));
        dispatch(&state, "USER_SETTINGS_PROTO_UPDATE", update(3, false));
        assert!(rx.try_recv().is_err(), "a type nobody reads");
    }

    #[test]
    fn presence_update_as_the_client_sends_it() {
        assert_eq!(
            request_text(3, &PresenceUpdate::new(UserStatus::DoNotDisturb)),
            r#"{"op":3,"d":{"status":"dnd","since":0,"activities":[],"afk":false}}"#
        );
    }

    #[test]
    fn typing_in_a_dm_and_in_a_server() {
        let dm = json!({"channel_id": "10", "user_id": "5", "timestamp": 1});
        assert!(matches!(
            typing_start(&dm),
            Some(Event::TypingStart { channel_id: Snowflake(10), user_id: Snowflake(5), user: None })
        ));
        let server = json!({
            "channel_id": "10", "guild_id": "2", "user_id": "5",
            "member": {"user": {"id": "5", "username": "ann", "global_name": "Ann"}, "roles": []},
        });
        let Some(Event::TypingStart { user: Some(user), .. }) = typing_start(&server) else { panic!("no user") };
        assert_eq!(user.display_name(), "Ann");
        assert!(typing_start(&json!({"channel_id": "10"})).is_none(), "nobody typing");
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

    fn fresh_state() -> (State, async_channel::Receiver<Event>) {
        let (tx, rx) = async_channel::unbounded();
        let state = State {
            token: String::new(),
            me: Cell::new(Some(Snowflake(5))),
            session: RefCell::new(None),
            resuming: Cell::new(false),
            resumable: Cell::new(true),
            established: Cell::new(true),
            seq: Cell::new(None),
            acked: Cell::new(true),
            events: tx,
        };
        (state, rx)
    }

    #[test]
    fn a_bulk_delete_names_every_message_it_removes() {
        // `MESSAGE_DELETE_BULK` in 596344 removes `ids` from the channel's messages.
        let (state, rx) = fresh_state();
        dispatch(&state, "MESSAGE_DELETE_BULK", json!({"ids": ["7", "9", "x"], "channel_id": "3", "guild_id": "1"}));
        let Ok(Event::MessageDeleteBulk { ids, channel_id }) = rx.try_recv() else { panic!("no bulk delete") };
        assert_eq!((ids, channel_id), (vec![Snowflake(7), Snowflake(9)], Snowflake(3)));
        dispatch(&state, "MESSAGE_DELETE_BULK", json!({"ids": ["7"]}));
        assert!(rx.try_recv().is_err(), "no channel, nothing to remove from");
    }

    #[test]
    fn an_update_carries_the_poll_it_ended() {
        let (state, rx) = fresh_state();
        let poll = json!({"question": {"text": "?"}, "answers": [], "expiry": "2026-01-01T00:00:00+00:00",
                          "results": {"is_finalized": true, "answer_counts": []}});
        dispatch(&state, "MESSAGE_UPDATE", json!({"id": "9", "channel_id": "3", "poll": poll}));
        let Ok(Event::MessageUpdate(edit)) = rx.try_recv() else { panic!("no update") };
        assert!(edit.poll.is_some_and(|p| p.results.is_some_and(|r| r.is_finalized)));
        dispatch(&state, "MESSAGE_UPDATE", json!({"id": "9", "channel_id": "3", "content": "x"}));
        let Ok(Event::MessageUpdate(edit)) = rx.try_recv() else { panic!("no update") };
        assert!(edit.poll.is_none());
    }

    #[test]
    fn poll_votes_arrive_one_or_many() {
        // 225123 turns these into reactions of the vote type.
        let (state, rx) = fresh_state();
        let one = json!({"channel_id": "3", "message_id": "9", "user_id": "5", "answer_id": 2, "guild_id": "1"});
        dispatch(&state, "MESSAGE_POLL_VOTE_ADD", one.clone());
        let Ok(Event::PollVote { channel_id, message_id, votes, added, many }) = rx.try_recv() else {
            panic!("no vote")
        };
        assert_eq!(
            (channel_id, message_id, votes, added, many),
            (Snowflake(3), Snowflake(9), vec![(2, Snowflake(5))], true, false)
        );
        dispatch(&state, "MESSAGE_POLL_VOTE_REMOVE", one);
        let Ok(Event::PollVote { added, .. }) = rx.try_recv() else { panic!("no vote taken back") };
        assert!(!added);
        dispatch(
            &state,
            "MESSAGE_POLL_VOTE_ADD_MANY",
            json!({"channel_id": "3", "message_id": "9", "votes": [
                {"answer_id": 1, "users": ["5", "6"]}, {"answer_id": 2, "users": ["7"]}]}),
        );
        let Ok(Event::PollVote { votes, added, many, .. }) = rx.try_recv() else { panic!("no votes") };
        let all = [(1, Snowflake(5)), (1, Snowflake(6)), (2, Snowflake(7))];
        assert_eq!((votes, added, many), (all.to_vec(), true, true));
    }
    #[test]
    fn reactions_arrive_added_or_taken_back() {
        // 225123 passes `type` and `burst_colors` on with the rest.
        let (state, rx) = fresh_state();
        let fire = json!({"id": null, "name": "🔥"});
        let one = json!({"channel_id": "3", "message_id": "9", "user_id": "5", "emoji": fire, "guild_id": "1"});
        dispatch(&state, "MESSAGE_REACTION_ADD", one.clone());
        let Ok(Event::Reaction(reactions::Reacted { channel_id, message_id, user_id, emoji, kind, colors, added })) =
            rx.try_recv()
        else {
            panic!("no reaction")
        };
        assert_eq!((channel_id, message_id, user_id), (Snowflake(3), Snowflake(9), Snowflake(5)));
        assert_eq!((emoji.name.as_deref(), kind, colors.len(), added), (Some("🔥"), Kind::Normal, 0, true));
        dispatch(&state, "MESSAGE_REACTION_REMOVE", one);
        let Ok(Event::Reaction(reactions::Reacted { added, .. })) = rx.try_recv() else {
            panic!("no reaction taken back")
        };
        assert!(!added);

        let burst = json!({"channel_id": "3", "message_id": "9", "user_id": "5", "emoji": {"id": "12", "name": "blob"},
                           "type": 1, "burst_colors": ["#ff0000", "#00ff00"]});
        dispatch(&state, "MESSAGE_REACTION_ADD", burst);
        let Ok(Event::Reaction(reactions::Reacted { emoji, kind, colors, .. })) = rx.try_recv() else {
            panic!("no super reaction")
        };
        assert_eq!(
            (emoji.id, kind, colors),
            (Some(Snowflake(12)), Kind::Burst, vec!["#ff0000".into(), "#00ff00".into()])
        );
    }

    #[test]
    fn reactions_that_are_poll_votes_or_unreadable_are_dropped() {
        let (state, rx) = fresh_state();
        let base = json!({"channel_id": "3", "message_id": "9", "user_id": "5", "emoji": {"name": "🔥"}});
        let with = |key: &str, value: Value| {
            let mut event = base.clone();
            event[key] = value;
            event
        };
        dispatch(&state, "MESSAGE_REACTION_ADD", with("type", json!(2)));
        dispatch(&state, "MESSAGE_REACTION_ADD", with("emoji", json!("🔥")));
        dispatch(&state, "MESSAGE_REACTION_ADD", with("user_id", json!(null)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn many_reactions_arrive_at_once() {
        let (state, rx) = fresh_state();
        dispatch(
            &state,
            "MESSAGE_REACTION_ADD_MANY",
            json!({"channel_id": "3", "message_id": "9", "reactions": [
                {"emoji": {"name": "🔥"}, "users": ["5", "6"], "type": 0},
                {"emoji": {"id": "12", "name": "blob"}, "users": ["7"]},
                {"users": ["8"]}]}),
        );
        let Ok(Event::ReactionsAdded { channel_id, message_id, reactions }) = rx.try_recv() else {
            panic!("no reactions")
        };
        assert_eq!((channel_id, message_id), (Snowflake(3), Snowflake(9)));
        let named: Vec<_> = reactions.iter().map(|(e, users)| (e.id, users.clone())).collect();
        assert_eq!(
            named,
            [(None, vec![Snowflake(5), Snowflake(6)]), (Some(Snowflake(12)), vec![Snowflake(7)])],
            "one with no emoji is left out"
        );
    }

    #[test]
    fn reactions_removed_in_bulk() {
        let (state, rx) = fresh_state();
        dispatch(&state, "MESSAGE_REACTION_REMOVE_ALL", json!({"channel_id": "3", "message_id": "9"}));
        let Ok(Event::ReactionsCleared { channel_id, message_id }) = rx.try_recv() else { panic!("no clear") };
        assert_eq!((channel_id, message_id), (Snowflake(3), Snowflake(9)));
        dispatch(
            &state,
            "MESSAGE_REACTION_REMOVE_EMOJI",
            json!({"channel_id": "3", "message_id": "9", "emoji": {"id": "12", "name": "blob"}}),
        );
        let Ok(Event::EmojiReactionsCleared { emoji, .. }) = rx.try_recv() else { panic!("no emoji clear") };
        assert_eq!(emoji.id, Some(Snowflake(12)));
        dispatch(&state, "MESSAGE_REACTION_REMOVE_EMOJI", json!({"channel_id": "3", "message_id": "9"}));
        dispatch(&state, "MESSAGE_REACTION_REMOVE_ALL", json!({"channel_id": "3"}));
        assert!(rx.try_recv().is_err());
    }
}
