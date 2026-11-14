//! /roll-dice and the roll it starts (`DiceRollLifecycleManager`, 545185):
//! the dice are rolled here, shown above the message box, then sent as a
//! message whose link rolls them again for whoever clicks it in that
//! channel (`FM` in 376943, through 362280). The command is offered only in
//! `2026-05-dice-roll-slash-command` (12115); a link rolls for anyone.

use crate::Snowflake;
use crate::experiments::Experiments;

/// The dice there are, by their sides (46013).
pub const SIDES: [u32; 6] = [4, 6, 8, 10, 12, 20];

/// The most dice the command rolls; a link may ask for 42.
pub const MAX_COUNT: u32 = 10;

/// The ones a link can roll past [`MAX_COUNT`].
const ANSWER: u32 = 42;

/// How long the dice roll before they land, and how long what they show
/// stays above the box after (545185), in milliseconds.
pub const ROLLING_MS: u32 = 1500;
pub const SHOWN_MS: u32 = 2000;

/// /roll-dice is offered: variant 1 of the Apex user experiment (its
/// `enabled`), which the client calls pre-alpha.
pub fn enabled(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-05-dice-roll-slash-command").is_some_and(|a| a.variant == 1)
}

/// What a roll throws: how many dice, and their sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dice {
    pub count: u32,
    pub sides: u32,
}

impl Dice {
    /// As `handleStartRoll` takes them: between 1 and [`MAX_COUNT`] dice
    /// unless 42, and a die it doesn't have is a D6.
    pub fn new(count: u64, sides: u64) -> Dice {
        let count = if count == u64::from(ANSWER) { ANSWER } else { count.clamp(1, u64::from(MAX_COUNT)) as u32 };
        let sides = SIDES.into_iter().find(|&s| u64::from(s) == sides).unwrap_or(6);
        Dice { count, sides }
    }

    /// The link that rolls these dice in `channel`, one of `guild`'s or a DM.
    pub fn link(self, guild: Option<Snowflake>, channel: Snowflake) -> String {
        let guild = guild.map_or_else(|| "@me".to_owned(), |g| g.to_string());
        format!("https://discord.com/channels/{guild}/{channel}/roll-dice/{}d{}", self.count, self.sides)
    }

    /// "a D6", "3 D20": the count as `uV5JaG` and `tmSbYW` plural it.
    fn named(self) -> String {
        match self.count {
            1 => format!("a D{}", self.sides),
            n => format!("{n} D{}", self.sides),
        }
    }

    /// The message a roll sends (`sendMessage` in 545185), before its
    /// `:game_die:`s become emoji: a heading with the total and the link to
    /// roll the same, and under it each die in small print when there are
    /// several.
    pub fn message(self, guild: Option<Snowflake>, channel: Snowflake, results: &[u32]) -> String {
        let total: u32 = results.iter().sum();
        let named = self.named();
        let heading =
            format!("### I rolled {named} and got **{total}**! [`Roll {named}`]({})", self.link(guild, channel));
        if self.count == 1 {
            return heading;
        }
        let each: Vec<String> = results.iter().map(|r| format!(":game_die: {r}")).collect();
        format!("{heading}\n-# {}", each.join(" "))
    }
}

/// What the bar above the box says while the dice roll (`x/FIRX`).
pub const ROLLING: &str = "Rolling...";

/// What it says once they land (`xU4pF1`).
pub fn rolled(results: &[u32]) -> String {
    format!("You rolled {}!", results.iter().sum::<u32>())
}

/// The roll a link asks for, and in which channel: one on a Discord host
/// (`isDiscordHostname`, 998218) whose path is `FM`'s in 376943. A link
/// without its dice (`…/roll-dice`) isn't one here: the client reads its
/// missing count as `NaN` and would send a message about rolling NaN dice.
pub fn from_link(url: &str) -> Option<(Snowflake, Dice)> {
    let uri = glib::Uri::parse(url, glib::UriFlags::NONE).ok()?;
    let host = uri.host()?.to_ascii_lowercase();
    let discord = ["discordapp.com", "discord.com", "discordmerch.com"]
        .iter()
        .any(|d| host == *d || host.strip_suffix(d).is_some_and(|sub| sub.ends_with('.')));
    if !discord {
        return None;
    }
    let path = uri.path();
    let rest = path.strip_prefix("/channels/")?;
    let mut parts = rest.split('/');
    let (guild, channel, roll, dice) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || roll != "roll-dice" || !(guild == "@me" || is_number(guild)) {
        return None;
    }
    let (count, sides) = dice.split_once('d')?;
    if !is_number(channel) || !is_number(count) || !is_number(sides) {
        return None;
    }
    // `parseInt` of a number too long for u64 is still past every limit.
    let number = |s: &str| s.parse::<u64>().unwrap_or(u64::MAX);
    Some((Snowflake(channel.parse().ok()?), Dice::new(number(count), number(sides))))
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dice_are_taken_as_the_client_takes_them() {
        assert_eq!(Dice::new(3, 20), Dice { count: 3, sides: 20 });
        assert_eq!(Dice::new(0, 6).count, 1);
        assert_eq!(Dice::new(11, 6).count, 10);
        assert_eq!(Dice::new(42, 6).count, 42, "the answer");
        assert_eq!(Dice::new(1, 7).sides, 6, "no D7");
        assert_eq!(Dice::new(1, 100).sides, 6);
    }

    #[test]
    fn the_message_reads_as_the_clients() {
        let channel = Snowflake(20);
        assert_eq!(
            Dice::new(1, 6).message(Some(Snowflake(10)), channel, &[4]),
            "### I rolled a D6 and got **4**! [`Roll a D6`](https://discord.com/channels/10/20/roll-dice/1d6)"
        );
        assert_eq!(
            Dice::new(3, 20).message(None, channel, &[1, 20, 7]),
            "### I rolled 3 D20 and got **28**! [`Roll 3 D20`](https://discord.com/channels/@me/20/roll-dice/3d20)\n\
             -# :game_die: 1 :game_die: 20 :game_die: 7"
        );
        assert_eq!(rolled(&[1, 20, 7]), "You rolled 28!");
    }

    #[test]
    fn links_roll_on_discord_hosts_only() {
        let roll = |count, sides| Some((Snowflake(20), Dice { count, sides }));
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice/3d20"), roll(3, 20));
        assert_eq!(from_link("https://discord.com/channels/@me/20/roll-dice/1d6"), roll(1, 6));
        assert_eq!(from_link("https://canary.discord.com/channels/10/20/roll-dice/2d8"), roll(2, 8));
        assert_eq!(from_link("https://DISCORDAPP.com/channels/10/20/roll-dice/2d8"), roll(2, 8));
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice/42d6"), roll(42, 6));
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice/99d7"), roll(10, 6));
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice/99999999999999999999d6"), roll(10, 6));
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice"), None, "no dice");
        assert_eq!(from_link("https://discord.com/channels/10/20/roll-dice/3d20/x"), None);
        assert_eq!(from_link("https://discord.com/channels/10/20/30"), None);
        assert_eq!(from_link("https://notdiscord.com/channels/10/20/roll-dice/1d6"), None);
        assert_eq!(from_link("https://discord.com.evil/channels/10/20/roll-dice/1d6"), None);
        assert_eq!(from_link("https://discord.gg/channels/10/20/roll-dice/1d6"), None);
        assert_eq!(from_link("not a link"), None);
    }

    #[test]
    fn the_experiment_is_variant_one() {
        let assigned = |variant: i64| {
            Experiments::from_ready(&json!({"apex_experiments": {"assignments": {"1": {"7": {"assignments": [
                [crate::experiments::murmur3("2026-05-dice-roll-slash-command"), variant, 0, 1]
            ]}}}}}))
        };
        let me = Snowflake(7);
        assert!(!enabled(&Experiments::default(), me));
        assert!(!enabled(&assigned(0), me), "control");
        assert!(enabled(&assigned(1), me));
        assert!(!enabled(&assigned(1), Snowflake(8)), "someone else's");
    }
}
