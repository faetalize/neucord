//! What the menus change in our notification settings, as the client sends it:
//! mutes, levels and the suppress switches (`updateGuildNotificationSettings`
//! and `updateChannelOverrideSettings` in 832712, saved by 931260), and what
//! reading and "Mark Unread" send (218461, 439147).

use serde_json::{Value, json};

use crate::Snowflake;

/// The mute durations the menus offer, in seconds, with their labels
/// (`MINUTES_15`.. `ALWAYS` in 355097, in the order 995102 lists them). `-1`
/// is until we turn it back on.
pub const MUTE_FOR: [(i64, &str); 6] = [
    (900, "For 15 Minutes"),
    (3600, "For 1 Hour"),
    (10_800, "For 3 Hours"),
    (28_800, "For 8 Hours"),
    (86_400, "For 24 Hours"),
    (-1, "Until I turn it back on"),
];

/// The settings of a mute for `seconds` from `now`: muted, with the window
/// picked and the time it ends, which `-1` has none of (812141).
pub fn mute(seconds: i64, now: &glib::DateTime) -> Value {
    let end = (seconds > 0).then(|| iso(&now.add_seconds(seconds as f64).unwrap_or_else(|_| now.clone())));
    json!({"muted": true, "mute_config": {"selected_time_window": seconds, "end_time": end}})
}

/// A time as `Date.prototype.toISOString` writes it: UTC, with milliseconds.
fn iso(time: &glib::DateTime) -> String {
    let utc = time.to_utc().unwrap_or_else(|_| time.clone());
    let text = utc.format("%Y-%m-%dT%H:%M:%S").map(|s| s.to_string()).unwrap_or_default();
    format!("{text}.{:03}Z", utc.microsecond() / 1000)
}

/// The settings of a channel's override, saved with the server's.
pub fn channel(channel: Snowflake, settings: Value) -> Value {
    json!({"channel_overrides": {channel.to_string(): settings}})
}

/// The body that saves `settings` for a server: the bulk endpoint's, keyed by
/// server (931260). The DMs' own settings (`None`) go as they are to their
/// own endpoint instead.
pub fn bulk(guild: Snowflake, settings: Value) -> Value {
    json!({"guilds": {guild.to_string(): settings}})
}

/// A read state `read-states/ack-bulk` takes (218461's `BULK_ACK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BulkAck {
    pub channel: Snowflake,
    pub message: Snowflake,
}

/// How many read states one request holds (`ep.splice(0, 100)`).
pub const BULK_ACK_BATCH: usize = 100;

/// The requests' bodies for `acks`: batches of [`BULK_ACK_BATCH`]. `read_state_type`
/// 0 is a channel's.
pub fn bulk_ack_bodies(acks: &[BulkAck]) -> Vec<Value> {
    acks.chunks(BULK_ACK_BATCH)
        .map(|batch| {
            let states: Vec<Value> = batch
                .iter()
                .map(|a| json!({"channel_id": a.channel, "message_id": a.message, "read_state_type": 0}))
                .collect();
            json!({"read_states": states})
        })
        .collect()
}

/// The body of an ack that marks a message unread: the ack goes to the message
/// before it, `manual`, with the mentions after it (439147).
pub fn unread_body(mention_count: u32) -> Value {
    json!({"manual": true, "mention_count": mention_count})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(iso: &str) -> glib::DateTime {
        glib::DateTime::from_iso8601(iso, None).unwrap()
    }

    #[test]
    fn a_mute_names_its_window_and_when_it_ends() {
        let now = at("2026-11-11T10:00:00.250Z");
        assert_eq!(
            mute(900, &now),
            json!({"muted": true, "mute_config": {"selected_time_window": 900, "end_time": "2026-11-11T10:15:00.250Z"}})
        );
        assert_eq!(mute(86_400, &now)["mute_config"]["end_time"], json!("2026-11-12T10:00:00.250Z"), "across midnight");
        assert_eq!(
            mute(-1, &now),
            json!({"muted": true, "mute_config": {"selected_time_window": -1, "end_time": null}}),
            "until turned back on has no end"
        );
    }

    #[test]
    fn the_menu_offers_the_clients_durations_in_its_order() {
        let seconds: Vec<i64> = MUTE_FOR.iter().map(|(s, _)| *s).collect();
        assert_eq!(seconds, [900, 3600, 10_800, 28_800, 86_400, -1]);
    }

    #[test]
    fn settings_are_keyed_the_way_the_client_saves_them() {
        let inner = channel(Snowflake(7), json!({"muted": false}));
        assert_eq!(inner, json!({"channel_overrides": {"7": {"muted": false}}}));
        assert_eq!(bulk(Snowflake(3), inner), json!({"guilds": {"3": {"channel_overrides": {"7": {"muted": false}}}}}));
    }

    #[test]
    fn bulk_acks_go_a_hundred_at_a_time() {
        let acks: Vec<BulkAck> =
            (1..=250).map(|n| BulkAck { channel: Snowflake(n), message: Snowflake(n * 10) }).collect();
        let bodies = bulk_ack_bodies(&acks);
        let sizes: Vec<usize> = bodies.iter().map(|b| b["read_states"].as_array().unwrap().len()).collect();
        assert_eq!(sizes, [100, 100, 50]);
        assert_eq!(bodies[0]["read_states"][0], json!({"channel_id": "1", "message_id": "10", "read_state_type": 0}));
        assert!(bulk_ack_bodies(&[]).is_empty());
    }

    #[test]
    fn marking_unread_is_a_manual_ack_with_the_mentions_after() {
        assert_eq!(unread_body(2), json!({"manual": true, "mention_count": 2}));
    }
}
