//! The status text of voice channels, as the client's `ChannelStatusStore`
//! keeps it (309698): what someone set on the channel, shown under its name
//! in the sidebar. Statuses arrive in answer to op 43, which the client sends
//! once for a server the first time one of its voice channels is drawn
//! (`requestChannelInfo` in 868, from 52074), and afterwards as
//! `VOICE_CHANNEL_STATUS_UPDATE`.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::Snowflake;

/// Op 43's payload, in the client's field order and with its fields:
/// `requestChannelInfo(guild, ["status", "voice_start_time"])` (52074).
#[derive(Serialize)]
pub struct InfoRequest {
    guild_id: Snowflake,
    fields: [&'static str; 2],
}

impl InfoRequest {
    pub fn new(guild: Snowflake) -> Self {
        InfoRequest { guild_id: guild, fields: ["status", "voice_start_time"] }
    }
}

/// The statuses `CHANNEL_INFO` names, by channel; none for a channel with none.
pub type Statuses = Vec<(Snowflake, Option<String>)>;

/// What `CHANNEL_INFO` says of a server's voice channels, which replaces what
/// was known of them.
pub fn parse_info(data: &Value) -> Option<(Snowflake, Statuses)> {
    let id = |value: &Value| value.as_str()?.parse().ok().map(Snowflake);
    let channels = data["channels"]
        .as_array()?
        .iter()
        .filter_map(|channel| Some((id(&channel["id"])?, status(&channel["status"]))))
        .collect();
    Some((id(&data["guild_id"])?, channels))
}

/// A status, none when it is null or empty.
pub fn status(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty()).map(str::to_owned)
}

#[derive(Clone, Debug, Default)]
pub struct ChannelStatuses {
    by_channel: HashMap<(Snowflake, Snowflake), String>,
    /// The servers asked for since the connection opened.
    asked: HashSet<Snowflake>,
}

impl ChannelStatuses {
    /// Whether to ask for `guild`'s statuses now: once a connection, and
    /// from then on it counts as asked.
    pub fn ask(&mut self, guild: Snowflake) -> bool {
        self.asked.insert(guild)
    }

    /// `CHANNEL_INFO`: these are the statuses of `guild`'s channels, whatever
    /// was known of them before.
    pub fn replace_guild(&mut self, guild: Snowflake, channels: Statuses) {
        self.by_channel.retain(|(g, _), _| *g != guild);
        for (channel, status) in channels {
            self.set(guild, channel, status);
        }
    }

    /// `VOICE_CHANNEL_STATUS_UPDATE`.
    pub fn set(&mut self, guild: Snowflake, channel: Snowflake, status: Option<String>) {
        match status {
            Some(status) => self.by_channel.insert((guild, channel), status),
            None => self.by_channel.remove(&(guild, channel)),
        };
    }

    /// `GUILD_CREATE`: the server is asked for again.
    pub fn asked_again(&mut self, guild: Snowflake) {
        self.asked.remove(&guild);
    }

    /// `GUILD_DELETE`: a server that is gone keeps nothing, and is asked for
    /// again if it comes back.
    pub fn forget_guild(&mut self, guild: Snowflake) {
        self.asked_again(guild);
        self.by_channel.retain(|(g, _), _| *g != guild);
    }

    /// `CONNECTION_OPEN` and `CONNECTION_RESUMED`.
    pub fn reconnected(&mut self) {
        self.asked.clear();
    }

    pub fn of(&self, guild: Snowflake, channel: Snowflake) -> Option<&str> {
        self.by_channel.get(&(guild, channel)).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const GUILD: Snowflake = Snowflake(1);
    const LOUNGE: Snowflake = Snowflake(2);
    const QUIET: Snowflake = Snowflake(3);

    #[test]
    fn the_request_is_op_43_as_the_client_sends_it() {
        let text = crate::gateway::request_text(43, &InfoRequest::new(Snowflake(555)));
        assert_eq!(text, r#"{"op":43,"d":{"guild_id":"555","fields":["status","voice_start_time"]}}"#);
    }

    #[test]
    fn a_server_is_asked_for_once_until_the_client_would_ask_again() {
        let mut statuses = ChannelStatuses::default();
        assert!(statuses.ask(GUILD));
        assert!(!statuses.ask(GUILD), "once");
        assert!(statuses.ask(Snowflake(9)), "another server");
        statuses.asked_again(GUILD);
        assert!(statuses.ask(GUILD), "after GUILD_CREATE");
        statuses.forget_guild(GUILD);
        assert!(statuses.ask(GUILD), "and GUILD_DELETE");
        statuses.reconnected();
        assert!(statuses.ask(GUILD) && statuses.ask(Snowflake(9)), "after a connection opens or resumes");
    }

    #[test]
    fn what_channel_info_says_replaces_what_was_known() {
        let mut statuses = ChannelStatuses::default();
        statuses.set(GUILD, QUIET, Some("old".into()));
        statuses.set(Snowflake(9), LOUNGE, Some("elsewhere".into()));
        let info = json!({"guild_id": "1", "channels": [
            {"id": "2", "status": "game night", "voice_start_time": "2026-01-01T00:00:00+00:00"},
            {"id": "3", "status": null},
            {"nonsense": true},
        ]});
        let (guild, channels) = parse_info(&info).expect("channel info");
        statuses.replace_guild(guild, channels);
        assert_eq!(statuses.of(GUILD, LOUNGE), Some("game night"));
        assert_eq!(statuses.of(GUILD, QUIET), None, "the old one went");
        assert_eq!(statuses.of(Snowflake(9), LOUNGE), Some("elsewhere"), "another server's stays");
    }

    #[test]
    fn an_update_sets_and_clears_one_status_and_an_empty_one_is_none() {
        let mut statuses = ChannelStatuses::default();
        statuses.set(GUILD, LOUNGE, status(&json!("movie")));
        assert_eq!(statuses.of(GUILD, LOUNGE), Some("movie"));
        statuses.set(GUILD, LOUNGE, status(&json!("")));
        assert_eq!(statuses.of(GUILD, LOUNGE), None);
        statuses.set(GUILD, LOUNGE, status(&json!("again")));
        statuses.set(GUILD, LOUNGE, status(&json!(null)));
        assert_eq!(statuses.of(GUILD, LOUNGE), None);
    }

    #[test]
    fn a_server_that_is_gone_keeps_no_statuses() {
        let mut statuses = ChannelStatuses::default();
        statuses.set(GUILD, LOUNGE, Some("x".into()));
        statuses.forget_guild(GUILD);
        assert_eq!(statuses.of(GUILD, LOUNGE), None);
    }
}
