//! Who is in which voice channel, as the official client's `VoiceStateStore`
//! (977997) keeps it and its `SortedVoiceStateStore` (607567) orders it.
//!
//! A voice state is Discord's record of one user's connection to one voice
//! channel. It says nothing about what a user says or hears, only whether they
//! muted or deafened themselves, a moderator did, or they share a camera or a
//! screen. A user has at most one per server; a state without a channel is
//! the user leaving.

use std::collections::HashMap;

use serde_json::Value;

use crate::{Snowflake, User};

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceState {
    pub user_id: Snowflake,
    pub channel_id: Snowflake,
    /// The session of the client they are connected from, which says whether
    /// the state is ours or another device of ours.
    pub session_id: String,
    /// Muted and deafened by a moderator.
    pub mute: bool,
    pub deaf: bool,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub self_video: bool,
    pub self_stream: bool,
    /// In a stage channel: in the audience, not speaking.
    pub suppress: bool,
    /// The user's nickname in the server, when the update carried their member.
    pub nick: Option<String>,
    pub user: Option<User>,
}

impl VoiceState {
    /// What the client does with a record of a voice state (`eH` in 225123):
    /// one without a channel is the user leaving. `None` if it names no user.
    pub fn parse(value: &Value) -> Option<(Snowflake, Option<VoiceState>)> {
        let id = |v: &Value| v.as_str()?.parse().ok().map(Snowflake);
        let user_id = id(&value["user_id"])?;
        let Some(channel_id) = id(&value["channel_id"]) else { return Some((user_id, None)) };
        let flag = |key: &str| value[key].as_bool().unwrap_or(false);
        Some((
            user_id,
            Some(VoiceState {
                user_id,
                channel_id,
                session_id: value["session_id"].as_str().unwrap_or_default().to_owned(),
                mute: flag("mute"),
                deaf: flag("deaf"),
                self_mute: flag("self_mute"),
                self_deaf: flag("self_deaf"),
                self_video: flag("self_video"),
                self_stream: flag("self_stream"),
                suppress: flag("suppress"),
                nick: value["member"]["nick"].as_str().map(str::to_owned),
                user: serde_json::from_value(value["member"]["user"].clone()).ok(),
            }),
        ))
    }

    /// A server's `voice_states`.
    pub fn parse_list(states: &Value) -> Vec<VoiceState> {
        let entries = states.as_array().into_iter().flatten().filter_map(VoiceState::parse);
        entries.filter_map(|(_, state)| state).collect()
    }

    /// Muted by themselves or by a moderator, which the client draws the same.
    pub fn muted(&self) -> bool {
        self.mute || self.self_mute
    }

    pub fn deafened(&self) -> bool {
        self.deaf || self.self_deaf
    }
}

/// One user's state in one server changing, `None` for leaving.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub guild: Snowflake,
    pub user: Snowflake,
    pub state: Option<VoiceState>,
}

impl Update {
    /// `VOICE_STATE_UPDATE`, or one of the states in `VOICE_STATE_UPDATE_BATCH`.
    /// The ones that name no server are DM calls, which come with them.
    pub fn parse(value: &Value) -> Option<Update> {
        let guild = value["guild_id"].as_str()?.parse().ok().map(Snowflake)?;
        let (user, state) = VoiceState::parse(value)?;
        Some(Update { guild, user, state })
    }
}

/// Every voice state of every server we are in.
#[derive(Debug, Default)]
pub struct VoiceStates {
    guilds: HashMap<Snowflake, HashMap<Snowflake, VoiceState>>,
}

impl VoiceStates {
    /// A server's whole list in place of what was known (`GUILD_CREATE`, and
    /// each server READY carries).
    pub fn replace_guild(&mut self, guild: Snowflake, states: Vec<VoiceState>) {
        let states: HashMap<_, _> = states.into_iter().map(|s| (s.user_id, s)).collect();
        if states.is_empty() {
            self.guilds.remove(&guild);
        } else {
            self.guilds.insert(guild, states);
        }
    }

    /// Whether the list changed.
    pub fn apply(&mut self, update: Update) -> bool {
        let Update { guild, user, state } = update;
        match state {
            Some(state) => self.guilds.entry(guild).or_default().insert(user, state.clone()).as_ref() != Some(&state),
            None => {
                let Some(users) = self.guilds.get_mut(&guild) else { return false };
                let removed = users.remove(&user).is_some();
                if users.is_empty() {
                    self.guilds.remove(&guild);
                }
                removed
            }
        }
    }

    /// Who is in `channel`, in the order the client lists them: people
    /// sharing a screen first, then by name regardless of case, then by id.
    /// `name` is what the user is called, which the client takes from their
    /// nickname, else their display name.
    pub fn in_channel(
        &self,
        guild: Snowflake,
        channel: Snowflake,
        name: impl Fn(&VoiceState) -> String,
    ) -> Vec<&VoiceState> {
        let mut states: Vec<(String, &VoiceState)> = self
            .guilds
            .get(&guild)
            .into_iter()
            .flat_map(HashMap::values)
            .filter(|s| s.channel_id == channel)
            .map(|s| (comparator(s, &name(s)), s))
            .collect();
        // The client keeps a string per state and compares them as JavaScript does.
        states.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
        states.into_iter().map(|(_, s)| s).collect()
    }
}

/// The client's `R` in 607567.
fn comparator(state: &VoiceState, name: &str) -> String {
    let stream = if state.self_stream { '\0' } else { '\x01' };
    format!("{stream}{}\0{}", name.to_lowercase(), state.user_id.0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn state(user: u64, channel: u64) -> serde_json::Value {
        json!({"user_id": user.to_string(), "channel_id": channel.to_string(), "guild_id": "1", "session_id": "s"})
    }

    fn names(states: &VoiceStates, channel: u64) -> Vec<u64> {
        let name = |s: &VoiceState| s.nick.clone().unwrap_or_else(|| s.user_id.0.to_string());
        states.in_channel(Snowflake(1), Snowflake(channel), name).iter().map(|s| s.user_id.0).collect()
    }

    #[test]
    fn a_state_without_a_channel_is_the_user_leaving() {
        let mut leaving = state(5, 9);
        leaving["channel_id"] = Value::Null;
        assert_eq!(VoiceState::parse(&leaving), Some((Snowflake(5), None)));
        assert_eq!(VoiceState::parse(&json!({"channel_id": "9"})), None, "no user");
    }

    #[test]
    fn dm_calls_belong_to_no_server() {
        let mut call = state(5, 9);
        call["guild_id"] = Value::Null;
        assert_eq!(Update::parse(&call), None);
    }

    #[test]
    fn joining_moving_and_leaving() {
        let mut states = VoiceStates::default();
        let update = |v: Value| Update::parse(&v).unwrap();
        assert!(states.apply(update(state(5, 9))));
        assert!(!states.apply(update(state(5, 9))), "nothing changed");
        assert_eq!(names(&states, 9), [5]);
        assert!(states.apply(update(state(5, 8))));
        assert_eq!((names(&states, 9), names(&states, 8)), (vec![], vec![5]));
        let mut leaving = state(5, 8);
        leaving["channel_id"] = Value::Null;
        assert!(states.apply(update(leaving.clone())));
        assert!(!states.apply(update(leaving)), "already gone");
        assert!(states.guilds.is_empty());
    }

    #[test]
    fn a_server_list_replaces_what_was_known() {
        let mut states = VoiceStates::default();
        states.replace_guild(Snowflake(1), VoiceState::parse_list(&json!([state(5, 9), state(6, 9)])));
        states.replace_guild(Snowflake(1), VoiceState::parse_list(&json!([state(7, 9)])));
        assert_eq!(names(&states, 9), [7]);
        states.replace_guild(Snowflake(1), Vec::new());
        assert!(states.guilds.is_empty());
    }

    #[test]
    fn people_sharing_a_screen_come_first_then_names_without_case() {
        let mut states = VoiceStates::default();
        let mut with = |user: u64, nick: &str, stream: bool| {
            let mut v = state(user, 9);
            v["member"] = json!({"nick": nick});
            v["self_stream"] = json!(stream);
            states.apply(Update::parse(&v).unwrap());
        };
        with(1, "bob", false);
        with(2, "Alice", false);
        with(3, "zed", true);
        with(4, "alice", false);
        assert_eq!(names(&states, 9), [3, 2, 4, 1], "zed streams; then Alice and alice by id, then bob");
    }

    #[test]
    fn ids_compare_as_text() {
        let mut states = VoiceStates::default();
        states.replace_guild(Snowflake(1), VoiceState::parse_list(&json!([state(10, 9), state(9, 9)])));
        let same = |_: &VoiceState| "x".to_owned();
        let order: Vec<u64> = states.in_channel(Snowflake(1), Snowflake(9), same).iter().map(|s| s.user_id.0).collect();
        assert_eq!(order, [10, 9], "\"10\" sorts before \"9\", as in the client");
    }
}
