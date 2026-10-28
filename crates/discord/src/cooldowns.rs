//! Slowmode, and how long a wait is in words (for slowmode and timeouts).
//!
//! The official client doesn't know a channel's cooldown either; it guesses
//! (`SlowmodeStore`). A cooldown starts when our message goes through, unless
//! the Bypass Slowmode permission lets us skip it, or when Discord refuses one
//! with [`crate::rest::SLOWMODE_RATE_LIMITED`] and says how long is left.
//!
//! Neucord can't compute that permission yet, so it holds everyone but the
//! server's owner (see [`bypasses_slowmode`]). A moderator waits when they
//! needn't; a message is never refused.

use std::collections::HashMap;

use crate::{Channel, Guild, Snowflake};

/// Added to every cooldown, as the official client does, so the wait ends
/// after Discord's rather than just before it.
const MARGIN_MS: i64 = 100;

/// Whether we skip slowmode in `guild`. The client asks for Bypass Slowmode;
/// until permissions are computed (#11), only the owner, who has them all,
/// is known to.
pub fn bypasses_slowmode(guild: &Guild, me: Snowflake) -> bool {
    guild.owner_id == Some(me)
}

#[derive(Debug, Default)]
pub struct Slowmode {
    /// When each channel's cooldown ends (ms since 1970), and the channel's
    /// slowmode when it started.
    ends: HashMap<Snowflake, (i64, u32)>,
}

impl Slowmode {
    /// Our message to `channel` is on its way, so another can't follow before
    /// it is answered. Call it again once it went through: Discord's wait
    /// starts when the message arrives, and so does the official client's.
    /// `bypass` is [`bypasses_slowmode`] for the channel's server.
    pub fn sent(&mut self, channel: &Channel, bypass: bool, now_ms: i64) {
        let rate = channel.rate_limit_per_user;
        if rate > 0 && !bypass {
            self.ends.insert(channel.id, (now_ms + i64::from(rate) * 1000 + MARGIN_MS, rate));
        }
    }

    /// Our message to `channel` failed for some other reason than slowmode,
    /// so the wait it started is void. None ran before it: we don't send then.
    pub fn failed(&mut self, channel: &Channel) {
        self.ends.remove(&channel.id);
    }

    /// Discord refused a message in `channel` for slowmode, with `retry_after`
    /// seconds left. Only a wait we didn't know of, as after a message from
    /// another device, gets here.
    pub fn refused(&mut self, channel: &Channel, retry_after: f64, now_ms: i64) {
        if retry_after > 0.0 {
            let end = now_ms + (retry_after * 1000.0).ceil() as i64 + MARGIN_MS;
            self.ends.insert(channel.id, (end, channel.rate_limit_per_user));
        }
    }

    /// `channel` changed. A new slowmode can only shorten a running cooldown,
    /// as in the official client.
    pub fn update(&mut self, channel: &Channel, now_ms: i64) {
        let rate = channel.rate_limit_per_user;
        if let Some((end, was)) = self.ends.get_mut(&channel.id)
            && *was != rate
        {
            *end = (*end).min(now_ms + i64::from(rate) * 1000);
            *was = rate;
        }
    }

    /// Milliseconds before we may post in `channel` again; 0 if we may now.
    /// Turning slowmode off ends the wait (the client's `isChannelOnCooldown`).
    pub fn remaining_ms(&self, channel: &Channel, now_ms: i64) -> i64 {
        if channel.rate_limit_per_user == 0 {
            return 0;
        }
        self.ends.get(&channel.id).map_or(0, |(end, _)| (end - now_ms).max(0))
    }
}

/// A wait in words, to the second, in its two largest units: "1 minute 5
/// seconds", "3 days 2 hours".
pub fn wait_words(seconds: i64) -> String {
    let units = [("day", 86_400), ("hour", 3_600), ("minute", 60), ("second", 1)];
    let seconds = seconds.max(1);
    let first = units.iter().position(|(_, size)| seconds >= *size).unwrap_or(3);
    let mut left = seconds;
    let mut words = Vec::new();
    for (name, size) in &units[first..(first + 2).min(units.len())] {
        let count = left / size;
        left %= size;
        if count > 0 {
            words.push(format!("{count} {name}{}", if count == 1 { "" } else { "s" }));
        }
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: u64, rate: u32) -> Channel {
        serde_json::from_value(serde_json::json!({ "id": id.to_string(), "type": 0, "rate_limit_per_user": rate }))
            .unwrap()
    }

    #[test]
    fn every_message_starts_a_cooldown() {
        let mut slowmode = Slowmode::default();
        let general = channel(1, 10);
        slowmode.sent(&general, false, 0);
        assert_eq!(slowmode.remaining_ms(&general, 0), 10_100, "from the first message, before any refusal");
        slowmode.sent(&general, false, 300);
        assert_eq!(slowmode.remaining_ms(&general, 300), 10_100, "restarted when it went through");
        assert_eq!(slowmode.remaining_ms(&general, 10_400), 0);
        assert_eq!(slowmode.remaining_ms(&channel(2, 10), 0), 0, "only in that channel");

        slowmode.sent(&channel(3, 0), false, 0);
        assert_eq!(slowmode.remaining_ms(&channel(3, 0), 0), 0, "no slowmode, no wait");
        slowmode.sent(&channel(4, 10), true, 0);
        assert_eq!(slowmode.remaining_ms(&channel(4, 10), 0), 0, "bypassed");

        slowmode.sent(&general, false, 20_000);
        slowmode.failed(&general);
        assert_eq!(slowmode.remaining_ms(&general, 20_000), 0, "a message that didn't go through starts none");

        slowmode.refused(&general, 4.2, 30_000);
        assert_eq!(slowmode.remaining_ms(&general, 30_000), 4_300, "Discord knew of a wait we didn't");
    }

    #[test]
    fn only_the_owner_bypasses() {
        let guild = |owner: &str| -> Guild {
            serde_json::from_value(serde_json::json!({ "id": "1", "name": "G", "owner_id": owner })).unwrap()
        };
        assert!(bypasses_slowmode(&guild("5"), Snowflake(5)));
        assert!(!bypasses_slowmode(&guild("6"), Snowflake(5)));
    }

    #[test]
    fn slowmode_changes() {
        let mut slowmode = Slowmode::default();
        slowmode.refused(&channel(1, 60), 50.0, 0);

        slowmode.update(&channel(1, 60), 1_000);
        assert_eq!(slowmode.remaining_ms(&channel(1, 60), 1_000), 49_100, "same slowmode, same wait");

        slowmode.update(&channel(1, 5), 1_000);
        assert_eq!(slowmode.remaining_ms(&channel(1, 5), 1_000), 5_000, "shortened to the new slowmode");
        slowmode.update(&channel(1, 120), 1_000);
        assert_eq!(slowmode.remaining_ms(&channel(1, 120), 1_000), 5_000, "never lengthened");
        assert_eq!(slowmode.remaining_ms(&channel(1, 0), 1_000), 0, "off ends it");
    }

    #[test]
    fn waits_in_words() {
        assert_eq!(wait_words(1), "1 second");
        assert_eq!(wait_words(0), "1 second", "never nothing while there is a wait");
        assert_eq!(wait_words(59), "59 seconds");
        assert_eq!(wait_words(60), "1 minute");
        assert_eq!(wait_words(125), "2 minutes 5 seconds");
        assert_eq!(wait_words(3_600 + 59), "1 hour", "seconds go once there are hours");
        assert_eq!(wait_words(28 * 86_400 - 1), "27 days 23 hours");
    }
}
