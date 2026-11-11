//! What the menus change in our notification settings, as the client sends it:
//! mutes, levels and the suppress switches (`updateGuildNotificationSettings`
//! and `updateChannelOverrideSettings` in 832712, saved by 931260), and what
//! reading and "Mark Unread" send (218461, 439147).

use serde_json::{Value, json};

use crate::Snowflake;
use crate::experiments::Experiments;
use crate::notifications::{Level, forum_flags, thread_flags};

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

/// The body of a settings request with the folds that aren't saved yet in it,
/// as `saveUserGuildSettingsBulk` adds them (931260): each category's
/// `collapsed`, in its override under its own server, whichever server the
/// request is for. `folds` are `(server, category, folded)`.
pub fn with_folds(mut body: Value, folds: &[(Snowflake, Snowflake, bool)]) -> Value {
    for &(guild, category, folded) in folds {
        body["guilds"][guild.to_string()]["channel_overrides"][category.to_string()]["collapsed"] = json!(folded);
    }
    body
}

/// Which messages in a joined thread notify, as the thread member's `flags`
/// (`EU` in 37411), with the level each stands for.
pub const THREAD_LEVELS: [(u64, Level, &str); 3] = [
    (thread_flags::ALL_MESSAGES, Level::All, "All Messages"),
    (thread_flags::ONLY_MENTIONS, Level::Mentions, "Only @mentions"),
    (thread_flags::NO_MESSAGES, Level::Nothing, "Nothing"),
];

/// A thread's level as the request sends it: the flags alone, with nothing
/// of the member's other bits (`setNotificationSettings` in 7909).
pub fn thread_level(flags: u64) -> Value {
    json!({"flags": flags})
}

/// A forum's channel override with New Posts switched, keeping its other
/// flags (`setForumThreadsCreated` in 832712).
pub fn new_posts(current: u64, on: bool) -> Value {
    let (set, clear) = if on {
        (forum_flags::NEW_POSTS_ON, forum_flags::NEW_POSTS_OFF)
    } else {
        (forum_flags::NEW_POSTS_OFF, forum_flags::NEW_POSTS_ON)
    };
    json!({"flags": (current & !clear) | set})
}

/// Whether the menus offer Mark Unread on a channel: the experiment's
/// variant 1 (511904), off for accounts Discord didn't put in it.
pub fn offers_mark_channel_unread(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-08-mark-channel-unread").is_some_and(|a| a.variant == 1)
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

/// Where the ack that marks `target` unread goes: the newest message we have
/// loaded before it, else the millisecond before it (439147).
pub fn unread_before(loaded: &[Snowflake], target: Snowflake) -> Snowflake {
    loaded
        .iter()
        .copied()
        .filter(|&id| id < target)
        .max()
        .unwrap_or_else(|| Snowflake::from_unix_ms(target.unix_ms().saturating_sub(1)))
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
    fn marking_unread_acks_the_message_before_or_the_millisecond_before() {
        let ids = [10, 20, 30].map(|n| Snowflake(n << 22));
        assert_eq!(unread_before(&ids, ids[2]), ids[1]);
        assert_eq!(unread_before(&ids, Snowflake(25 << 22)), ids[1], "a message we don't hold");
        let first = Snowflake((1_800_000_000_000 - 1_420_070_400_000) << 22);
        assert_eq!(unread_before(&[first], first).unix_ms(), first.unix_ms() - 1, "nothing before it");
    }

    #[test]
    fn marking_unread_is_a_manual_ack_with_the_mentions_after() {
        assert_eq!(unread_body(2), json!({"manual": true, "mention_count": 2}));
    }

    #[test]
    fn unsaved_folds_ride_with_any_servers_settings() {
        let body = bulk(Snowflake(1), channel(Snowflake(10), mute(-1, &at("2026-11-11T10:00:00Z"))));
        let sent = with_folds(body, &[(Snowflake(1), Snowflake(10), true), (Snowflake(2), Snowflake(20), false)]);
        assert_eq!(
            sent,
            json!({"guilds": {
                "1": {"channel_overrides": {"10": {
                    "muted": true,
                    "mute_config": {"selected_time_window": -1, "end_time": null},
                    "collapsed": true
                }}},
                "2": {"channel_overrides": {"20": {"collapsed": false}}},
            }})
        );
        assert_eq!(with_folds(json!({"guilds": {}}), &[]), json!({"guilds": {}}));
    }

    #[test]
    fn a_threads_level_is_its_flags_alone() {
        assert_eq!(thread_level(THREAD_LEVELS[1].0), json!({"flags": 4}));
        let flags: Vec<u64> = THREAD_LEVELS.iter().map(|l| l.0).collect();
        assert_eq!(flags, [2, 4, 8]);
    }

    #[test]
    fn new_posts_flips_its_two_flags_and_keeps_the_rest() {
        let other = 1 << 3;
        assert_eq!(new_posts(other, true), json!({"flags": other | 16_384}));
        assert_eq!(new_posts(other | 16_384, false), json!({"flags": other | 8192}));
        assert_eq!(new_posts(other | 8192, true), json!({"flags": other | 16_384}));
        assert_eq!(new_posts(0, false), json!({"flags": 8192}));
    }

    #[test]
    fn mark_unread_on_channels_is_variant_one_of_its_experiment() {
        let assigned = |variant: i64| {
            Experiments::from_ready(&json!({"apex_experiments": {"assignments": {"1": {"7": {"assignments": [
                [crate::experiments::murmur3("2026-08-mark-channel-unread"), variant, 0, 1]
            ]}}}}}))
        };
        let offered = |e: &Experiments| offers_mark_channel_unread(e, Snowflake(7));
        assert!(!offered(&Experiments::default()), "not in it");
        assert!(!offered(&assigned(0)), "control");
        assert!(offered(&assigned(1)));
        assert!(!offered(&assigned(2)), "a variant it doesn't define");
        assert!(!offers_mark_channel_unread(&assigned(1), Snowflake(8)), "someone else's");
    }
}
