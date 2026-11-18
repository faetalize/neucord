//! Slowmode, and how long a wait is in words (for slowmode and timeouts).
//!
//! The official client doesn't know a channel's cooldown either; it guesses
//! (`SlowmodeStore`). A cooldown starts when our message goes through, unless
//! the Bypass Slowmode permission lets us skip it, or when Discord refuses one
//! with [`crate::rest::SLOWMODE_RATE_LIMITED`] and says how long is left.

use std::collections::HashMap;

use crate::permissions::{self, Permissions};
use crate::verification::Account;
use crate::{Channel, Guild, Snowflake};

/// Added to every cooldown, and again each second it runs, as the official
/// client does, so the wait ends after Discord's rather than just before it.
const MARGIN_MS: i64 = 100;

/// Whether we skip slowmode in `channel`, one of `guild`'s: the client asks
/// for Bypass Slowmode there.
pub fn bypasses_slowmode(guild: &Guild, channel: &Channel, me: &Account, now_unix: i64) -> bool {
    permissions::in_channel(guild, channel, me, now_unix).contains(Permissions::BYPASS_SLOWMODE)
}

/// One running cooldown: the client's store sets what is left again every
/// second, 100 ms more than it was (`SLOWMODE_SET_COOLDOWN` in 101392), so
/// what it holds after `k` seconds is `initial - 900 k`, and it ends at the
/// first whole second where nothing is left to set.
#[derive(Debug, Clone, Copy)]
struct Wait {
    started_ms: i64,
    /// What it was set to, margin included.
    initial_ms: i64,
    /// The channel's slowmode when it started.
    rate: u32,
}

impl Wait {
    fn remaining_ms(&self, now_ms: i64) -> i64 {
        let seconds = (now_ms - self.started_ms).max(0) / 1000;
        if 900 * seconds >= self.initial_ms - MARGIN_MS { 0 } else { self.initial_ms - 900 * seconds }
    }
}

/// When each channel's cooldown started.
#[derive(Debug, Default)]
struct Waits(HashMap<Snowflake, Wait>);

impl Waits {
    /// Set the cooldown to `left_ms`, replacing the one running.
    fn set(&mut self, channel: &Channel, left_ms: i64, now_ms: i64) {
        self.0.remove(&channel.id);
        if left_ms > 0 {
            let wait = Wait { started_ms: now_ms, initial_ms: left_ms, rate: channel.rate_limit_per_user };
            self.0.insert(channel.id, wait);
        }
    }

    fn start(&mut self, channel: &Channel, bypass: bool, now_ms: i64) {
        let rate = channel.rate_limit_per_user;
        if rate > 0 && !bypass {
            self.set(channel, i64::from(rate) * 1000 + MARGIN_MS, now_ms);
        }
    }

    fn refused(&mut self, channel: &Channel, retry_after: f64, now_ms: i64) {
        if retry_after > 0.0 {
            self.set(channel, (retry_after * 1000.0).ceil() as i64 + MARGIN_MS, now_ms);
        }
    }

    fn update(&mut self, channel: &Channel, now_ms: i64) {
        let rate = channel.rate_limit_per_user;
        if let Some(wait) = self.0.get(&channel.id).copied()
            && wait.rate != rate
        {
            self.set(channel, wait.remaining_ms(now_ms).min(i64::from(rate) * 1000), now_ms);
        }
    }

    fn remaining_ms(&self, channel: &Channel, now_ms: i64) -> i64 {
        if channel.rate_limit_per_user == 0 {
            return 0;
        }
        self.0.get(&channel.id).map_or(0, |wait| wait.remaining_ms(now_ms))
    }
}

/// A channel's cooldowns for posting a message and for making a thread, which
/// the client keeps apart (`SendMessage` and `CreateThread` in
/// `SlowmodeStore`): making a thread doesn't hold the next message, nor a
/// message the next thread.
#[derive(Debug, Default)]
pub struct Slowmode {
    messages: Waits,
    threads: Waits,
}

impl Slowmode {
    /// Our message to `channel` is on its way, so another can't follow before
    /// it is answered. Call it again once it went through: Discord's wait
    /// starts when the message arrives, and so does the official client's.
    /// `bypass` is [`bypasses_slowmode`] for the channel's server.
    pub fn sent(&mut self, channel: &Channel, bypass: bool, now_ms: i64) {
        self.messages.start(channel, bypass, now_ms);
    }

    /// Our message to `channel` failed for some other reason than slowmode,
    /// so the wait it started is void. None ran before it: we don't send then.
    pub fn failed(&mut self, channel: &Channel) {
        self.messages.0.remove(&channel.id);
    }

    /// Discord refused a message in `channel` for slowmode, with `retry_after`
    /// seconds left. Only a wait we didn't know of, as after a message from
    /// another device, gets here.
    pub fn refused(&mut self, channel: &Channel, retry_after: f64, now_ms: i64) {
        self.messages.refused(channel, retry_after, now_ms);
    }

    /// Discord answered our typing notice in `channel` with how long we must
    /// wait before posting, which replaces what we had worked out
    /// (`SLOWMODE_SET_COOLDOWN` in `SlowmodeStore`). The client sets only a
    /// wait above zero, and `bypass` ends the wait instead.
    pub fn told(&mut self, channel: &Channel, cooldown_ms: i64, bypass: bool, now_ms: i64) {
        if cooldown_ms <= 0 {
            return;
        }
        self.messages.0.remove(&channel.id);
        if !bypass {
            self.messages.set(channel, cooldown_ms + MARGIN_MS, now_ms);
        }
    }

    /// We made a thread in `channel`: the next waits from now (the client's
    /// `SLOWMODE_RESET_COOLDOWN` for `CreateThread`, `Y` in 971389).
    pub fn thread_made(&mut self, channel: &Channel, bypass: bool, now_ms: i64) {
        self.threads.start(channel, bypass, now_ms);
    }

    /// Discord refused a thread in `channel` for slowmode, with `retry_after`
    /// seconds left.
    pub fn thread_refused(&mut self, channel: &Channel, retry_after: f64, now_ms: i64) {
        self.threads.refused(channel, retry_after, now_ms);
    }

    /// `channel` changed. A new slowmode can only shorten a running cooldown,
    /// as in the official client.
    pub fn update(&mut self, channel: &Channel, now_ms: i64) {
        self.messages.update(channel, now_ms);
        self.threads.update(channel, now_ms);
    }

    /// Milliseconds before we may post in `channel` again; 0 if we may now.
    /// Turning slowmode off ends the wait (the client's `isChannelOnCooldown`).
    pub fn remaining_ms(&self, channel: &Channel, now_ms: i64) -> i64 {
        self.messages.remaining_ms(channel, now_ms)
    }

    /// Milliseconds before we may make another thread in `channel`.
    pub fn thread_remaining_ms(&self, channel: &Channel, now_ms: i64) -> i64 {
        self.threads.remaining_ms(channel, now_ms)
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
        assert_eq!(slowmode.remaining_ms(&general, 12_299), 200, "it ends at the first tick that finds it up");
        assert_eq!(slowmode.remaining_ms(&general, 12_300), 0);
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
    fn the_store_adds_a_margin_every_second() {
        // Each second the client sets what is left again, plus 100 ms, so a
        // wait of 10.1 s is shown as 10.1, 9.2, 8.3 … and ends at a whole
        // second: the 12th.
        let mut slowmode = Slowmode::default();
        let general = channel(1, 10);
        slowmode.sent(&general, false, 5_000);
        let left = |at: i64| slowmode.remaining_ms(&general, 5_000 + at);
        assert_eq!([left(0), left(999), left(1_000), left(2_500)], [10_100, 10_100, 9_200, 8_300]);
        assert_eq!([left(10_999), left(11_000), left(11_999), left(12_000)], [1_100, 200, 200, 0]);
        // The unit of the margin is the second of the slowmode.
        let short = channel(2, 1);
        slowmode.sent(&short, false, 0);
        assert_eq!(slowmode.remaining_ms(&short, 0), 1_100);
        assert_eq!(slowmode.remaining_ms(&short, 1_000), 200, "the margin pushed the end past the first tick");
        assert_eq!(slowmode.remaining_ms(&short, 2_000), 0);
    }

    #[test]
    fn threads_wait_apart_from_messages() {
        let mut slowmode = Slowmode::default();
        let general = channel(1, 10);
        slowmode.thread_made(&general, false, 0);
        assert_eq!(slowmode.thread_remaining_ms(&general, 0), 10_100, "the next thread waits");
        assert_eq!(slowmode.remaining_ms(&general, 0), 0, "a message doesn't");

        slowmode.sent(&general, false, 1_000);
        assert_eq!(slowmode.thread_remaining_ms(&general, 1_000), 9_200, "nor does a message restart the thread's");
        assert_eq!(slowmode.remaining_ms(&general, 1_000), 10_100);

        slowmode.thread_refused(&channel(2, 10), 4.2, 0);
        assert_eq!(slowmode.thread_remaining_ms(&channel(2, 10), 0), 4_300);
        assert_eq!(slowmode.remaining_ms(&channel(2, 10), 0), 0);
        slowmode.thread_made(&channel(3, 10), true, 0);
        assert_eq!(slowmode.thread_remaining_ms(&channel(3, 10), 0), 0, "bypassed");

        slowmode.update(&channel(1, 5), 2_000);
        assert_eq!(slowmode.thread_remaining_ms(&channel(1, 5), 2_000), 5_000, "a new slowmode shortens both");
        assert_eq!(slowmode.remaining_ms(&channel(1, 5), 2_000), 5_000);
    }

    #[test]
    fn typing_tells_of_a_wait() {
        let mut slowmode = Slowmode::default();
        let general = channel(1, 10);
        slowmode.told(&general, 4_000, false, 1_000);
        assert_eq!(slowmode.remaining_ms(&general, 1_000), 4_100, "with the client's margin");

        slowmode.told(&general, 9_000, false, 2_000);
        assert_eq!(slowmode.remaining_ms(&general, 2_000), 9_100, "replaces the wait we had, longer or not");
        slowmode.told(&general, 1_000, false, 3_000);
        assert_eq!(slowmode.remaining_ms(&general, 3_000), 1_100);

        slowmode.told(&general, 0, false, 5_000);
        assert_eq!(slowmode.remaining_ms(&general, 5_000), 0, "only a wait above zero is set; the 1.1 s were up");
        slowmode.sent(&general, false, 10_000);
        slowmode.told(&general, 0, false, 10_500);
        assert_eq!(slowmode.remaining_ms(&general, 10_500), 10_100, "and zero leaves one running alone");

        slowmode.told(&general, 5_000, true, 11_000);
        assert_eq!(slowmode.remaining_ms(&general, 11_000), 0, "a moderator waits for nothing");
        slowmode.told(&channel(2, 0), 5_000, false, 0);
        assert_eq!(slowmode.remaining_ms(&channel(2, 0), 0), 0, "nor does a channel without slowmode");
    }

    #[test]
    fn bypass_slowmode() {
        let mut guild: Guild = serde_json::from_value(serde_json::json!({
            "id": "1", "name": "G", "owner_id": "6",
            "roles": [{"id": "1", "permissions": "3072"}, {"id": "40", "permissions": "4503599627370496"}],
        }))
        .unwrap();
        let me = Account { id: Snowflake(5), ..Default::default() };
        let general = channel(9, 10);
        assert!(!bypasses_slowmode(&guild, &general, &me, 0), "@everyone waits");
        guild.my_roles = vec![Snowflake(40)];
        assert!(bypasses_slowmode(&guild, &general, &me, 0), "a moderator doesn't");
        guild.my_roles.clear();
        guild.owner_id = Some(me.id);
        assert!(bypasses_slowmode(&guild, &general, &me, 0), "nor the owner");
    }

    #[test]
    fn slowmode_changes() {
        let mut slowmode = Slowmode::default();
        slowmode.refused(&channel(1, 60), 50.0, 0);

        slowmode.update(&channel(1, 60), 1_000);
        assert_eq!(slowmode.remaining_ms(&channel(1, 60), 1_000), 49_200, "same slowmode, same wait");

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
